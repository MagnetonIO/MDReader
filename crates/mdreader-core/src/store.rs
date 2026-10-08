use crate::{parse_markdown, visit_source_root, ScanOptions, SourceRoot, SourceRootIdentity};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, UNIX_EPOCH};

#[path = "related.rs"]
mod related;
pub use related::*;
#[path = "provider_projection.rs"]
mod projection;
pub use projection::{ProviderProjection, ProviderProjectionReceipt, SourceKind};
#[path = "catalog/mod.rs"]
pub mod catalog;
mod local_source;
pub use local_source::publish_local_source;
#[cfg(feature = "device-assets")]
pub(crate) use local_source::validate_root;

pub const MODEL_NAME: &str = "bge-small-en-v1.5";
pub const EMBEDDING_DIM: usize = 384;
pub const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;
#[derive(Debug, thiserror::Error)]
pub enum ReaderError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Source authority: {0}")]
    Source(#[from] crate::SourceRootError),
    #[error("Invalid request: {0}")]
    Invalid(String),
    #[error("Source unavailable")]
    Unavailable,
    #[error("Local embedding model unavailable")]
    ModelUnavailable,
    #[error("Cancelled")]
    Cancelled,
    #[error("Document revision changed")]
    Conflict,
    #[error("Unsupported database version {0}")]
    NewerDatabase(i64),
}
pub type Result<T> = std::result::Result<T, ReaderError>;
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            Err(ReaderError::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub source_id: String,
    pub path: String,
    pub revision: String,
    pub title: String,
    pub heading: String,
    pub line_start: u32,
    pub line_end: u32,
    pub text: String,
    pub score: f64,
    #[serde(default)]
    pub lexical_rank: Option<u32>,
    #[serde(default)]
    pub semantic_rank: Option<u32>,
    #[serde(default)]
    pub modified_ms: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub source_id: String,
    pub path: String,
    pub revision: String,
    pub markdown: String,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct ListedDocument {pub path:String,pub revision:String,pub modified_ms:i64}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatus {
    pub documents: u64,
    pub chunks: u64,
    pub embedded: u64,
    pub model: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexJob {
    pub source_id: String,
    pub phase: String,
    pub documents: u64,
    pub chunks: u64,
    pub embedded: u64,
    pub error: Option<String>,
    pub generation: u64,
}
/// One service owns this pair. Durable user records never live in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub source_id: String,
    pub path: String,
    pub revision: String,
    pub line: u32,
}

pub struct ReaderStore {
    library: Connection,
    index: Connection,
    #[cfg(feature = "device-assets")]
    owner_session: std::sync::Arc<()>,
}
const LIBRARY_SCHEMA: &str = "
CREATE TABLE sources(id TEXT PRIMARY KEY, name TEXT NOT NULL, path BLOB NOT NULL,
 identity TEXT NOT NULL, enabled INTEGER NOT NULL CHECK(enabled IN (0,1)));
CREATE TABLE bookmarks(source_id TEXT NOT NULL REFERENCES sources(id), path TEXT NOT NULL,
 line INTEGER NOT NULL CHECK(line>0), PRIMARY KEY(source_id,path));
PRAGMA user_version=1;";
const INDEX_SCHEMA: &str = "
CREATE TABLE documents(id INTEGER PRIMARY KEY, source_id TEXT NOT NULL, path TEXT NOT NULL,
 revision TEXT NOT NULL, title TEXT NOT NULL, modified_ms INTEGER NOT NULL, scan TEXT NOT NULL, UNIQUE(source_id,path));
CREATE TABLE chunks(id INTEGER PRIMARY KEY, document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 heading TEXT NOT NULL, line_start INTEGER NOT NULL, line_end INTEGER NOT NULL, text TEXT NOT NULL, embedding_text TEXT NOT NULL);
CREATE INDEX chunks_document ON chunks(document_id);
CREATE VIRTUAL TABLE chunks_fts USING fts5(text,heading,content='chunks',content_rowid='id',tokenize='unicode61');
CREATE VIRTUAL TABLE vectors USING vec0(embedding float[384] distance_metric=cosine);
CREATE TRIGGER chunks_insert AFTER INSERT ON chunks BEGIN
 INSERT INTO chunks_fts(rowid,text,heading) VALUES(new.id,new.text,new.heading); END;
CREATE TRIGGER chunks_delete AFTER DELETE ON chunks BEGIN
 INSERT INTO chunks_fts(chunks_fts,rowid,text,heading) VALUES('delete',old.id,old.text,old.heading);
 DELETE FROM vectors WHERE rowid=old.id; END;
PRAGMA user_version=1;";
fn register_vectors() -> Result<()> {
    static REGISTERED: OnceLock<i32> = OnceLock::new();
    let rc = *REGISTERED.get_or_init(|| {
        // SAFETY: the statically linked sqlite-vec entry point has SQLite's
        // extension ABI. SQLite copies the pointer; no borrowed data is retained.
        #[allow(unsafe_code)]
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::os::raw::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const ()
            )))
        }
    });
    if rc == rusqlite::ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(ReaderError::Invalid(
            "sqlite-vec initialization failed".into(),
        ))
    }
}
const LINK_SCHEMA: &str = "
CREATE TABLE headings(document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 text TEXT NOT NULL, slug TEXT NOT NULL, line INTEGER NOT NULL, PRIMARY KEY(document_id,slug));
CREATE TABLE links(document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 target TEXT NOT NULL, kind TEXT NOT NULL, line INTEGER NOT NULL);
CREATE INDEX links_document ON links(document_id);
PRAGMA user_version=2;";
const JOB_SCHEMA: &str = "
CREATE TABLE index_jobs(source_id TEXT PRIMARY KEY REFERENCES sources(id),phase TEXT NOT NULL,
 documents INTEGER NOT NULL DEFAULT 0,chunks INTEGER NOT NULL DEFAULT 0,embedded INTEGER NOT NULL DEFAULT 0,error TEXT);
PRAGMA user_version=2;";
const TAG_SCHEMA: &str = "
CREATE TABLE document_tags(document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,tag TEXT NOT NULL,
 PRIMARY KEY(document_id,tag));
CREATE INDEX tags_lookup ON document_tags(tag,document_id);
PRAGMA user_version=3;";
// Both tables are derived from indexed Markdown. Keep ambiguous aliases rather
// than silently choosing a document; edges carry the resolved heading evidence.
const REFERENCE_SCHEMA: &str = "
CREATE TABLE document_aliases(source_id TEXT NOT NULL,kind TEXT NOT NULL,alias TEXT NOT NULL,
 document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 PRIMARY KEY(source_id,kind,alias,document_id)) WITHOUT ROWID;
