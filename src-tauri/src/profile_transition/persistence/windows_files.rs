//! Canonical Windows session/settings bytes. SQLite EXTRA/PERSIST owns durable
//! publication; JSON filenames are neither dual writes nor migration receipts.
//! Existing old JSON remains untouched and requires coordinated legacy import.
use super::types::{PersistenceError as Error, Result, MAX_JSON_BYTES};
use mdreader_capture::{same_open_file, PrivateDirectory};
use mdreader_core::SourceRoot;
use rusqlite::{params, Connection, OptionalExtension};
use std::{fs::File, path::Path, sync::Mutex};

#[path = "windows_files/schema.rs"]
mod schema;
const DATABASE: &str = "reader-json-v1.sqlite";
const LOCK: &str = ".mdreader-json-owner.lock";
const MAX_PAGES: u64 = 4096; // 16 MiB including reusable free pages.

pub(super) struct JsonLifecycle {
    private: PrivateDirectory,
    connection: Mutex<Connection>,
    lock: File,
    _database: File,
    _rollback: File,
}
impl JsonLifecycle {
    pub(super) fn capture(root: &SourceRoot) -> Result<Self> {
        root.verify().map_err(|_| Error::Conflict)?;
        let private = PrivateDirectory::open(&root.path).map_err(|_| Error::Unavailable)?;
        let lock = private
            .open_lock(Path::new(LOCK))
            .map_err(|_| Error::Storage)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::Unavailable)?;
        let database = private
            .database_file(Path::new(DATABASE), true)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Storage)?;
        let length = database.metadata().map_err(|_| Error::Storage)?.len();
        if length > MAX_PAGES * 4096 {
            return Err(Error::TooLarge);
        }
        for suffix in ["-wal", "-shm"] {
            if private
                .database_file(Path::new(&format!("{DATABASE}{suffix}")), false)
                .map_err(|_| Error::Conflict)?
                .is_some()
            {
                return Err(Error::UnsupportedSchema);
            }
        }
        let old_rollback = private
            .database_file(Path::new(&format!("{DATABASE}-journal")), false)
            .map_err(|_| Error::Storage)?;
        if old_rollback
            .as_ref()
            .map(|file| file.metadata().map(|metadata| metadata.len()))
            .transpose()
            .map_err(|_| Error::Storage)?
            .is_some_and(|length| length > MAX_PAGES * 4096)
        {
            return Err(Error::TooLarge);
        }
        let mut connection =
            Connection::open(root.path.join(DATABASE)).map_err(|_| Error::Storage)?;
        connection
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "trusted_schema", "OFF")
            .map_err(|_| Error::Storage)?;
        // No schema read before this connection-local flag: hot recovery must
        // zero the pinned journal header, not delete its Windows handle.
        connection
            .pragma_update(None, "locking_mode", "EXCLUSIVE")
            .map_err(|_| Error::Storage)?;
        let count: u64 = connection
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        let fresh = length == 0 && count == 0;
        if fresh {
            // No claim that an old plugin writer cooperates with this new lock.
            // Even valid old wrappers must remain original bytes until the
            // explicit Windows closed-owner migration boundary is available.
            for name in ["session.json", "settings.json"] {
                if root
                    .open_optional_file_relative(Path::new(name))
                    .map_err(|_| Error::Conflict)?
                    .is_some()
                {
                    return Err(Error::UnsupportedSchema);
                }
            }
            connection
                .pragma_update(None, "page_size", 4096)
                .map_err(|_| Error::Storage)?;
        } else {
            schema::verify(&connection)?;
        }
        let rollback = match old_rollback {
            Some(file) => file,
            None => private
                .database_file(Path::new(&format!("{DATABASE}-journal")), true)
                .map_err(|_| Error::Storage)?
                .ok_or(Error::Storage)?,
        };
        let page_size: u64 = connection
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        if page_size != 4096 {
            return Err(Error::UnsupportedSchema);
        }
        let mode: String = connection
            .query_row("PRAGMA journal_mode=PERSIST", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        if mode != "persist" {
            return Err(Error::UnsupportedSchema);
        }
        connection
            .pragma_update(None, "synchronous", "EXTRA")
            .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "max_page_count", MAX_PAGES)
            .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)
            .map_err(|_| Error::Storage)?;
        if fresh {
            schema::initialize(&mut connection)?;
        }
        schema::verify(&connection)?;
        connection
            .pragma_update(None, "locking_mode", "NORMAL")
            .map_err(|_| Error::Storage)?;
        connection
            .execute_batch("BEGIN; SELECT count(*) FROM sqlite_master; COMMIT;")
            .map_err(|_| Error::Storage)?;
        let owner = Self {
            private,
            connection: Mutex::new(connection),
            lock,
            _database: database,
            _rollback: rollback,
        };
        owner.verify(root)?;
        Ok(owner)
    }
    pub(super) fn verify(&self, root: &SourceRoot) -> Result<()> {
        root.verify().map_err(|_| Error::Conflict)?;
        self.private.verify().map_err(|_| Error::Conflict)?;
        let observed = self
            .private
            .open_lock(Path::new(LOCK))
            .map_err(|_| Error::Conflict)?;
        if !same_open_file(&self.lock, &observed).map_err(|_| Error::Conflict)? {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
impl Drop for JsonLifecycle {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.lock);
    }
}

