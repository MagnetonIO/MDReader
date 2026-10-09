//! Windows private immutable body cache. SQLite owns body durability; generated
//! cache paths are logical keys, not files or grants. No directory-fsync claim.
use super::*;
use mdreader_capture::PrivateDirectory;
use rusqlite::OpenFlags;
use std::{fs::File, sync::Mutex, time::Duration};
#[path = "windows_cache/schema.rs"]
mod schema;

const DATABASE: &str = "provider-bodies-v1.sqlite";
const PAGES: u64 = 131_072; // 512 MiB including orphaned immutable versions.
const MAX_BODIES: u64 = 50_000;

pub(super) struct Cache {
    connection: Mutex<Connection>,
    private: PrivateDirectory,
    library: LibraryId,
    _database: File,
    _rollback: File,
}

impl Cache {
    pub(super) fn open(path: &Path, library: &LibraryId) -> Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("expected absolute private cache path"));
        }
        // Existing inherited/broad DACLs, reparse points, remote/non-NTFS roots
        // and hardlinked files are refused by the reviewed platform boundary.
        let private = PrivateDirectory::create(path).map_err(private_error)?;
        let database = private
            .database_file(Path::new(DATABASE), true)
            .map_err(private_error)?
            .ok_or(ReaderError::Unavailable)?;
        let length = database.metadata()?.len();
        if length > PAGES * 4096 {
            return Err(invalid("provider cache exceeds budget"));
        }
        let old_rollback = private
            .database_file(Path::new(&format!("{DATABASE}-journal")), false)
            .map_err(private_error)?;
        no_wal(&private)?;
        let mut connection = Connection::open(path.join(DATABASE))?;
        connection.busy_timeout(Duration::ZERO)?;
        connection.pragma_update(None, "trusted_schema", "OFF")?;
        // Unlike journal_mode, locking_mode does not read schema first. A hot
        // rollback is finalized by zeroing its header in EXCLUSIVE mode, not
        // deleting the retained no-SHARE_DELETE journal. Only SQLite's own
        // interrupted transaction is recovered before namespace admission.
        connection.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
        let count: u64 =
            connection.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        let fresh = length == 0 && count == 0;
        if !fresh {
            schema::verify(&connection, library.as_str())?;
        }
        let rollback = match old_rollback {
            Some(file) => file,
            None => private
                .database_file(Path::new(&format!("{DATABASE}-journal")), true)
                .map_err(private_error)?
                .ok_or(ReaderError::Unavailable)?,
        };
        if fresh {
            connection.pragma_update(None, "page_size", 4096)?;
        }
        let size: u64 = connection.pragma_query_value(None, "page_size", |r| r.get(0))?;
        if size != 4096 {
            return Err(invalid("unsupported provider cache page size"));
        }
        // PERSIST uses the already pinned private journal. Successful EXTRA
        // commits do not depend on durable creation/removal of Markdown paths.
        let mode: String = connection.query_row("PRAGMA journal_mode=PERSIST", [], |r| r.get(0))?;
        if mode != "persist" {
            return Err(invalid("unsupported provider cache journal"));
        }
        connection.pragma_update(None, "synchronous", "EXTRA")?;
        connection.pragma_update(None, "max_page_count", PAGES)?;
        connection.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
        if fresh {
            schema::initialize(&mut connection, library.as_str())?;
        }
        schema::verify(&connection, library.as_str())?;
        connection.pragma_update(None, "locking_mode", "NORMAL")?;
        // Switching the flag alone retains the old exclusive lock until a
        // read transaction ends. Release it before sibling readonly readers.
        connection.execute_batch("BEGIN; SELECT count(*) FROM sqlite_master; COMMIT;")?;
        private.verify().map_err(private_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
            private,
            library: library.clone(),
            _database: database,
            _rollback: rollback,
        })
    }

    pub(super) fn verify(&self) -> Result<()> {
        self.private.verify().map_err(private_error)
    }

    /// Commits before the caller's borrowed library transaction publishes its
    /// authority receipt. Rollback there may leave a private orphan, never an
    /// admitted document. Retrying the same key cannot replace different bytes.
    pub(super) fn publish(&self, key: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        validate_key(key)?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES || revision(bytes) != hash {
            return Err(ReaderError::Conflict);
        }
        self.verify()?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ReaderError::Unavailable)?;
        let tx = connection.transaction()?;
        schema::verify(&tx, self.library.as_str())?;
        if let Some((old_hash, old_body)) = body(&tx, key)? {
            if old_hash != hash || old_body != bytes {
                return Err(ReaderError::Conflict);
            }
        } else {
            let count: u64 =
                tx.query_row("SELECT count(*) FROM provider_bodies", [], |r| r.get(0))?;
            if count >= MAX_BODIES {
                return Err(invalid("provider cache exceeds budget"));
            }
            tx.execute(
                "INSERT INTO provider_bodies VALUES(?1,?2,?3)",
                params![key, hash, bytes],
            )?;
        }
        self.verify()?;
        schema::verify(&tx, self.library.as_str())?;
        tx.commit()?;
        self.verify()
    }

    pub(super) fn read_at(root: &SourceRoot, library: &str, key: &str) -> Result<Vec<u8>> {
        validate_key(key)?;
        root.verify()?;
        let private = PrivateDirectory::open(&root.path).map_err(private_error)?;
        let database = private
            .database_file(Path::new(DATABASE), false)
            .map_err(private_error)?
            .ok_or(ReaderError::Unavailable)?;
        if database.metadata()?.len() > PAGES * 4096 {
            return Err(invalid("provider cache exceeds budget"));
        }
        let _rollback = private
            .database_file(Path::new(&format!("{DATABASE}-journal")), false)
            .map_err(private_error)?
            .ok_or(ReaderError::Unavailable)?;
        no_wal(&private)?;
        let connection = Connection::open_with_flags(
            root.path.join(DATABASE),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        connection.busy_timeout(Duration::ZERO)?;
        connection.pragma_update(None, "query_only", "ON")?;
        connection.pragma_update(None, "trusted_schema", "OFF")?;
        schema::verify(&connection, library)?;
        let (hash, bytes) = body(&connection, key)?.ok_or(ReaderError::Unavailable)?;
        if revision(&bytes) != hash {
            return Err(ReaderError::Conflict);
        }
        private.verify().map_err(private_error)?;
        root.verify()?;
        Ok(bytes)
    }
}