CREATE INDEX aliases_document ON document_aliases(document_id);
CREATE TABLE resolved_links(source_id TEXT NOT NULL,
 origin_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 target_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 origin_line INTEGER NOT NULL,target_line INTEGER NOT NULL,
 PRIMARY KEY(source_id,origin_id,target_id,origin_line,target_line)) WITHOUT ROWID;
CREATE INDEX resolved_incoming ON resolved_links(source_id,target_id,origin_id);
CREATE INDEX resolved_target ON resolved_links(target_id,origin_id);
CREATE INDEX resolved_origin ON resolved_links(origin_id,target_id);
PRAGMA user_version=4;";
// Row membership is derived in the same transaction as each vector install.
// Counting/progress must never read 384-float vector payloads. Chunk deletion
// cascades through both this table and the existing vector cleanup trigger.
const EMBEDDING_MEMBERSHIP_SCHEMA: &str = "
CREATE TABLE embedding_memberships(chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
 source_id TEXT NOT NULL);
CREATE INDEX embedding_memberships_source ON embedding_memberships(source_id,chunk_id);
INSERT INTO embedding_memberships(chunk_id,source_id)
 SELECT v.rowid,d.source_id FROM vectors v
 CROSS JOIN chunks c ON c.id=v.rowid
 CROSS JOIN documents d ON d.id=c.document_id;
PRAGMA user_version=5;";
const INDEX_VERSION: i64 = 5;
const LIBRARY_VERSION: i64 = 4;
const MAX_REFERENCE_MATCHES: usize = 10_000;
const REFERENCE_AMBIGUITY_BUDGET: &str = "reference ambiguity exceeds budget";
const MAX_SOURCE_LINKS: usize = 100_000;
const MAX_RESOLVED_EDGES: usize = 1_000_000;

