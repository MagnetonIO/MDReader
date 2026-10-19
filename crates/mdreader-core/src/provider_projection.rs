//! Provider content is derived authority, never a local folder grant. All durable
//! gates and receipts share the service's borrowed library transaction. Orphaned
//! immutable cache files confer no authority and are never enumerated as records.
use super::*;
use mdreader_provider::{DocumentBody, LibraryId, Metadata, RecordKey, SourceBinding};
use rusqlite::Transaction;
#[cfg(not(windows))]
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
#[cfg(windows)]
#[path = "provider_projection/windows_cache.rs"]
mod windows_cache;

const SCHEMA: &str = "
CREATE TABLE core_provider_schema(version INTEGER NOT NULL);
INSERT INTO core_provider_schema VALUES(1);
CREATE TABLE core_provider_profile(singleton INTEGER PRIMARY KEY CHECK(singleton=1),library_id TEXT NOT NULL,
 cache_path BLOB NOT NULL,cache_identity TEXT NOT NULL);
CREATE TABLE core_provider_slots(library_id TEXT NOT NULL,service_source_id TEXT NOT NULL,
 binding_digest TEXT NOT NULL,enabled INTEGER NOT NULL CHECK(enabled IN(0,1)),PRIMARY KEY(library_id,service_source_id));
CREATE TABLE core_provider_sources(reader_source_id TEXT PRIMARY KEY REFERENCES sources(id),library_id TEXT NOT NULL,
 service_source_id TEXT NOT NULL,binding_digest TEXT NOT NULL UNIQUE);
CREATE TABLE core_provider_records(reader_source_id TEXT NOT NULL REFERENCES core_provider_sources(reader_source_id),
 record_key TEXT NOT NULL,path TEXT NOT NULL,revision TEXT NOT NULL,body_hash TEXT NOT NULL,cache_path TEXT NOT NULL,
 metadata_json TEXT NOT NULL,version_json TEXT NOT NULL,modified_ms INTEGER NOT NULL,
 PRIMARY KEY(reader_source_id,record_key),UNIQUE(reader_source_id,path));
CREATE TABLE core_provider_jobs(id TEXT PRIMARY KEY,reader_source_id TEXT NOT NULL REFERENCES core_provider_sources(reader_source_id),
 record_key TEXT NOT NULL,path TEXT NOT NULL,revision TEXT NOT NULL,state TEXT NOT NULL);
CREATE INDEX core_provider_jobs_record ON core_provider_jobs(reader_source_id,record_key,revision,state);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Local,
    ProviderDerived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProjectionReceipt {
    pub job_id: uuid::Uuid,
    pub reader_source_id: String,
    /// Safe generated reader path; the remote opaque key is never a filename.
    pub path: String,
    /// Exact provider version plus body hash; not an HTTP write validator.
    pub revision: String,
}

pub struct ProviderProjection {
    root: SourceRoot,
    library_id: LibraryId,
    #[cfg(windows)]
    cache: windows_cache::Cache,
}

pub(super) fn initialize(connection: &Connection) -> Result<()> {
    let exists: bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='core_provider_schema')",[],|r|r.get(0))?;
    if !exists {
        let tx = connection.unchecked_transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.commit()?;
    }
    check_schema(connection)
}
pub(super) fn check_schema(connection: &Connection) -> Result<()> {
    let versions: Vec<i64> = connection
        .prepare("SELECT version FROM core_provider_schema")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    if versions != [1] {
        return Err(invalid("provider authority requires owner migration"));
    }
    Ok(())
}

