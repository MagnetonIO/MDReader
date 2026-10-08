//! Exact bounded installer namespace; unknown/partial databases are not repaired.
use super::*;
use mdreader_capture::PrivateDirectory;
use rusqlite::{Connection, OptionalExtension};
use std::{fs::File, path::Path};
const NAME: &str = "installer-v1.sqlite";
const APP: u32 = 0x4d445249;
const MAX: usize = 4 * 1024 * 1024;
const SCHEMA: [(&str, &str); 2] = [
    (
        "installer_meta",
        "CREATE TABLE installer_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL)",
    ),
    (
        "installer_receipts",
        "CREATE TABLE installer_receipts(id TEXT PRIMARY KEY CHECK(length(id)=64),record BLOB NOT NULL CHECK(typeof(record)='blob' AND length(record)<=4194304),bytes INTEGER NOT NULL CHECK(bytes>0 AND bytes=length(record)))",
    ),
];
pub(crate) struct Journal {
    connection: Connection,
    _database: File,
    _rollback: File,
}
impl Journal {
    pub(crate) fn open(root: &Path, authority: &PrivateDirectory) -> Result<Self> {
        authority.verify().map_err(capture)?;
        let database = authority
            .database_file(Path::new(NAME), true)
            .map_err(capture)?
            .ok_or(Error::Invalid)?;
        let length = database.metadata()?.len();
        if length > 64 * 1024 * 1024 {
            return Err(Error::Bounds);
        }
        let rollback_name = format!("{NAME}-journal");
        let old = authority
            .database_file(Path::new(&rollback_name), false)
            .map_err(capture)?;
        for suffix in ["-wal", "-shm"] {
            if authority
                .database_file(Path::new(&format!("{NAME}{suffix}")), false)
                .map_err(capture)?
                .is_some()
            {
                return Err(Error::Invalid);
            }
        }
        let mut connection = Connection::open(root.join(NAME))?;
        connection.busy_timeout(std::time::Duration::ZERO)?;
        let objects: u64 =
            connection.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        let fresh = length == 0 && objects == 0;
        if !fresh {
            verify(&connection)?;
        }
        // No prior Windows file receipts are silently adopted or ignored.
        if fresh
            && root.join("receipts").exists()
            && std::fs::read_dir(root.join("receipts"))?.next().is_some()
        {
            return Err(Error::Conflict);
        }
        let rollback = match old {
            Some(file) => file,
            None => authority
                .database_file(Path::new(&rollback_name), true)
                .map_err(capture)?
                .ok_or(Error::Invalid)?,
        };
        if fresh {
            connection.pragma_update(None, "page_size", 4096)?;
        }
        if connection.pragma_query_value(None, "page_size", |r| r.get::<_, u64>(0))? != 4096 {
            return Err(Error::Invalid);
        }
        let mode: String = connection.query_row("PRAGMA journal_mode=PERSIST", [], |r| r.get(0))?;
        if mode != "persist" {
            return Err(Error::Unsupported);
        }
        connection.pragma_update(None, "synchronous", "EXTRA")?;
        connection.pragma_update(None, "trusted_schema", "OFF")?;
        connection.pragma_update(None, "max_page_count", 16384)?;
        connection.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
        if fresh {
            let tx = connection.transaction()?;
            tx.pragma_update(None, "application_id", APP)?;
            for (_, sql) in SCHEMA {
                tx.execute_batch(sql)?;
            }
            tx.execute("INSERT INTO installer_meta VALUES(1,1)", [])?;
            tx.commit()?;
        }
        verify(&connection)?;
        authority.verify().map_err(capture)?;
        Ok(Self {
            connection,
            _database: database,
            _rollback: rollback,
        })
    }
    pub(crate) fn read(&self, id: &str) -> Result<Option<String>> {
        valid_id(id)?;
        let row: Option<(Vec<u8>, usize)> = self
            .connection
            .query_row(
                "SELECT record,bytes FROM installer_receipts WHERE id=?1 AND length(record)<=?2",
                (id, MAX),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((bytes, length)) if bytes.len() == length => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| Error::Invalid),
            Some(_) => Err(Error::Invalid),
            None => {
                if self.connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM installer_receipts WHERE id=?1)",
                    [id],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(Error::Bounds);
                }
                Ok(None)
            }
        }
    }
    pub(crate) fn save(&mut self, id: &str, bytes: &[u8]) -> Result<()> {
        valid_id(id)?;
        if bytes.is_empty() || bytes.len() > MAX {
            return Err(Error::Bounds);
        }
        let tx = self.connection.transaction()?;
        let (count, total): (u64, u64) = tx.query_row(
            "SELECT count(*),coalesce(sum(bytes),0) FROM installer_receipts WHERE id!=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count >= 256 || total.saturating_add(bytes.len() as u64) > 32 * 1024 * 1024 {
            return Err(Error::Bounds);
        }
        tx.execute("INSERT INTO installer_receipts VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET record=excluded.record,bytes=excluded.bytes",(id,bytes,bytes.len()))?;
        tx.commit()?;
        Ok(())
    }
}
fn valid_id(id: &str) -> Result<()> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn verify(connection: &Connection) -> Result<()> {
    if connection.pragma_query_value(None, "application_id", |r| r.get::<_, u32>(0))? != APP {
        return Err(Error::Invalid);
    }
    let rows=connection.prepare("SELECT CASE WHEN length(name)<=64 THEN name ELSE NULL END,CASE WHEN length(sql)<=4096 THEN sql ELSE NULL END FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 3")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    if rows.len() != 2
        || rows
            .iter()
            .any(|(n, s)| !SCHEMA.iter().any(|(name, sql)| n == name && s == sql))
    {
        return Err(Error::Invalid);
    }
    let (version, count): (u64, u64) = connection.query_row(
        "SELECT version,(SELECT count(*) FROM installer_meta) FROM installer_meta WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if version != 1 || count != 1 {
        return Err(Error::Invalid);
    }
    Ok(())
}
