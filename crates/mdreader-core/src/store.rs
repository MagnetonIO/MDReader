use crate::{parse_markdown, scan_source_root, ScanOptions, SourceRoot, SourceRootIdentity};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, UNIX_EPOCH};

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
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub source_id: String,
    pub path: String,
    pub revision: String,
    pub markdown: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatus {
    pub documents: u64,
    pub chunks: u64,
    pub embedded: u64,
    pub model: String,
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

fn open_database(path: &Path, schema: &str, index: bool) -> Result<Connection> {
    let connection = Connection::open(path)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > if index { 2 } else { 1 } {
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
    if index && version < 2 {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(LINK_SCHEMA)?;
        tx.commit()?;
    }
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
impl ReaderStore {
    pub fn open(directory: &Path) -> Result<Self> {
        register_vectors()?;
        std::fs::create_dir_all(directory)?;
        Ok(Self {
            library: open_database(&directory.join("library.sqlite"), LIBRARY_SCHEMA, false)?,
            index: open_database(&directory.join("index.sqlite"), INDEX_SCHEMA, true)?,
        })
    }
    pub fn add_local_source(&mut self, name: &str, root: SourceRoot) -> Result<Source> {
        if name.trim().is_empty() || name.len() > 256 {
            return Err(invalid("invalid source name"));
        }
        root.verify()?;
        let identity = serde_json::to_string(&root.identity)
            .map_err(|_| invalid("invalid source identity"))?;
        let id = revision(identity.as_bytes());
        self.library.execute("INSERT INTO sources VALUES(?1,?2,?3,?4,1) ON CONFLICT(id) DO UPDATE SET name=excluded.name,path=excluded.path,enabled=1",params![id,name,encode_path(&root.path),identity])?;
        Ok(Source {
            id,
            name: name.into(),
            enabled: true,
        })
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
        if enabled {
            self.stored_root(id, false)?.verify()?;
        }
        if self.library.execute(
            "UPDATE sources SET enabled=?2 WHERE id=?1",
            params![id, enabled],
        )? != 1
        {
            return Err(ReaderError::Unavailable);
        }
        Ok(())
    }
    pub fn index_source(&mut self, id: &str) -> Result<usize> {
        self.index_source_cancelled(id, &Cancellation::default())
    }
    pub fn index_source_cancelled(&mut self, id: &str, cancel: &Cancellation) -> Result<usize> {
        cancel.check()?;
        let root = self.stored_root(id, true)?;
        let files = scan_source_root(
            &root,
            &ScanOptions {
                include_mdx: false,
                max_file_size: MAX_DOCUMENT_BYTES,
                respect_gitignore: true,
            },
        )?;
        let scan = uuid::Uuid::new_v4().to_string();
        let tx = self.index.transaction()?;
        for file in &files {
            cancel.check()?;
            let bytes = file.read_bounded(MAX_DOCUMENT_BYTES)?;
            if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
                return Err(invalid("document grew beyond size limit"));
            }
            let markdown =
                std::str::from_utf8(&bytes).map_err(|_| invalid("Markdown must be UTF-8"))?;
            let hash = revision(&bytes);
            let known: Option<(i64, String)> = tx
                .query_row(
                    "SELECT id,revision FROM documents WHERE source_id=?1 AND path=?2",
                    params![id, file.rel],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((document, old)) = &known {
                if old == &hash {
                    tx.execute(
                        "UPDATE documents SET scan=?2 WHERE id=?1",
                        params![document, scan],
                    )?;
                    continue;
                }
                tx.execute("DELETE FROM documents WHERE id=?1", [document])?;
            }
            let parsed = parse_markdown(markdown, &file.rel);
            let modified = file
                .metadata()?
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            tx.execute("INSERT INTO documents(source_id,path,revision,title,modified_ms,scan) VALUES(?1,?2,?3,?4,?5,?6)",params![id,file.rel,hash,parsed.title,modified,scan])?;
            let document = tx.last_insert_rowid();
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
        }
        tx.execute(
            "DELETE FROM documents WHERE source_id=?1 AND scan<>?2",
            params![id, scan],
        )?;
        cancel.check()?;
        root.verify()?;
        tx.commit()?;
        Ok(files.len())
    }
    pub fn search(&self, query: &str, sources: &[String], limit: usize) -> Result<Vec<Evidence>> {
        let ids = self.scope(sources)?;
        if query.trim().is_empty() || query.len() > 1024 || !(1..=100).contains(&limit) {
            return Err(invalid("invalid search bounds"));
        }
        let literal = query
            .split_whitespace()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql=format!("SELECT d.source_id,d.path,d.revision,d.title,c.heading,c.line_start,c.line_end,c.text,-bm25(chunks_fts) FROM chunks_fts JOIN chunks c ON c.id=chunks_fts.rowid JOIN documents d ON d.id=c.document_id WHERE chunks_fts MATCH ? AND d.source_id IN ({}) ORDER BY bm25(chunks_fts),d.path,c.id LIMIT ?",vec!["?";ids.len()].join(","));
        let mut values: Vec<rusqlite::types::Value> = vec![literal.into()];
        values.extend(ids.iter().cloned().map(Into::into));
        values.push((limit as i64).into());
        let hits = self
            .index
            .prepare(&sql)?
            .query_map(params_from_iter(values), evidence_row)?
            .collect::<std::result::Result<_, _>>()?;
        self.scope(&ids)?;
        Ok(hits)
    }
    pub fn read_document(
        &self,
        source: &str,
        path: &str,
        expected: Option<&str>,
    ) -> Result<Document> {
        checked_path(path)?;
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
            let (docs,chunks,embedded):(u64,u64,u64)=self.index.query_row("SELECT (SELECT count(*) FROM documents WHERE source_id=?1),(SELECT count(*) FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.source_id=?1),(SELECT count(*) FROM vectors v JOIN chunks c ON c.id=v.rowid JOIN documents d ON d.id=c.document_id WHERE d.source_id=?1)",[id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            status.documents += docs;
            status.chunks += chunks;
            status.embedded += embedded;
        }
        Ok(status)
    }
    pub fn set_bookmark(&self, source: &str, path: &str, line: u32) -> Result<()> {
        checked_path(path)?;
        if line == 0 {
            return Err(invalid("lines start at one"));
        }
        self.library.execute("INSERT INTO bookmarks VALUES(?1,?2,?3) ON CONFLICT(source_id,path) DO UPDATE SET line=excluded.line",params![source,path,line])?;
        Ok(())
    }
    pub fn bookmark(&self, source: &str, path: &str) -> Result<Option<u32>> {
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
            "SELECT id,revision FROM documents WHERE source_id=?1 AND path=?2",
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
        let tx = self.index.transaction()?;
        let document: Option<(i64, String)> = tx
            .query_row(
                "SELECT id,revision FROM documents WHERE source_id=?1 AND path=?2",
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
        let ids = self.scope(sources)?;
        if !(1..=100).contains(&limit) {
            return Err(invalid("invalid search bounds"));
        }
        // Scope before LIMIT, never filter a global top-k after the query.
        let sql=format!("SELECT d.source_id,d.path,d.revision,d.title,c.heading,c.line_start,c.line_end,c.text,1.0-vec_distance_cosine(v.embedding,?) AS score FROM vectors v JOIN chunks c ON c.id=v.rowid JOIN documents d ON d.id=c.document_id WHERE d.source_id IN ({}) ORDER BY score DESC,d.path,c.id LIMIT ?",vec!["?";ids.len()].join(","));
        let mut values: Vec<rusqlite::types::Value> = vec![vector_bytes(vector)?.into()];
        values.extend(ids.iter().cloned().map(Into::into));
        values.push((limit as i64).into());
        let hits = self
            .index
            .prepare(&sql)?
            .query_map(params_from_iter(values), evidence_row)?
            .collect::<std::result::Result<_, _>>()?;
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
            self.semantic_search(vector, sources, 100)?,
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
impl ReaderStore {
    pub fn resolve_reference(
        &self,
        source: &str,
        from: &str,
        target: &str,
    ) -> Result<Vec<Reference>> {
        self.stored_root(source, true)?.verify()?;
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
                    .map(|v| v.into_owned())
                    .map_err(|_| invalid("invalid heading encoding"))
            })
            .transpose()?;
        let resolved = if decoded.is_empty() {
            from.into()
        } else {
            normalize_target(from, &decoded)?
        };
        let all: Vec<(i64, String, String, String)> = self
            .index
            .prepare(
                "SELECT id,path,revision,title FROM documents WHERE source_id=?1 ORDER BY path",
            )?
            .query_map([source], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let path_matches = |path: &str| path == resolved || path == format!("{resolved}.md");
        let exact = all.iter().any(|(_, p, _, _)| path_matches(p));
        let mut results = Vec::new();
        for (id, path, revision, title) in all {
            let stem = Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let matches = if exact {
                path_matches(&path)
            } else {
                !decoded.contains('/')
                    && (stem.eq_ignore_ascii_case(decoded.trim_end_matches(".md"))
                        || title.eq_ignore_ascii_case(&decoded))
            };
            if !matches {
                continue;
            }
            let line = if let Some(heading) = &heading {
                self.index.query_row("SELECT line FROM headings WHERE document_id=?1 AND (slug=?2 OR text=?2 COLLATE NOCASE) ORDER BY line LIMIT 1",params![id,heading],|r| r.get(0)).optional()?
            } else {
                Some(1)
            };
            if let Some(line) = line {
                results.push(Reference {
                    source_id: source.into(),
                    path,
                    revision,
                    line,
                });
            }
        }
        self.stored_root(source, true)?.verify()?;
        Ok(results)
    }
    pub fn related(&self, source: &str, path: &str, limit: usize) -> Result<Vec<Reference>> {
        self.stored_root(source, true)?.verify()?;
        checked_path(path)?;
        if !(1..=100).contains(&limit) {
            return Err(invalid("invalid related bounds"));
        }
        let links:Vec<(String,String)>=self.index.prepare("SELECT d.path,l.target FROM links l JOIN documents d ON d.id=l.document_id WHERE d.source_id=?1 AND l.kind<>'image' ORDER BY d.path,l.line LIMIT 100001")?.query_map([source],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<std::result::Result<_,_>>()?;
        if links.len() > 100000 {
            return Err(invalid("related traversal exceeds budget"));
        }
        let mut results = std::collections::BTreeMap::new();
        for (origin, target) in links {
            let resolved = match self.resolve_reference(source, &origin, &target) {
                Ok(r) => r,
                Err(ReaderError::Invalid(_)) => continue,
                Err(e) => return Err(e),
            };
            if origin == path {
                for reference in resolved {
                    if reference.path != path {
                        results.insert(reference.path.clone(), reference);
                    }
                }
            } else if resolved.iter().any(|r| r.path == path) {
                for reference in self.resolve_reference(source, &origin, "")? {
                    results.insert(reference.path.clone(), reference);
                }
            }
        }
        Ok(results.into_values().take(limit).collect())
    }
}
