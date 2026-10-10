//! Durable Windows stage publication intent. NT rename is projection; this
//! private SQLite namespace owns the publication phase and exact identity.
use super::*;
use rusqlite::{params, Connection, OptionalExtension};

const DATABASE: &str = ".reader-profile-publications-v1.sqlite";
const LOCK: &str = ".reader-profile-publications-v1.lock";
const MAX_ROWS: u64 = 4096;
const APPLICATION: u32 = 0x4d445250;
const TABLES: [(&str, &str); 2] = [
    ("publication_meta", "CREATE TABLE publication_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL)"),
    ("publications", "CREATE TABLE publications(destination TEXT PRIMARY KEY,stage TEXT NOT NULL,identity TEXT NOT NULL,manifest TEXT NOT NULL,digest TEXT NOT NULL,phase TEXT NOT NULL CHECK(phase IN('prepared','published')))"),
];

pub(super) struct Journal {
    // Field drop order closes SQLite before releasing file pins/byte lock,
    // while the exact private ancestor chain is still retained.
    connection: Connection,
    database: File,
    _rollback: File,
    _lock: File,
    directory: PrivateDirectory,
}
impl Journal {
    pub(super) fn open(parent: &Path) -> Result<Self> {
        let directory = PrivateDirectory::open(parent).map_err(|_| Error::Invalid)?;
        let lock = directory
            .open_lock(Path::new(LOCK))
            .map_err(|_| Error::Storage)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::NotQuiescent)?;
        let database = directory
            .database_file(Path::new(DATABASE), true)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Storage)?;
        let length = database.metadata().map_err(|_| Error::Storage)?.len();
        if length > 16 * 1024 * 1024 {
            return Err(Error::Bounds);
        }
        for suffix in ["-wal", "-shm"] {
            if directory
                .database_file(Path::new(&format!("{DATABASE}{suffix}")), false)
                .map_err(|_| Error::Invalid)?
                .is_some()
            {
                return Err(Error::Unsupported);
            }
        }
        let old_rollback = directory
            .database_file(Path::new(&format!("{DATABASE}-journal")), false)
            .map_err(|_| Error::Storage)?;
        if old_rollback
            .as_ref()
            .map(|f| f.metadata().map(|m| m.len()))
            .transpose()
            .map_err(|_| Error::Storage)?
            .is_some_and(|n| n > 16 * 1024 * 1024)
        {
            return Err(Error::Bounds);
        }
        let mut connection = Connection::open(parent.join(DATABASE)).map_err(|_| Error::Storage)?;
        connection
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "trusted_schema", "OFF")
            .map_err(|_| Error::Storage)?;
        // Non-NeedSchema pragma before recovery of a pinned hot rollback file.
        connection
            .pragma_update(None, "locking_mode", "EXCLUSIVE")
            .map_err(|_| Error::Storage)?;
        let count: u64 = connection
            .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        let fresh = length == 0 && count == 0;
        if !fresh {
            schema(&connection)?;
        } else {
            connection
                .pragma_update(None, "page_size", 4096)
                .map_err(|_| Error::Storage)?;
        }
        // Creating a new sidecar is permitted only after exact namespace
        // admission; a foreign/future database is never initialized or repaired.
        let rollback = match old_rollback {
            Some(file) => file,
            None => directory
                .database_file(Path::new(&format!("{DATABASE}-journal")), true)
                .map_err(|_| Error::Storage)?
                .ok_or(Error::Storage)?,
        };
        let page_size: u64 = connection
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        if page_size != 4096 {
            return Err(Error::Unsupported);
        }
        let mode: String = connection
            .query_row("PRAGMA journal_mode=PERSIST", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        if mode != "persist" {
            return Err(Error::Unsupported);
        }
        connection
            .pragma_update(None, "synchronous", "EXTRA")
            .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "max_page_count", 4096)
            .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "journal_size_limit", 16 * 1024 * 1024)
            .map_err(|_| Error::Storage)?;
        if fresh {
            let tx = connection.transaction().map_err(|_| Error::Storage)?;
            tx.pragma_update(None, "application_id", APPLICATION)
                .map_err(|_| Error::Storage)?;
            for (_, sql) in TABLES {
                tx.execute_batch(sql).map_err(|_| Error::Storage)?;
            }
            tx.execute("INSERT INTO publication_meta VALUES(1,1)", [])
                .map_err(|_| Error::Storage)?;
            tx.commit().map_err(|_| Error::ReconciliationRequired)?;
        }
        schema(&connection)?;
        connection
            .pragma_update(None, "locking_mode", "NORMAL")
            .map_err(|_| Error::Storage)?;
        connection
            .execute_batch("BEGIN; SELECT count(*) FROM sqlite_schema; COMMIT;")
            .map_err(|_| Error::Storage)?;
        let journal = Self {
            directory,
            _lock: lock,
            database,
            _rollback: rollback,
            connection,
        };
        journal.verify()?;
        Ok(journal)
    }
    fn verify(&self) -> Result<()> {
        self.directory.verify().map_err(|_| Error::Conflict)?;
        let named = self
            .directory
            .database_file(Path::new(DATABASE), false)
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        if !mdreader_capture::same_open_file(&self.database, &named).map_err(|_| Error::Conflict)? {
            return Err(Error::Conflict);
        }
        schema(&self.connection)
    }
    pub(super) fn prepare(
        &mut self,
        destination: &str,
        stage: &str,
        binding: &RootBinding,
        manifest: &str,
        digest: &str,
    ) -> Result<()> {
        if !crate::model::valid_hash(digest) || !matches!(manifest, "manifest.json" | "bundle.json")
        {
            return Err(Error::Invalid);
        }
        binding.validate()?;
        let identity = serde_json::to_string(binding).map_err(|_| Error::Invalid)?;
        if identity.len() > 65536 {
            return Err(Error::Bounds);
        }
        self.verify()?;
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema(&tx)?;
        let count: u64 = tx
            .query_row("SELECT count(*) FROM publications", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        if count >= MAX_ROWS {
            return Err(Error::Bounds);
        }
        tx.execute(
            "INSERT INTO publications VALUES(?1,?2,?3,?4,?5,'prepared')",
            params![destination, stage, identity, manifest, digest],
        )
        .map_err(|_| Error::Conflict)?;
        schema(&tx)?;
        self.directory.verify().map_err(|_| Error::Conflict)?;
        tx.commit().map_err(|_| Error::ReconciliationRequired)?;
        self.verify()
    }
    pub(super) fn finish(
        &mut self,
        destination: &str,
        stage: &str,
        binding: &RootBinding,
        manifest: &str,
        digest: &str,
    ) -> Result<()> {
        self.verify()?;
        let identity = serde_json::to_string(binding).map_err(|_| Error::Invalid)?;
        let tx = self.connection.transaction().map_err(|_| Error::Storage)?;
        schema(&tx)?;
        let count = tx.execute("UPDATE publications SET phase='published' WHERE destination=?1 AND stage=?2 AND identity=?3 AND manifest=?4 AND digest=?5 AND phase='prepared'",
            params![destination, stage, identity, manifest, digest]).map_err(|_| Error::Storage)?;
        if count != 1 {
            return Err(Error::Conflict);
        }
        schema(&tx)?;
        self.directory.verify().map_err(|_| Error::Conflict)?;
        tx.commit().map_err(|_| Error::ReconciliationRequired)?;
        self.verify()
    }
}
fn schema(connection: &Connection) -> Result<()> {
    let application: u32 = connection
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|_| Error::Storage)?;
    let mut statement = connection.prepare("SELECT name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 3").map_err(|_| Error::Invalid)?;
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|_| Error::Invalid)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| Error::Invalid)?;
    if application != APPLICATION
        || rows.len() != TABLES.len()
        || rows
            .iter()
            .any(|(n, s)| !TABLES.iter().any(|(name, sql)| n == name && s == sql))
    {
        return Err(Error::Unsupported);
    }
    let version: Option<u64> = connection.query_row("SELECT version FROM publication_meta WHERE singleton=1 AND (SELECT count(*) FROM publication_meta)=1", [], |r|r.get(0)).optional().map_err(|_| Error::Invalid)?;
    let invalid: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM publications WHERE typeof(destination)!='text' OR length(destination) NOT BETWEEN 1 AND 64 OR destination GLOB '*[^a-zA-Z0-9_-]*' OR typeof(stage)!='text' OR length(stage)!=51 OR stage NOT GLOB '.profile-stage-*' OR typeof(identity)!='text' OR length(CAST(identity AS BLOB))>65536 OR NOT json_valid(identity) OR manifest NOT IN('manifest.json','bundle.json') OR typeof(digest)!='text' OR length(digest)!=64 OR digest GLOB '*[^0-9a-f]*' OR phase NOT IN('prepared','published')) OR (SELECT count(*) FROM publications)>4096", [], |r|r.get(0)).map_err(|_| Error::Invalid)?;
    if version != Some(1) || invalid {
        return Err(Error::Unsupported);
    }
    Ok(())
}