fn body(connection: &Connection, key: &str) -> Result<Option<(String, Vec<u8>)>> {
    let row = connection
        .query_row(
            "SELECT body_hash,body FROM provider_bodies WHERE path=?1 AND length(body_hash)=64 AND length(body)<=?2",
            params![key, MAX_DOCUMENT_BYTES],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if row.is_none()
        && connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_bodies WHERE path=?1)",
            [key],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(invalid("provider body exceeds budget"));
    }
    Ok(row)
}

fn no_wal(private: &PrivateDirectory) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        if private
            .database_file(Path::new(&format!("{DATABASE}{suffix}")), false)
            .map_err(private_error)?
            .is_some()
        {
            return Err(invalid("unsupported provider cache journal"));
        }
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<()> {
    if key.len() != 197 {
        return Err(invalid("invalid provider cache key"));
    }
    let parts: Vec<_> = key.split('/').collect();
    if parts.len() != 3
        || !parts[2].ends_with(".md")
        || !parts.iter().enumerate().all(|(i, part)| {
            let digest = if i == 2 {
                &part[..part.len() - 3]
            } else {
                part
            };
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Err(invalid("invalid provider cache key"));
    }
    Ok(())
}

fn private_error(_: mdreader_capture::CaptureError) -> ReaderError {
    ReaderError::Unavailable
}