pub(super) struct Original {
    pub bytes: Vec<u8>,
}
fn key(path: &Path) -> Result<&str> {
    match path.to_str() {
        Some("session.json") => Ok("session.json"),
        Some("settings.json") => Ok("settings.json"),
        _ => Err(Error::Invalid),
    }
}
fn bytes(connection: &Connection, name: &str) -> Result<Option<Vec<u8>>> {
    let row: Option<Vec<u8>> = connection.query_row(
        "SELECT body FROM reader_json WHERE name=?1 AND typeof(body)='blob' AND length(body)<=?2",
        params![name, MAX_JSON_BYTES], |r| r.get(0)).optional().map_err(|_| Error::Storage)?;
    if row.is_none()
        && connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM reader_json WHERE name=?1)",
                [name],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|_| Error::Storage)?
    {
        return Err(Error::TooLarge);
    }
    Ok(row)
}
pub(super) fn read(
    root: &SourceRoot,
    lifecycle: &JsonLifecycle,
    path: &Path,
) -> Result<Option<Original>> {
    let name = key(path)?;
    lifecycle.verify(root)?;
    let connection = lifecycle
        .connection
        .lock()
        .map_err(|_| Error::Unavailable)?;
    schema::verify(&connection)?;
    let original = bytes(&connection, name)?.map(|bytes| Original { bytes });
    lifecycle.verify(root)?;
    Ok(original)
}
pub(super) fn verify(
    root: &SourceRoot,
    lifecycle: &JsonLifecycle,
    path: &Path,
    original: Option<&Original>,
) -> Result<()> {
    let observed = read(root, lifecycle, path)?;
    if observed.as_ref().map(|v| &v.bytes) != original.map(|v| &v.bytes) {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(super) fn replace(
    root: &SourceRoot,
    lifecycle: &JsonLifecycle,
    path: &Path,
    original: Option<&Original>,
    body: &[u8],
) -> Result<()> {
    let name = key(path)?;
    if body.len() > MAX_JSON_BYTES {
        return Err(Error::TooLarge);
    }
    lifecycle.verify(root)?;
    let mut connection = lifecycle
        .connection
        .lock()
        .map_err(|_| Error::Unavailable)?;
    let tx = connection.transaction().map_err(|_| Error::Storage)?;
    schema::verify(&tx)?;
    if bytes(&tx, name)?.as_deref() != original.map(|v| v.bytes.as_slice()) {
        return Err(Error::Conflict);
    }
    tx.execute(
        "INSERT INTO reader_json VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET body=excluded.body",
        params![name, body],
    )
    .map_err(|_| Error::Storage)?;
    schema::verify(&tx)?;
    lifecycle.verify(root)?;
    // A failed COMMIT cannot be represented as provably unsent. JsonOwner
    // quarantines this renderer lifecycle until a fresh native owner is opened.
    tx.commit().map_err(|_| Error::PublicationUncertain)?;
    lifecycle
        .verify(root)
        .map_err(|_| Error::PublicationUncertain)?;
    if bytes(&connection, name)
        .map_err(|_| Error::PublicationUncertain)?
        .as_deref()
        != Some(body)
    {
        return Err(Error::PublicationUncertain);
    }
    Ok(())
}