pub(super) fn attach_authority(index: &Connection, library_path: &Path) -> Result<()> {
    // URI mode=ro preserves WAL change detection; immutable=1 would be wrong.
    // Preserve Unix native bytes (including non-UTF8), escaping URI metacharacters.
    let path = std::fs::canonicalize(library_path)?;
    #[cfg(unix)]
    let raw = encode_path(&path);
    #[cfg(windows)]
    // SQLite decodes escaped URI path bytes before its Windows VFS opens them.
    // Preserve the canonical native namespace verbatim (including \\?\ and
    // backslashes); adding '/' and replacing separators breaks extended paths.
    // Encoding '?' also keeps the namespace marker out of the query string.
    let raw = path
        .to_str()
        .ok_or_else(|| invalid("invalid database path"))?
        .as_bytes()
        .to_vec();
    let encoded =
        percent_encoding::percent_encode(&raw, percent_encoding::NON_ALPHANUMERIC).to_string();
    index.execute(
        "ATTACH DATABASE ?1 AS reader_authority",
        [format!("file:{encoded}?mode=ro")],
    )?;
    crate::local_authority::sqlite_match::register(index)?;
    index.execute_batch("DROP VIEW IF EXISTS temp.available_documents;
CREATE TEMP VIEW available_documents AS SELECT d.* FROM main.documents d WHERE
 (d.source_id NOT LIKE 'provider:%' AND NOT EXISTS(SELECT 1 FROM reader_authority.reader_local_denied_v1 x WHERE x.source_id=d.source_id AND mdreader_private_prefix_v1(d.path,x.prefix))) OR EXISTS(
 SELECT 1 FROM reader_authority.core_provider_records p
 JOIN reader_authority.core_provider_sources s ON s.reader_source_id=p.reader_source_id
 JOIN reader_authority.core_provider_slots g ON g.library_id=s.library_id AND g.service_source_id=s.service_source_id AND g.binding_digest=s.binding_digest
 JOIN reader_authority.sources granted ON granted.id=s.reader_source_id
 WHERE p.reader_source_id=d.source_id AND p.path=d.path AND p.revision=d.revision AND g.enabled=1 AND granted.enabled=1);")?;
    Ok(())
}

impl ProviderProjection {
    /// The owner supplies a private cache directory and its durable profile ID.
    #[cfg(not(any(unix, windows)))]
    pub fn open(_cache_root: &Path, _expected_library_id: LibraryId) -> Result<Self> {
        Err(invalid(
            "provider projection requires private-cache platform support",
        ))
    }
    #[cfg(unix)]
    pub fn open(cache_root: &Path, expected_library_id: LibraryId) -> Result<Self> {
        if !cache_root.is_absolute() {
            return Err(invalid("expected absolute private cache path"));
        }
        if !cache_root.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new().mode(0o700).create(cache_root)?;
            }
            #[cfg(not(unix))]
            std::fs::create_dir(cache_root)?;
        }
        // Capture rejects symlink/reparse roots; do not canonicalize them first.
        let captured = SourceRoot::capture(cache_root.to_path_buf())?;
        let root = SourceRoot::restore(std::fs::canonicalize(cache_root)?, captured.identity)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if root
                .try_clone_directory_handle()?
                .metadata()?
                .permissions()
                .mode()
                & 0o077
                != 0
            {
                return Err(invalid("provider cache must be private"));
            }
        }
        root.verify()?;
        let parent = root
            .path
            .parent()
            .ok_or_else(|| invalid("invalid cache parent"))?;
        SourceRoot::capture(parent.to_path_buf())?
            .try_clone_directory_handle()?
            .sync_all()?;
        Ok(Self {
            root,
            library_id: expected_library_id,
        })
    }
    #[cfg(windows)]
    pub fn open(cache_root: &Path, expected_library_id: LibraryId) -> Result<Self> {
        let cache = windows_cache::Cache::open(cache_root, &expected_library_id)?;
        let captured = SourceRoot::capture(cache_root.to_path_buf())?;
        let root = SourceRoot::restore(std::fs::canonicalize(cache_root)?, captured.identity)?;
        root.verify()?;
        cache.verify()?;
        Ok(Self {
            root,
            library_id: expected_library_id,
            cache,
        })
    }
    fn validate_binding(&self, binding: &SourceBinding) -> Result<String> {
        if binding.library_id() != &self.library_id {
            return Err(ReaderError::Unavailable);
        }
        Ok(mdreader_provider::hex(&binding.digest()))
    }
    fn bind_profile(&self, tx: &Transaction<'_>) -> Result<()> {
        self.root.verify()?;
        let identity = serde_json::to_string(&self.root.identity)
            .map_err(|_| invalid("invalid cache identity"))?;
        let path = encode_path(&self.root.path);
        let existing:Option<(String,Vec<u8>,String)>=tx.query_row("SELECT library_id,cache_path,cache_identity FROM core_provider_profile WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some(existing) = existing {
            if existing != (self.library_id.as_str().into(), path, identity) {
                return Err(ReaderError::Unavailable);
            }
        } else {
            // Neither an existing local grant nor a later parent-folder grant
            // may expose private provider cache/history as ordinary Markdown.
            for bytes in tx
                .prepare("SELECT path FROM sources WHERE id NOT LIKE 'provider:%'")?
                .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            {
                let local = decode_path(bytes?)?;
                let local = std::fs::canonicalize(&local).unwrap_or(local);
                if local.starts_with(&self.root.path)
                    || (self.root.path.starts_with(&local)
                        && !crate::local_authority::private_paths(tx)?
                            .iter()
                            .any(|p| self.root.path.starts_with(p)))
                {
                    return Err(invalid("provider cache overlaps a local source"));
                }
            }
            tx.execute(
                "INSERT INTO core_provider_profile VALUES(1,?1,?2,?3)",
                params![self.library_id.as_str(), path, identity],
            )?;
        }
        Ok(())
    }
    fn current(&self, tx: &Transaction<'_>, binding: &SourceBinding) -> Result<String> {
        let digest = self.validate_binding(binding)?;
        let gate:Option<(String,bool)>=tx.query_row("SELECT binding_digest,enabled FROM core_provider_slots WHERE library_id=?1 AND service_source_id=?2",params![self.library_id.as_str(),binding.source_id().as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if gate != Some((digest.clone(), true)) {
            return Err(ReaderError::Unavailable);
        }
        Ok(format!("provider:{digest}"))
    }
    pub fn admit(
        &self,
        tx: &Transaction<'_>,
        binding: &SourceBinding,
        metadata: &Metadata,
        document: &DocumentBody,
    ) -> Result<ProviderProjectionReceipt> {
        let digest = self.validate_binding(binding)?;
        if metadata.key != document.key || metadata.version != document.version {
            return Err(ReaderError::Conflict);
        }
        if let Some(hash) = &document.version.content_hash {
            mdreader_provider::verify_content_hash(document.body.as_bytes(), hash)
                .map_err(|_| ReaderError::Conflict)?;
        }
        self.bind_profile(tx)?;
        let gate:Option<(String,bool)>=tx.query_row("SELECT binding_digest,enabled FROM core_provider_slots WHERE library_id=?1 AND service_source_id=?2",params![self.library_id.as_str(),binding.source_id().as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let source = format!("provider:{digest}");
        if let Some((old, enabled)) = gate {
            if old == digest && !enabled {
                return Err(ReaderError::Unavailable);
            }
            if old != digest {
                if enabled {
                    return Err(ReaderError::Conflict);
                }
                // Any prior generation is a permanent tombstone, even after a
                // newer generation has replaced the current slot.
                if tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM core_provider_sources WHERE reader_source_id=?1)",
                    [&source],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(ReaderError::Unavailable);
                }
            }
        }
        let path = format!("records/{}.md", revision(metadata.key.as_str().as_bytes()));
        let hash = revision(document.body.as_bytes());
        let version_json = serde_json::to_string(&document.version)
            .map_err(|_| invalid("invalid provider version"))?;
        let exact_revision = revision(
            &serde_json::to_vec(&(&document.version, &hash))
                .map_err(|_| invalid("invalid provider version"))?,
        );
        let cache_path = format!(
            "{digest}/{}/{exact_revision}.md",
            revision(metadata.key.as_str().as_bytes())
        );
        #[cfg(windows)]
        self.cache
            .publish(&cache_path, &hash, document.body.as_bytes())?;
        #[cfg(unix)]
        {
            let leaf = self
                .root
                .create_or_capture_directory_relative(Path::new(&digest))?;
            let key_dir = leaf.create_or_capture_directory_relative(Path::new(&revision(
                metadata.key.as_str().as_bytes(),
            )))?;
            let filename = format!("{exact_revision}.md");
            match key_dir.open_file_relative(Path::new(&filename)) {
                Ok(file) => {
                    let mut actual = Vec::new();
                    file.take(MAX_DOCUMENT_BYTES + 1).read_to_end(&mut actual)?;
                    if actual != document.body.as_bytes() {
                        return Err(ReaderError::Conflict);
                    }
                }
                Err(_) => {
                    let mut file = key_dir.create_new_relative(Path::new(&filename))?;
                    file.write_all(document.body.as_bytes())?;
                    file.sync_all()?;
                }
            }
            // Publish bytes and every newly-created directory before a durable DB receipt.
            key_dir.try_clone_directory_handle()?.sync_all()?;
            leaf.try_clone_directory_handle()?.sync_all()?;
            self.root.try_clone_directory_handle()?.sync_all()?;
        }
        #[cfg(not(any(unix, windows)))]
        return Err(ReaderError::Unavailable);
        self.root.verify()?;
        tx.execute("INSERT INTO core_provider_slots VALUES(?1,?2,?3,1) ON CONFLICT(library_id,service_source_id) DO UPDATE SET binding_digest=excluded.binding_digest,enabled=1",params![self.library_id.as_str(),binding.source_id().as_str(),digest])?;
        let identity = serde_json::to_string(&self.root.identity)
            .map_err(|_| invalid("invalid cache identity"))?;
        tx.execute("INSERT INTO sources VALUES(?1,?2,?3,?4,1) ON CONFLICT(id) DO UPDATE SET name=excluded.name",params![source,"Provider documents",encode_path(&self.root.path),identity])?;
        tx.execute(
            "INSERT OR IGNORE INTO core_provider_sources VALUES(?1,?2,?3,?4)",
            params![
                source,
                self.library_id.as_str(),
                binding.source_id().as_str(),
                digest
            ],
        )?;
        let metadata_json =
            serde_json::to_string(metadata).map_err(|_| invalid("invalid provider metadata"))?;
        let old_metadata:Option<String>=tx.query_row("SELECT metadata_json FROM core_provider_records WHERE reader_source_id=?1 AND record_key=?2",params![source,metadata.key.as_str()],|r|r.get(0)).optional()?;
        let metadata_changed = old_metadata.as_deref() != Some(&metadata_json);
        tx.execute("INSERT INTO core_provider_records VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(reader_source_id,record_key) DO UPDATE SET revision=excluded.revision,body_hash=excluded.body_hash,cache_path=excluded.cache_path,metadata_json=excluded.metadata_json,version_json=excluded.version_json,modified_ms=excluded.modified_ms",params![source,metadata.key.as_str(),path,exact_revision,hash,cache_path,metadata_json,version_json,metadata.modified_at_ms.unwrap_or(document.observed_at_ms)])?;
        tx.execute("UPDATE core_provider_jobs SET state='superseded' WHERE reader_source_id=?1 AND record_key=?2 AND revision<>?3 AND state='queued'",params![source,metadata.key.as_str(),exact_revision])?;
        let prior:Option<String>=tx.query_row("SELECT id FROM core_provider_jobs WHERE reader_source_id=?1 AND record_key=?2 AND revision=?3 AND state='queued' LIMIT 1",params![source,metadata.key.as_str(),exact_revision],|r|r.get(0)).optional()?;
        let job = if let Some(prior) = prior {
            if metadata_changed {
                queue(tx, &source)?;
            }
            uuid::Uuid::parse_str(&prior).map_err(|_| invalid("invalid provider job"))?
        } else {
            let job = uuid::Uuid::new_v4();
            tx.execute(
                "INSERT INTO core_provider_jobs VALUES(?1,?2,?3,?4,?5,'queued')",
                params![
                    job.to_string(),
                    source,
                    metadata.key.as_str(),
                    path,
                    exact_revision
                ],
            )?;
            queue(tx, &source)?;
            job
        };
        Ok(ProviderProjectionReceipt {
            job_id: job,
            reader_source_id: source,
            path,
            revision: exact_revision,
        })
    }
    pub fn remove(
        &self,
        tx: &Transaction<'_>,
        binding: &SourceBinding,
        key: &RecordKey,
    ) -> Result<()> {
        let source = self.current(tx, binding)?;
        tx.execute(
            "DELETE FROM core_provider_records WHERE reader_source_id=?1 AND record_key=?2",
            params![source, key.as_str()],
        )?;
        tx.execute("UPDATE core_provider_jobs SET state='superseded' WHERE reader_source_id=?1 AND record_key=?2",params![source,key.as_str()])?;
        queue(tx, &source)?;
        Ok(())
    }
    /// Pure SQL authority revocation. Call inside the registry-disable transaction;
    /// physical cache/index cleanup is separate and cannot make this gate pending.
    pub fn revoke(&self, tx: &Transaction<'_>, binding: &SourceBinding) -> Result<()> {
        let digest = self.validate_binding(binding)?;
        let source = format!("provider:{digest}");
        tx.execute("UPDATE core_provider_slots SET enabled=0 WHERE library_id=?1 AND service_source_id=?2 AND binding_digest=?3",params![self.library_id.as_str(),binding.source_id().as_str(),digest])?;
        tx.execute("UPDATE sources SET enabled=0 WHERE id=?1", [&source])?;
        tx.execute(
            "UPDATE core_provider_jobs SET state='revoked' WHERE reader_source_id=?1",
            [&source],
        )?;
        tx.execute("UPDATE index_jobs SET phase='error',error='Source unavailable',generation=generation+1 WHERE source_id=?1",[&source])?;
        Ok(())
    }
}
fn queue(tx: &Transaction<'_>, source: &str) -> Result<()> {
    tx.execute("INSERT INTO index_jobs(source_id,phase) VALUES(?1,'queued') ON CONFLICT(source_id) DO UPDATE SET phase='queued',error=NULL,generation=index_jobs.generation+1",[source])?;
    Ok(())
}

impl ReaderStore {
    pub fn source_kind(&self, source: &str) -> Result<SourceKind> {
        if !self.library.query_row(
            "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1)",
            [source],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(ReaderError::Unavailable);
        }
        Ok(if source.starts_with("provider:") {
            SourceKind::ProviderDerived
        } else {
            SourceKind::Local
        })
    }
    pub(super) fn provider_enabled(&self, source: &str) -> Result<()> {
        let enabled:bool=self.library.query_row("SELECT EXISTS(SELECT 1 FROM core_provider_sources s JOIN core_provider_slots g ON g.library_id=s.library_id AND g.service_source_id=s.service_source_id AND g.binding_digest=s.binding_digest JOIN sources granted ON granted.id=s.reader_source_id WHERE s.reader_source_id=?1 AND g.enabled=1 AND granted.enabled=1)",[source],|r|r.get(0))?;
        if !enabled {
            return Err(ReaderError::Unavailable);
        }
        Ok(())
    }
    pub(super) fn provider_document(
        &self,
        source: &str,
        path: &str,
        expected: Option<&str>,
    ) -> Result<Document> {
        self.provider_enabled(source)?;
        let (revision,hash,cache_path):(String,String,String)=self.library.query_row("SELECT revision,body_hash,cache_path FROM core_provider_records WHERE reader_source_id=?1 AND path=?2",params![source,path],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or(ReaderError::Unavailable)?;
        if expected.is_some_and(|e| e != revision) {
            return Err(ReaderError::Conflict);
        }
        let root = self.stored_root(source, true)?;
        #[cfg(windows)]
        let bytes = {
            let library: String = self.library.query_row(
                "SELECT library_id FROM core_provider_profile WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            windows_cache::Cache::read_at(&root, &library, &cache_path)?
        };
        #[cfg(not(windows))]
        let bytes = {
            let mut bytes = Vec::new();
            root.open_file_relative(Path::new(&cache_path))?
                .take(MAX_DOCUMENT_BYTES + 1)
                .read_to_end(&mut bytes)?;
            bytes
        };
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES || super::revision(&bytes) != hash {
            return Err(ReaderError::Conflict);
        }
        let markdown = String::from_utf8(bytes).map_err(|_| invalid("Markdown must be UTF-8"))?;
        root.verify()?;
        self.provider_enabled(source)?;
        let current: Option<String> = self
            .library
            .query_row(
                "SELECT revision FROM core_provider_records WHERE reader_source_id=?1 AND path=?2",
                params![source, path],
                |r| r.get(0),
            )
            .optional()?;
        if current.as_deref() != Some(&revision) {
            return Err(ReaderError::Conflict);
        }
        Ok(Document {
            source_id: source.into(),
            path: path.into(),
            revision,
            markdown,
        })
    }
    pub(super) fn index_provider(&mut self, source: &str, cancel: &Cancellation) -> Result<usize> {
        self.provider_enabled(source)?;
        let records:Vec<(String,String,i64)>=self.library.prepare("SELECT path,revision,modified_ms FROM core_provider_records WHERE reader_source_id=?1 ORDER BY path LIMIT 50001")?.query_map([source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<std::result::Result<_,_>>()?;
        if records.len() > 50000 {
            return Err(invalid("provider record indexing exceeds budget"));
        }
        let root = self.stored_root(source, true)?;
        // Clone the reader authority to keep streaming per-document cache reads
        // independent from the mutable index transaction. No cache directory scan.
        let scan = uuid::Uuid::new_v4().to_string();
        let tx = self.index.unchecked_transaction()?;
        let mut changed = false;
        for (path, revision, modified) in &records {
            cancel.check()?;
            let document = self.provider_document(source, path, Some(revision))?;
            changed |= index_markdown(
                &tx,
                source,
                path,
                revision,
                &document.markdown,
                *modified,
                &scan,
            )?;
        }
        changed |= tx.execute(
            "DELETE FROM documents WHERE source_id=?1 AND scan<>?2",
            params![source, scan],
        )? > 0;
        if changed {
            rebuild_resolved_links(&tx, source, cancel)?;
        }
        cancel.check()?;
        root.verify()?;
        self.provider_enabled(source)?;
        catalog::schema::publish(&tx, source)?;
        tx.commit()?;
        Ok(records.len())
    }
}
