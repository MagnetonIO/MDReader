//! Windows canonical capture journal. SQLite FULL synchronous commits own queue,
//! bodies, provenance and receipts; Markdown files are rebuildable projections.
//! No directory-fsync equivalence or permanent file/DB dual-authority is claimed.
use super::*;
use rusqlite::{Connection, OptionalExtension};
mod projection;
mod queue;
mod schema;
const DATABASE: &str = "capture-v1.sqlite";
const MAX_DATABASE_PAGES: u64 = 1_048_576; // 4 GiB; SQLITE_FULL retains pending data.
pub(super) struct Journal {
    connection: Connection,
    _database: File,
    _rollback: File,
}
impl Journal {
    pub(super) fn open(spool: &Spool) -> Result<Self> {
        spool.verify()?;
        let path = spool.root.join(DATABASE);
        // Precreate protected noninherited DACL objects; SQLite's PERSIST mode
        // reuses the rollback journal, and never relies on deleting it to commit.
        let database = platform::pin_database(&path)?;
        let length = database.metadata()?.len();
        if length > MAX_DATABASE_PAGES * 4096 {
            return Err(CaptureError::QueueFull);
        }
        let pristine = length == 0;
        let previous_rollback =
            platform::pin_existing_database(&spool.root.join(format!("{DATABASE}-journal")))?;
        for suffix in ["-wal", "-shm"] {
            if regular_exists(&spool.root.join(format!("{DATABASE}{suffix}")))? {
                return Err(CaptureError::Invalid);
            }
        }
        let mut connection = Connection::open(&path)?;
        connection.busy_timeout(std::time::Duration::ZERO)?;
        // Querying schema also completes SQLite's own hot-journal recovery.
        let objects: u64 =
            connection.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        let fresh = pristine && objects == 0;
        if !fresh {
            schema::verify(&connection)?;
        }
        if fresh {
            for name in ["pending", "markdown", "receipts"] {
                if fs::read_dir(spool.root.join(name))?.next().is_some() {
                    return Err(CaptureError::Invalid);
                }
            }
        }
        // Only after the exact existing namespace is accepted may SQLite policy
        // change or a missing rollback file be created.
        let rollback = match previous_rollback {
            Some(file) => file,
            None => platform::pin_database(&spool.root.join(format!("{DATABASE}-journal")))?,
        };
        if fresh {
            connection.pragma_update(None, "page_size", 4096)?;
        }
        let page_size: u64 = connection.pragma_query_value(None, "page_size", |r| r.get(0))?;
        if page_size != 4096 {
            return Err(CaptureError::Invalid);
        }
        let mode: String = connection.query_row("PRAGMA journal_mode=PERSIST", [], |r| r.get(0))?;
        if mode != "persist" {
            return Err(CaptureError::UnsupportedPlatform);
        }
        connection.pragma_update(None, "synchronous", "EXTRA")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "trusted_schema", "OFF")?;
        connection.pragma_update(None, "max_page_count", MAX_DATABASE_PAGES)?;
        connection.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
        if fresh {
            schema::initialize(&mut connection)?;
        }
        schema::verify(&connection)?;
        spool.verify()?;
        Ok(Self {
            connection,
            _database: database,
            _rollback: rollback,
        })
    }
    fn pending(&self, id: &str, limits: &Limits) -> Result<Option<CaptureRecord>> {
        if !valid_id(id) {
            return Err(CaptureError::Invalid);
        }
        let row: Option<(Vec<u8>, usize)> = self
            .connection
            .query_row(
                "SELECT record,bytes FROM capture_pending WHERE id=?1 AND length(record)<=?2",
                (id, limits.max_event_bytes),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((bytes, length)) => {
                if bytes.len() != length {
                    return Err(CaptureError::Invalid);
                }
                let record: CaptureRecord =
                    serde_json::from_slice(&bytes).map_err(|_| CaptureError::Invalid)?;
                validate_record(&record)?;
                if record.id != id {
                    return Err(CaptureError::Invalid);
                }
                Ok(Some(record))
            }
            None => {
                if self.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM capture_pending WHERE id=?1)",
                    [id],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(CaptureError::TooLarge);
                }
                Ok(None)
            }
        }
    }
    fn body(&self, id: &str, limits: &Limits) -> Result<Option<Vec<u8>>> {
        if !valid_id(id) {
            return Err(CaptureError::Invalid);
        }
        let row: Option<(Vec<u8>,usize)> = self.connection.query_row("SELECT projection,bytes FROM capture_bodies WHERE id=?1 AND length(projection)<=?2", (id,limits.max_projection_bytes), |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        match row {
            Some((bytes, length)) => {
                let (metadata, _) = crate::projection::parse_projection_bytes(&bytes, limits)?;
                if bytes.len() != length || metadata.content_id != id {
                    return Err(CaptureError::Invalid);
                }
                Ok(Some(bytes))
            }
            None => {
                if self.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM capture_bodies WHERE id=?1)",
                    [id],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(CaptureError::TooLarge);
                }
                Ok(None)
            }
        }
    }
}