fn open_database(path: &Path, schema: &str, index: bool) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::default() | rusqlite::OpenFlags::SQLITE_OPEN_URI)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > if index {INDEX_VERSION} else {LIBRARY_VERSION} {
        return Err(ReaderError::NewerDatabase(version));
    }
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
    )?;
    if version == 0 {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(schema)?;
        tx.commit()?;
    }
    if index { connection.execute_batch("CREATE TEMP VIEW available_documents AS SELECT * FROM documents;")?; }
    if index && version < 2 {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(LINK_SCHEMA)?;
        tx.commit()?;
    }
    if index && version < 3 {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(TAG_SCHEMA)?;
        // Derived rows must be parsed again to populate tags after upgrade.
        tx.execute("DELETE FROM documents", [])?;
        tx.commit()?;
    }
    if index && version < 4 {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(REFERENCE_SCHEMA)?;
        {
            let mut documents = tx.prepare("SELECT id,source_id,path,title FROM documents")?;
            let mut rows = documents.query([])?;
            while let Some(row) = rows.next()? {
                add_document_aliases(&tx, row.get(0)?, &row.get::<_,String>(1)?, &row.get::<_,String>(2)?, &row.get::<_,String>(3)?)?;
            }
        }
        let sources: Vec<String> = tx.prepare("SELECT DISTINCT source_id FROM documents")?
            .query_map([], |row| row.get(0))?.collect::<std::result::Result<_,_>>()?;
        for source in sources {
            rebuild_resolved_links(&tx, &source, &Cancellation::default())?;
        }
        tx.commit()?;
    }
    if index && version < 5 {
        let tx = connection.unchecked_transaction()?;
        // Public vec0 rowid iteration only. CROSS JOIN keeps vectors outermost
        // so migration scans membership rather than fetching each payload.
        tx.execute_batch(EMBEDDING_MEMBERSHIP_SCHEMA)?;
        tx.commit()?;
    }
    if !index && version < 2 {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(JOB_SCHEMA)?;
        tx.commit()?;
    }
    if !index && version < 3 {
        let tx=connection.unchecked_transaction()?;
        tx.execute_batch("ALTER TABLE index_jobs ADD COLUMN generation INTEGER NOT NULL DEFAULT 1; PRAGMA user_version=3;")?;
        tx.commit()?;
    }
    if !index && version < 4 {
        let tx=connection.unchecked_transaction()?;
        tx.execute_batch("CREATE TABLE capture_policies(project_id TEXT PRIMARY KEY,configuration TEXT NOT NULL); CREATE TABLE captures(id TEXT PRIMARY KEY,source_id TEXT NOT NULL REFERENCES sources(id),path TEXT NOT NULL,revision TEXT NOT NULL,provenance TEXT NOT NULL); PRAGMA user_version=4;")?;
        tx.commit()?;
    }
    Ok(connection)
}
fn open_readonly_database(path: &Path, expected: i64) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX | rusqlite::OpenFlags::SQLITE_OPEN_URI)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > expected { return Err(ReaderError::NewerDatabase(version)); }
    if version != expected { return Err(invalid("database requires owner migration")); }
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(connection)
}
// Native path units are durable authority, never a lossy display string.
fn encode_path(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}
fn decode_path(bytes: Vec<u8>) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes).into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if bytes.len() % 2 != 0 {
            return Err(ReaderError::Invalid("invalid native path".into()));
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|x| u16::from_le_bytes([x[0], x[1]]))
            .collect();
        Ok(std::ffi::OsString::from_wide(&units).into())
    }
}
fn revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
fn invalid(message: &str) -> ReaderError {
    ReaderError::Invalid(message.into())
}
fn checked_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\\')
        || path.contains('\0')
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(invalid("expected source-relative document path"));
    }
    Ok(())
}
fn vector_bytes(vector: &[f32]) -> Result<Vec<u8>> {
    if vector.len() != EMBEDDING_DIM
        || vector.iter().any(|x| !x.is_finite())
        || vector.iter().all(|x| *x == 0.0)
    {
        return Err(invalid("expected finite nonzero 384-dimensional embedding"));
    }
    Ok(vector.iter().flat_map(|x| x.to_le_bytes()).collect())
}
fn index_markdown(tx: &rusqlite::Transaction<'_>, id: &str, path: &str, hash: &str, markdown: &str, modified: i64, scan: &str) -> Result<bool> {
    let known: Option<(i64, String)> = tx
        .query_row(
            "SELECT id,revision FROM documents WHERE source_id=?1 AND path=?2",
            params![id, path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((document, old)) = &known {
        if old == hash {
            tx.execute(
                "UPDATE documents SET scan=?2,modified_ms=?3 WHERE id=?1",
                params![document, scan, modified],
            )?;
            return Ok(false);
        }
        tx.execute("DELETE FROM documents WHERE id=?1", [document])?;
    }
    let parsed = parse_markdown(markdown, path);
    tx.execute("INSERT INTO documents(source_id,path,revision,title,modified_ms,scan) VALUES(?1,?2,?3,?4,?5,?6)",params![id,path,hash,parsed.title,modified,scan])?;
    let document = tx.last_insert_rowid();
    add_document_aliases(tx, document, id, path, &parsed.title)?;
    for (key,value) in &parsed.frontmatter {
        if key.eq_ignore_ascii_case("tags") || key.eq_ignore_ascii_case("tag") {
            for tag in value.trim_matches(['[',']']).split(',') {
                let tag=tag.trim().trim_matches(['\'', '"']).trim_start_matches('#').to_lowercase();
                if !tag.is_empty() { tx.execute("INSERT OR IGNORE INTO document_tags VALUES(?1,?2)",params![document,tag])?; }
            }
        }
    }
    let mut slugs: HashMap<String, u32> = HashMap::new();
    for section in parsed.sections {
        if section.depth > 0 {
            let text = section.heading_path.last().cloned().unwrap_or_default();
            let base = heading_slug(&text);
            let mut slug = base.clone();
            let mut suffix = 0;
            while slugs.contains_key(&slug) {
                suffix += 1;
                slug = format!("{base}-{suffix}");
            }
            slugs.insert(slug.clone(), section.start_line);
            tx.execute(
                "INSERT INTO headings VALUES(?1,?2,?3,?4)",
                params![document, text, slug, section.start_line],
            )?;
        }
        for link in &section.links {
            tx.execute(
                "INSERT INTO links VALUES(?1,?2,?3,?4)",
                params![document, link.raw_target, link.kind, link.line],
            )?;
        }
        let mut offset = 0;
        for chunk in section.chunks {
            let position = section.text[offset..]
                .find(&chunk.text)
                .map(|p| p + offset)
                .unwrap_or(offset);
            let start = section.start_line
                + section.text[..position]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count() as u32;
            let end = start
                + chunk
                    .text
                    .trim_end_matches('\n')
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count() as u32;
            offset = position + chunk.text.len();
            tx.execute("INSERT INTO chunks(document_id,heading,line_start,line_end,text,embedding_text) VALUES(?1,?2,?3,?4,?5,?6)",params![document,section.heading_path.join(" > "),start,end,chunk.text,chunk.embedding_text])?;
        }
    }
    Ok(true)
}

impl ReaderStore {
    pub fn open(directory: &Path) -> Result<Self> {
        register_vectors()?;
        std::fs::create_dir_all(directory)?;
        let library=open_database(&directory.join("library.sqlite"), LIBRARY_SCHEMA, false)?;
        projection::initialize(&library)?;
        let index=open_database(&directory.join("index.sqlite"), INDEX_SCHEMA, true)?;
        projection::attach_authority(&index,&directory.join("library.sqlite"))?;
        catalog::schema::initialize(&library,&index)?;
        Ok(Self {
            library,
            index,
            #[cfg(feature = "device-assets")]
            owner_session: std::sync::Arc::new(()),
        })
    }
    /// Read existing owner-managed databases without creating or migrating them.
    pub fn open_readonly(directory: &Path) -> Result<Self> {
        register_vectors()?;
        let library=open_readonly_database(&directory.join("library.sqlite"), LIBRARY_VERSION)?;
        projection::check_schema(&library)?;
        let index=open_readonly_database(&directory.join("index.sqlite"), INDEX_VERSION)?;
        projection::attach_authority(&index,&directory.join("library.sqlite"))?;
        Ok(Self {
            library,
            index,
            #[cfg(feature = "device-assets")]
            owner_session: std::sync::Arc::new(()),
        })
    }
    /// Ephemeral capability for this live owner; reopening needs fresh sessions.
    #[cfg(feature = "device-assets")]
    pub(crate) fn device_owner_session(&self) -> std::sync::Arc<()> {
        self.owner_session.clone()
    }
    /// Borrow the owner's durable connection, including for sync transactions.
    pub fn with_library_connection<T>(&mut self, operation: impl FnOnce(&mut Connection) -> T) -> T {
        operation(&mut self.library)
    }
    /// Look up an exact persisted binding without admitting or reopening it.
    /// This also permits revocation after a directory disappears or is replaced.
    pub fn find_local_source(&self, path: &Path, identity: &SourceRootIdentity) -> Result<Option<Source>> {
        let identity = serde_json::to_string(identity).map_err(|_| invalid("invalid source identity"))?;
        Ok(self.library.query_row("SELECT id,name,enabled FROM sources WHERE identity=?1 AND path=?2 AND id NOT LIKE 'provider:%'",
            params![identity, encode_path(path)], |r| Ok(Source { id:r.get(0)?, name:r.get(1)?, enabled:r.get(2)? })).optional()?)
    }
    pub fn list_sources(&self) -> Result<Vec<Source>> {
        Ok(self
            .library
            .prepare("SELECT id,name,enabled FROM sources ORDER BY name,id")?
            .query_map([], |r| {
                Ok(Source {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    enabled: r.get(2)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?)
    }
    fn stored_root(&self, id: &str, require_enabled: bool) -> Result<SourceRoot> {
        crate::device::source_authority(&self.library,id)?;
        if id.starts_with("provider:") {self.provider_enabled(id)?;}
        let row: Option<(Vec<u8>, String, bool)> = self
            .library
            .query_row(
                "SELECT path,identity,enabled FROM sources WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (path, identity, enabled) = row.ok_or(ReaderError::Unavailable)?;
        if require_enabled && !enabled {
            return Err(ReaderError::Unavailable);
        }
        let identity: SourceRootIdentity =
            serde_json::from_str(&identity).map_err(|_| invalid("invalid source identity"))?;
        Ok(SourceRoot::restore(decode_path(path)?, identity)?)
    }
    fn scope(&self, sources: &[String]) -> Result<Vec<String>> {
        if sources.is_empty() || sources.len() > 128 {
            return Err(invalid("select 1–128 sources"));
        }
        let ids: Vec<_> = sources
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for id in &ids {
            self.stored_root(id, true)?.verify()?;
        }
        Ok(ids)
    }
    pub fn set_source_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        if id.starts_with("provider:") {
            if enabled {return Err(invalid("provider sources require a new explicit binding generation"));}
            let tx=self.library.transaction()?;
            tx.execute("UPDATE core_provider_slots SET enabled=0 WHERE binding_digest IN(SELECT binding_digest FROM core_provider_sources WHERE reader_source_id=?1)",[id])?;
            tx.execute("UPDATE sources SET enabled=0 WHERE id=?1",[id])?;
            tx.execute("UPDATE core_provider_jobs SET state='revoked' WHERE reader_source_id=?1",[id])?;
            tx.execute("UPDATE index_jobs SET phase='error',error='Source unavailable',generation=generation+1 WHERE source_id=?1",[id])?;
            tx.commit()?;return Ok(());
        }
        if enabled {
            self.stored_root(id, false)?.verify()?;
        }
        if crate::device::set_source_enabled(&mut self.library,id,enabled)? {return Ok(());}
        if self.library.execute(
            "UPDATE sources SET enabled=?2 WHERE id=?1",
            params![id, enabled],
        )? != 1
        {
            return Err(ReaderError::Unavailable);
        }
        Ok(())
    }
    /// Revoke first, then remove only rebuildable index data. The disabled
    /// durable binding remains as a tombstone against late Admit requests;
    /// durable bookmarks and original Markdown are preserved for reactivation.
    pub fn forget_source_index(&mut self, id: &str) -> Result<()> {
        self.set_source_enabled(id, false)?;
        let tx = self.index.transaction()?;
        tx.execute("DELETE FROM documents WHERE source_id=?1", [id])?;
        tx.execute("DELETE FROM reader_catalog_snapshots_v1 WHERE source_id=?1", [id])?;
        tx.commit()?;
        self.library.execute("DELETE FROM index_jobs WHERE source_id=?1", [id])?;
        Ok(())
    }
    pub fn index_source(&mut self, id: &str) -> Result<usize> {
        self.index_source_cancelled(id, &Cancellation::default())
    }
    pub fn index_source_cancelled(&mut self, id: &str, cancel: &Cancellation) -> Result<usize> {
        cancel.check()?;
        if id.starts_with("provider:") {return self.index_provider(id,cancel);}
        let root = self.stored_root(id, true)?;
        let options=ScanOptions {include_mdx:false,max_file_size:MAX_DOCUMENT_BYTES,respect_gitignore:true};
        let scan = uuid::Uuid::new_v4().to_string();
        let tx = self.index.transaction()?;
        let mut changed = false;
        let count=visit_source_root(&root,&options,|file|->Result<()> {
            cancel.check()?;
            let bytes = file.read_bounded(MAX_DOCUMENT_BYTES)?;
            if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
                return Err(invalid("document grew beyond size limit"));
            }
            let markdown =
                std::str::from_utf8(&bytes).map_err(|_| invalid("Markdown must be UTF-8"))?;
            let hash = revision(&bytes);
            let modified = file
                .metadata()?
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            changed |= index_markdown(&tx,id,&file.rel,&hash,markdown,modified,&scan)?;
            Ok(())
        })?;
        changed |= tx.execute(
            "DELETE FROM documents WHERE source_id=?1 AND scan<>?2",
            params![id, scan],
        )? > 0;
        if changed { rebuild_resolved_links(&tx, id, cancel)?; }
        cancel.check()?;
        root.verify()?;
        catalog::schema::publish(&tx,id)?;
        tx.commit()?;
        Ok(count)
    }
    pub fn search(&self, query: &str, sources: &[String], limit: usize) -> Result<Vec<Evidence>> {
        self.search_ordered(query,sources,limit,false)
    }
    pub fn search_recent(&self, query: &str, sources: &[String], limit: usize) -> Result<Vec<Evidence>> {
        self.search_ordered(query,sources,limit,true)
    }
    fn search_ordered(&self, query: &str, sources: &[String], limit: usize, recent:bool) -> Result<Vec<Evidence>> {
        let ids = self.scope(sources)?;
        if !(1..=100).contains(&limit) {
            return Err(invalid("invalid search bounds"));
        }
        let parsed = crate::query::Query::parse(query)?;
        let (filters, mut values) = self.query_filters(&parsed, &ids)?;
        let has_terms = !parsed.terms.is_empty();
        let (from, score, order, predicate) = if has_terms {
            values.insert(0, parsed.fts().into());
            ("chunks_fts JOIN chunks c ON c.id=chunks_fts.rowid", "-bm25(chunks_fts)", "bm25(chunks_fts),d.path,c.id", "chunks_fts MATCH ? AND ")
        } else { ("chunks c", "0.0", "d.modified_ms DESC,d.path,c.id", "") };
        let order=if recent {"d.modified_ms DESC,d.path,c.id"} else {order};
        let sql=format!("SELECT d.source_id,d.path,d.revision,d.title,c.heading,c.line_start,c.line_end,c.text,{score},d.modified_ms FROM {from} JOIN available_documents d ON d.id=c.document_id WHERE {predicate}{filters} ORDER BY {order} LIMIT ?");
        values.push((limit as i64).into());
        let mut hits: Vec<Evidence> = self
            .index
            .prepare(&sql)?
            .query_map(params_from_iter(values), evidence_row)?
            .collect::<std::result::Result<_, _>>()?;
        for (rank, hit) in hits.iter_mut().enumerate() { hit.lexical_rank=Some(rank as u32+1); }
        self.scope(&ids)?;
        Ok(hits)
    }

    fn query_filters(&self, query: &crate::query::Query, ids: &[String]) -> Result<(String,Vec<rusqlite::types::Value>)> {
        let mut predicate=format!("d.source_id IN ({})",vec!["?";ids.len()].join(","));
        let mut values:Vec<rusqlite::types::Value>=ids.iter().cloned().map(Into::into).collect();
        for tag in &query.tags {
            predicate.push_str(" AND EXISTS(SELECT 1 FROM document_tags t WHERE t.document_id=d.id AND t.tag=?)");
            values.push(tag.clone().into());
        }
        for path in &query.paths {
            predicate.push_str(" AND d.path GLOB ?");
            // An unadorned directory/path fragment keeps the existing palette's
            // substring semantics; explicit glob patterns stay patterns.
            values.push(if path.contains(['*','?','[']) {path.clone()} else {format!("*{path}*")}.into());
        }
        for after in &query.after { predicate.push_str(" AND d.modified_ms > ?"); values.push((*after).into()); }
        for target in &query.linked {
            let mut targets = Vec::new();
            for source in ids {
                targets.extend(resolve_indexed(&self.index,source,"__query__.md",target)?.into_iter().map(|(id,_)|id));
                if targets.len() > MAX_SOURCE_LINKS { return Err(invalid("link query target budget exceeded")); }
            }
            // Start at the indexed incoming edge set, rather than scanning all
            // source links (or checking every document with a correlated scan).
            predicate.push_str(" AND d.id IN(SELECT r.origin_id FROM resolved_links r WHERE r.target_id IN(SELECT value FROM json_each(?)))");
            values.push(serde_json::to_string(&targets).map_err(|_|invalid("invalid link query"))?.into());
        }
        Ok((predicate,values))
    }

    /// Opaque versioned capture policy, owned by the single native service.
    pub fn save_capture_policy(&self, project:&str, configuration:&str)->Result<()> {
        if project.trim().is_empty() || project.len()>256 || configuration.len()>128*1024 {return Err(invalid("invalid capture policy"));}
        if self.capture_policies()?.len()>=128 && !self.capture_policies()?.iter().any(|(id,_)|id==project) {return Err(invalid("capture project limit reached"));}
        self.library.execute("INSERT INTO capture_policies VALUES(?1,?2) ON CONFLICT(project_id) DO UPDATE SET configuration=excluded.configuration",params![project,configuration])?;
        Ok(())
    }
    pub fn capture_policies(&self)->Result<Vec<(String,String)>> {
        Ok(self.library.prepare("SELECT project_id,configuration FROM capture_policies ORDER BY project_id")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<std::result::Result<_,_>>()?)
    }
    /// Ordinary Markdown is durable before admission. Admission and the index
    /// job commit together; a hook receipt may be issued only after this returns.
    pub fn commit_capture(&self,id:&str,source:&str,path:&str,provenance:&str)->Result<bool> {
        if id.len()!=64 || !id.bytes().all(|b|b.is_ascii_hexdigit()) || provenance.len()>128*1024 {return Err(invalid("invalid capture admission"));}
        let document=self.read_document(source,path,None)?;
        let tx=self.library.unchecked_transaction()?;
        let existing:Option<(String,String,String)>=tx.query_row("SELECT source_id,path,provenance FROM captures WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some(existing)=existing {
            if existing!=(source.into(),path.into(),provenance.into()) {return Err(ReaderError::Conflict);}
            tx.commit()?;
            return Ok(false);
        }
        tx.execute("INSERT INTO captures VALUES(?1,?2,?3,?4,?5)",params![id,source,path,document.revision,provenance])?;
        tx.execute("INSERT INTO index_jobs(source_id,phase) VALUES(?1,'queued') ON CONFLICT(source_id) DO UPDATE SET phase='queued',error=NULL,generation=index_jobs.generation+1",[source])?;
        tx.commit()?;
        Ok(true)
    }
    pub fn capture_count(&self)->Result<u64> {Ok(self.library.query_row("SELECT count(*) FROM captures",[],|r|r.get(0))?)}

    pub fn queue_index(&self, source: &str) -> Result<()> {
        self.scope(&[source.into()])?;
        self.library.execute("INSERT INTO index_jobs(source_id,phase) VALUES(?1,'queued') ON CONFLICT(source_id) DO UPDATE SET phase='queued',error=NULL,generation=index_jobs.generation+1",[source])?;
        Ok(())
    }
    pub fn pending_index_jobs(&self) -> Result<Vec<String>> {
        Ok(self.library.prepare("SELECT j.source_id FROM index_jobs j JOIN sources s ON s.id=j.source_id WHERE s.enabled=1 AND j.phase IN('queued','indexing','loading','embedding') ORDER BY j.source_id")?.query_map([],|r|r.get(0))?.collect::<std::result::Result<_,_>>()?)
    }
    pub fn record_index_job(&self, source: &str, phase: &str, error: Option<&str>) -> Result<()> {
        let generation=self.index_job_record(source)?.map_or(1,|job|job.generation);
        self.record_index_job_if_current(source,generation,phase,error)?;
        Ok(())
    }
    pub fn record_index_job_if_current(&self, source:&str,generation:u64,phase:&str,error:Option<&str>) -> Result<bool> {
        if !["queued","indexing","loading","embedding","done","model-unavailable","error"].contains(&phase) || error.is_some_and(|e|e.len()>4096) { return Err(invalid("invalid job status")); }
        if source.starts_with("provider:") && self.provider_enabled(source).is_err() {return Ok(false);}
        let status=self.index_status(&[source.into()]).ok();
        let changed=self.library.execute("INSERT INTO index_jobs(source_id,phase,documents,chunks,embedded,error,generation) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(source_id) DO UPDATE SET phase=excluded.phase,documents=excluded.documents,chunks=excluded.chunks,embedded=excluded.embedded,error=excluded.error WHERE index_jobs.generation=excluded.generation",params![source,phase,status.as_ref().map_or(0,|s|s.documents),status.as_ref().map_or(0,|s|s.chunks),status.as_ref().map_or(0,|s|s.embedded),error,generation])?;
        if changed==1 && source.starts_with("provider:") && ["done","model-unavailable","error"].contains(&phase) {
            self.library.execute("UPDATE core_provider_jobs SET state=?2 WHERE reader_source_id=?1 AND state='queued' AND EXISTS(SELECT 1 FROM core_provider_records p WHERE p.reader_source_id=core_provider_jobs.reader_source_id AND p.record_key=core_provider_jobs.record_key AND p.revision=core_provider_jobs.revision)",params![source,phase])?;
        }
        Ok(changed==1)
    }
    pub fn index_job(&self, source: &str) -> Result<Option<IndexJob>> {
        self.scope(&[source.into()])?;
        self.index_job_record(source)
    }
    /// Service recovery metadata; reading this does not grant document access.
    pub fn index_job_record(&self, source:&str) -> Result<Option<IndexJob>> {
        Ok(self.library.query_row("SELECT source_id,phase,documents,chunks,embedded,error,generation FROM index_jobs WHERE source_id=?1",[source],|r|Ok(IndexJob {source_id:r.get(0)?,phase:r.get(1)?,documents:r.get(2)?,chunks:r.get(3)?,embedded:r.get(4)?,error:r.get(5)?,generation:r.get(6)?})).optional()?)
    }
    pub fn pending_embedding_documents(&self, source: &str) -> Result<Vec<String>> {
        self.scope(&[source.into()])?;
        Ok(self.index.prepare("SELECT d.path FROM available_documents d WHERE d.source_id=?1 AND EXISTS(SELECT 1 FROM chunks c LEFT JOIN embedding_memberships m ON m.chunk_id=c.id WHERE c.document_id=d.id AND m.chunk_id IS NULL) ORDER BY d.path")?.query_map([source],|r|r.get(0))?.collect::<std::result::Result<_,_>>()?)
    }
    pub fn list_documents(&self,source:&str)->Result<Vec<ListedDocument>> {
        self.scope(&[source.into()])?;
        let documents:Vec<_>=self.index.prepare("SELECT path,revision,modified_ms FROM available_documents WHERE source_id=?1 ORDER BY path LIMIT 50001")?.query_map([source],|r|Ok(ListedDocument {path:r.get(0)?,revision:r.get(1)?,modified_ms:r.get(2)?}))?.collect::<std::result::Result<_,_>>()?;
        if documents.len()>50000 {return Err(invalid("document listing exceeds budget"));}
        self.scope(&[source.into()])?;
        Ok(documents)
    }
    pub fn read_document(
        &self,
        source: &str,
        path: &str,
        expected: Option<&str>,
    ) -> Result<Document> {
        checked_path(path)?;
        if source.starts_with("provider:") {return self.provider_document(source,path,expected);}
        let root = self.stored_root(source, true)?;
        use std::io::Read;
        let mut bytes = Vec::new();
        root.open_file_relative(Path::new(path))?
            .take(MAX_DOCUMENT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err(invalid("document exceeds size limit"));
        }
        let revision = revision(&bytes);
        if expected.is_some_and(|r| r != revision) {
            return Err(ReaderError::Conflict);
        }
        let markdown = String::from_utf8(bytes).map_err(|_| invalid("Markdown must be UTF-8"))?;
        root.verify()?;
        Ok(Document {
            source_id: source.into(),
            path: path.into(),
            revision,
            markdown,
        })
    }
    pub fn index_status(&self, sources: &[String]) -> Result<IndexStatus> {
        let ids = self.scope(sources)?;
        let mut status = IndexStatus {
            documents: 0,
            chunks: 0,
            embedded: 0,
            model: MODEL_NAME.into(),
        };
        for id in ids {
            let (docs,chunks,embedded):(u64,u64,u64)=self.index.query_row("SELECT (SELECT count(*) FROM available_documents WHERE source_id=?1),(SELECT count(*) FROM chunks c JOIN available_documents d ON d.id=c.document_id WHERE d.source_id=?1),(SELECT count(*) FROM embedding_memberships WHERE source_id=?1)",[&id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let embedded=if id.starts_with("provider:") {self.index.query_row("SELECT count(*) FROM embedding_memberships m JOIN chunks c ON c.id=m.chunk_id JOIN available_documents d ON d.id=c.document_id WHERE d.source_id=?1",[&id],|r|r.get(0))?}else {embedded};
            status.documents += docs;
            status.chunks += chunks;
            status.embedded += embedded;
        }
        Ok(status)
    }
    pub fn set_bookmark(&self, source: &str, path: &str, line: u32) -> Result<()> {
        self.scope(&[source.into()])?;
        checked_path(path)?;
        if line == 0 {
            return Err(invalid("lines start at one"));
        }
        self.library.execute("INSERT INTO bookmarks VALUES(?1,?2,?3) ON CONFLICT(source_id,path) DO UPDATE SET line=excluded.line",params![source,path,line])?;
        Ok(())
    }
    pub fn bookmark(&self, source: &str, path: &str) -> Result<Option<u32>> {
        self.scope(&[source.into()])?;
        checked_path(path)?;
        Ok(self
            .library
            .query_row(
                "SELECT line FROM bookmarks WHERE source_id=?1 AND path=?2",
                params![source, path],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn rebuild_index(&mut self) -> Result<()> {
        // Per-source transactions preserve previous evidence if a scan fails.
        // Only derived rows change; bookmarks and captures never reset.
        for source in self.list_sources()?.into_iter().filter(|s| s.enabled) {
            self.index_source(&source.id)?;
        }
        Ok(())
    }
    pub fn embedding_inputs(&self, source: &str, path: &str) -> Result<(String, Vec<String>)> {
        self.stored_root(source, true)?.verify()?;
        let (id, revision): (i64, String) = self.index.query_row(
            "SELECT id,revision FROM available_documents WHERE source_id=?1 AND path=?2",
            params![source, path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let inputs = self
            .index
            .prepare("SELECT embedding_text FROM chunks WHERE document_id=?1 ORDER BY id")?
            .query_map([id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok((revision, inputs))
    }
    /// Slow inference must install against its original indexed revision.
    pub fn install_embeddings(
        &mut self,
        source: &str,
        path: &str,
        expected: &str,
        vectors: &[Vec<f32>],
    ) -> Result<()> {
        self.stored_root(source, true)?.verify()?;
        if source.starts_with("provider:") {self.provider_document(source,path,Some(expected))?;}
        let tx = self.index.transaction()?;
        let document: Option<(i64, String)> = tx
            .query_row(
                "SELECT id,revision FROM available_documents WHERE source_id=?1 AND path=?2",
                params![source, path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (document, revision) = document.ok_or(ReaderError::Conflict)?;
        if revision != expected {
            return Err(ReaderError::Conflict);
        }
        let ids: Vec<i64> = tx
            .prepare("SELECT id FROM chunks WHERE document_id=?1 ORDER BY id")?
            .query_map([document], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        if ids.len() != vectors.len() {
            return Err(invalid("embedding count mismatch"));
        }
        for (id, vector) in ids.into_iter().zip(vectors) {
            let bytes = vector_bytes(vector)?;
            tx.execute("DELETE FROM vectors WHERE rowid=?1", [id])?;
            tx.execute(
                "INSERT INTO vectors(rowid,embedding) VALUES(?1,?2)",
                params![id, bytes],
            )?;
            tx.execute("INSERT INTO embedding_memberships(chunk_id,source_id) VALUES(?1,?2) ON CONFLICT(chunk_id) DO UPDATE SET source_id=excluded.source_id", params![id,source])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn semantic_search(
        &self,
        vector: &[f32],
        sources: &[String],
        limit: usize,
    ) -> Result<Vec<Evidence>> {
        self.semantic_search_filtered(None,vector,sources,limit)
    }
    pub fn semantic_search_query(&self, query:&str, vector:&[f32], sources:&[String], limit:usize) -> Result<Vec<Evidence>> {
        self.semantic_search_filtered(Some(query),vector,sources,limit)
    }
    pub fn semantic_query_text(query:&str) -> Result<String> {
        Ok(crate::query::Query::parse(query)?.terms.join(" "))
    }
    fn semantic_search_filtered(&self, query:Option<&str>, vector:&[f32], sources:&[String], limit:usize) -> Result<Vec<Evidence>> {
        let ids = self.scope(sources)?;
        if !(1..=100).contains(&limit) {
            return Err(invalid("invalid search bounds"));
        }
        // Scope before LIMIT, never filter a global top-k after the query.
        let parsed=query.map(crate::query::Query::parse).transpose()?.unwrap_or_default();
        let (filters,filter_values)=self.query_filters(&parsed,&ids)?;
        let sql=format!("SELECT d.source_id,d.path,d.revision,d.title,c.heading,c.line_start,c.line_end,c.text,1.0-vec_distance_cosine(v.embedding,?) AS score,d.modified_ms FROM vectors v JOIN chunks c ON c.id=v.rowid JOIN available_documents d ON d.id=c.document_id WHERE {filters} ORDER BY score DESC,d.path,c.id LIMIT ?");
        let mut values: Vec<rusqlite::types::Value> = vec![vector_bytes(vector)?.into()];
        values.extend(filter_values);
        values.push((limit as i64).into());
        let mut hits:Vec<Evidence> = self
            .index
            .prepare(&sql)?
            .query_map(params_from_iter(values), evidence_row)?
            .collect::<std::result::Result<_, _>>()?;
        for (rank,hit) in hits.iter_mut().enumerate() { hit.semantic_rank=Some(rank as u32+1); }
        self.scope(&ids)?;
        Ok(hits)
    }
    pub fn hybrid_search(
        &self,
        query: &str,
        vector: &[f32],
        sources: &[String],
        limit: usize,
    ) -> Result<Vec<Evidence>> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("invalid search bounds"));
        }
        let mut merged: HashMap<(String, String, String, u32), Evidence> = HashMap::new();
        for arm in [
            self.search(query, sources, 100)?,
            self.semantic_search_query(query, vector, sources, 100)?,
        ] {
            for (rank, mut hit) in arm.into_iter().enumerate() {
                let key = (
                    hit.source_id.clone(),
                    hit.path.clone(),
                    hit.revision.clone(),
                    hit.line_start,
                );
                let score = 1.0 / (61.0 + rank as f64);
                if let Some(previous) = merged.get_mut(&key) {
                    previous.score += score;
                    if hit.lexical_rank.is_some() { previous.lexical_rank=hit.lexical_rank; }
                    if hit.semantic_rank.is_some() { previous.semantic_rank=hit.semantic_rank; }
                } else {
                    hit.score = score;
                    merged.insert(key, hit);
                }
            }
        }
        let mut hits: Vec<_> = merged.into_values().collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.path.cmp(&b.path))
                .then(a.line_start.cmp(&b.line_start))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}
fn evidence_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Evidence> {
    Ok(Evidence {
        source_id: r.get(0)?,
        path: r.get(1)?,
        revision: r.get(2)?,
        title: r.get(3)?,
        heading: r.get(4)?,
        line_start: r.get(5)?,
        line_end: r.get(6)?,
        text: r.get(7)?,
        score: r.get(8)?,
        modified_ms: r.get(9)?,
        lexical_rank: None,
        semantic_rank: None,
    })
}

fn heading_slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == ' ')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}
fn normalize_target(from: &str, target: &str) -> Result<String> {
    let joined = if target.starts_with('/') {
        target.trim_start_matches('/').to_string()
    } else {
        format!(
            "{}/{}",
            from.rsplit_once('/').map(|x| x.0).unwrap_or(""),
            target
        )
        .trim_start_matches('/')
        .to_string()
    };
    let mut components = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(invalid("reference escapes source"));
                }
            }
            x => components.push(x),
        }
    }
    Ok(components.join("/"))
}
fn add_document_aliases(
    connection: &Connection,
    document: i64,
    source: &str,
    path: &str,
    title: &str,
) -> Result<()> {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut insert = connection.prepare_cached("INSERT INTO document_aliases VALUES(?1,?2,?3,?4)")?;
    for (kind, value) in [("stem", stem), ("title", title)] {
        insert.execute(params![source, kind, value.to_ascii_lowercase(), document])?;
    }
    Ok(())
}

/// Resolve only through the index. The caller owns filesystem authority checks
/// (public reads), or an atomic index transaction with a verified root (rebuild).
fn resolve_indexed(
    connection: &Connection,
    source: &str,
    from: &str,
    target: &str,
) -> Result<Vec<(i64, Reference)>> {
    checked_path(from)?;
    if target.len() > 4096 || target.contains('\\') || target.contains(':') {
        return Err(invalid("unsupported document reference"));
    }
    let (target, heading) = target
        .split_once('#')
        .map(|(t, h)| (t, Some(h)))
        .unwrap_or((target, None));
    let decoded = percent_encoding::percent_decode_str(target)
        .decode_utf8()
        .map_err(|_| invalid("invalid reference encoding"))?;
    let heading = heading
        .map(|h| {
            percent_encoding::percent_decode_str(h)
                .decode_utf8()
                .map(|h| h.into_owned())
                .map_err(|_| invalid("invalid heading encoding"))
        })
        .transpose()?;
    let resolved = if decoded.is_empty() {
        from.into()
    } else {
        normalize_target(from, &decoded)?
    };
    let mut exact = connection.prepare_cached("SELECT id,path,revision FROM available_documents WHERE source_id=?1 AND path IN(?2,?3) ORDER BY path")?;
    let mut matches: Vec<(i64, String, String)> = exact
        .query_map(params![source, resolved, format!("{resolved}.md")], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    // Exact relative paths take precedence even if their requested heading does
    // not exist. ASCII folding deliberately preserves the previous semantics.
    if matches.is_empty() && !decoded.contains('/') {
        let mut aliases = connection.prepare_cached("SELECT d.id,d.path,d.revision FROM available_documents d WHERE d.id IN(SELECT a.document_id FROM document_aliases a WHERE a.source_id=?1 AND a.kind='stem' AND a.alias=?2 UNION SELECT a.document_id FROM document_aliases a WHERE a.source_id=?1 AND a.kind='title' AND a.alias=?3) ORDER BY d.path LIMIT ?4")?;
        matches = aliases
            .query_map(
                params![
                    source,
                    decoded.trim_end_matches(".md").to_ascii_lowercase(),
                    decoded.to_ascii_lowercase(),
                    MAX_REFERENCE_MATCHES as i64 + 1
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?
            .collect::<std::result::Result<_, _>>()?;
        if matches.len() > MAX_REFERENCE_MATCHES {
            return Err(invalid(REFERENCE_AMBIGUITY_BUDGET));
        }
    }
    let mut results = Vec::new();
    let mut headings = connection.prepare_cached("SELECT line FROM headings WHERE document_id=?1 AND (slug=?2 OR text=?2 COLLATE NOCASE) ORDER BY line LIMIT 1")?;
    for (id, path, revision) in matches {
        let line = if let Some(heading) = &heading {
            headings
                .query_row(params![id, heading], |r| r.get(0))
                .optional()?
        } else {
            Some(1)
        };
        if let Some(line) = line {
            results.push((
                id,
                Reference {
                    source_id: source.into(),
                    path,
                    revision,
                    line,
                },
            ));
        }
    }
    Ok(results)
}

fn rebuild_resolved_links(
    connection: &Connection,
    source: &str,
    cancel: &Cancellation,
) -> Result<()> {
    // Recompute when any source document changes: new/deleted aliases or headings
    // can change an unchanged link's resolution. Never publish a partial graph.
    connection.execute("DELETE FROM resolved_links WHERE source_id=?1", [source])?;
    let mut links = connection.prepare("SELECT d.id,d.path,l.target,l.line FROM available_documents d JOIN links l ON l.document_id=d.id WHERE d.source_id=?1 AND l.kind<>'image' LIMIT ?2")?;
    let mut rows = links.query(params![source, MAX_SOURCE_LINKS as i64 + 1])?;
    let mut count = 0;
    let mut edges = 0;
    let mut insert =
        connection.prepare_cached("INSERT OR IGNORE INTO resolved_links VALUES(?1,?2,?3,?4,?5)")?;
    while let Some(row) = rows.next()? {
        cancel.check()?;
        count += 1;
        if count > MAX_SOURCE_LINKS {
            return Err(invalid("source link materialization exceeds budget"));
        }
        let origin: i64 = row.get(0)?;
        let from: String = row.get(1)?;
        let target: String = row.get(2)?;
        let line: u32 = row.get(3)?;
        let resolved = match resolve_indexed(connection, source, &from, &target) {
            Ok(resolved) => resolved,
            // Unsupported external/escaping/invalid references never form edges.
            // Budget failures must remain visible and roll back the index pass.
            Err(ReaderError::Invalid(message)) if message != REFERENCE_AMBIGUITY_BUDGET => continue,
            Err(error) => return Err(error),
        };
        for (target, reference) in resolved {
            edges += insert.execute(params![source, origin, target, line, reference.line])?;
            if edges > MAX_RESOLVED_EDGES {
                return Err(invalid("resolved edge materialization exceeds budget"));
            }
        }
    }
    Ok(())
}

impl ReaderStore {
    /// Directed two-hop graph, explicit shared tags and measured stored vectors.
    /// The existing `related` API remains the one-hop incoming/outgoing link set.
    pub fn related_ranked(&self, source: &str, path: &str, limit: usize) -> Result<RelatedRanked> {
        self.related_ranked_cancelled(source, path, None, limit, &Cancellation::default())
    }
    /// Apply the same tag/path/after/linked-to and literal-term filters as search
    /// before the final result limit. Filters do not add lexical ranking scores.
    pub fn related_ranked_query(
        &self,
        source: &str,
        path: &str,
        query: &str,
        limit: usize,
    ) -> Result<RelatedRanked> {
        self.related_ranked_cancelled(source, path, Some(query), limit, &Cancellation::default())
    }
    pub fn related_ranked_cancelled(
        &self,
        source: &str,
        path: &str,
        query: Option<&str>,
        limit: usize,
        cancel: &Cancellation,
    ) -> Result<RelatedRanked> {
        cancel.check()?;
        self.stored_root(source, true)?.verify()?;
        checked_path(path)?;
        if !(1..=100).contains(&limit) {
            return Err(invalid("invalid related bounds"));
        }
        // A service reader can run beside the owner indexer. Every component and
        // returned revision must describe one consistent index snapshot.
        let tx = self.index.unchecked_transaction()?;
        let parsed = query
            .map(crate::query::Query::parse)
            .transpose()?
            .unwrap_or_default();
        let (mut filters, mut values) = self.query_filters(&parsed, &[source.to_string()])?;
        if !parsed.terms.is_empty() {
            filters.push_str(" AND EXISTS(SELECT 1 FROM chunks_fts JOIN chunks fc ON fc.id=chunks_fts.rowid WHERE fc.document_id=d.id AND chunks_fts MATCH ?)");
            values.push(parsed.fts().into());
        }
        let outcome = related::rank(&tx, source, path, &filters, &values, limit, cancel)?;
        cancel.check()?;
        self.stored_root(source, true)?.verify()?;
        tx.commit()?;
        Ok(outcome)
    }
    pub fn resolve_reference(
        &self,
        source: &str,
        from: &str,
        target: &str,
    ) -> Result<Vec<Reference>> {
        self.stored_root(source, true)?.verify()?;
        let results = resolve_indexed(&self.index, source, from, target)?
            .into_iter()
            .map(|(_, reference)| reference)
            .collect();
        self.stored_root(source, true)?.verify()?;
        Ok(results)
    }
    pub fn related(&self, source: &str, path: &str, limit: usize) -> Result<Vec<Reference>> {
        self.stored_root(source, true)?.verify()?;
        checked_path(path)?;
        if !(1..=100).contains(&limit) {
            return Err(invalid("invalid related bounds"));
        }
        // A reciprocal neighbor can appear as both incoming and outgoing. The
        // origin-path/line ordering preserves the previous deterministic last
        // link selection, including heading evidence for outgoing references.
        let sql = "WITH current AS (SELECT id FROM available_documents WHERE source_id=?1 AND path=?2), neighbors AS (
          SELECT t.path,t.revision,r.target_line AS line,o.path AS origin,r.origin_line
          FROM current c JOIN resolved_links r ON r.source_id=?1 AND r.origin_id=c.id
          JOIN available_documents t ON t.id=r.target_id JOIN available_documents o ON o.id=r.origin_id WHERE t.id<>c.id
          UNION ALL
          SELECT o.path,o.revision,1,o.path,r.origin_line
          FROM current c JOIN resolved_links r ON r.source_id=?1 AND r.target_id=c.id
          JOIN available_documents o ON o.id=r.origin_id WHERE o.id<>c.id
        ), ranked AS (SELECT path,revision,line,ROW_NUMBER() OVER(PARTITION BY path ORDER BY origin DESC,origin_line DESC,line DESC) AS n FROM neighbors)
        SELECT path,revision,line FROM ranked WHERE n=1 ORDER BY path LIMIT ?3";
        let results = self
            .index
            .prepare(sql)?
            .query_map(params![source, path, limit as i64], |r| {
                Ok(Reference {
                    source_id: source.into(),
                    path: r.get(0)?,
                    revision: r.get(1)?,
                    line: r.get(2)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        self.stored_root(source, true)?.verify()?;
        Ok(results)
    }
}
