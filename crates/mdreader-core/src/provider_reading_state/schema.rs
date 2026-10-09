use crate::{ReaderError, Result};
use rusqlite::Connection;
const V1: &str = include_str!("migrations/001.sql");
fn presence(db: &Connection) -> Result<u32> {
    Ok(db.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('provider_reading_schema','provider_reading_states')", [], |row| row.get(0))?)
}
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    if presence(db)? == 0 {
        let tx = db.unchecked_transaction()?;
        tx.execute_batch(V1)?;
        tx.commit()?;
    }
    check(db)
}
/// Older profiles may omit this additive subsystem; never initialize on readonly open.
pub(crate) fn check(db: &Connection) -> Result<()> {
    match presence(db)? {
        0 => return Ok(()),
        2 => (),
        _ => {
            return Err(ReaderError::Invalid(
                "provider reading state schema is incomplete".into(),
            ))
        }
    }
    let versions = db
        .prepare("SELECT version FROM provider_reading_schema")?
        .query_map([], |row| row.get::<_, u32>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if versions != [1] {
        return Err(ReaderError::Invalid(
            "provider reading state requires owner migration".into(),
        ));
    }
    let columns = db
        .prepare("PRAGMA table_info(provider_reading_states)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if columns
        != [
            "source_id",
            "path",
            "revision",
            "pinned",
            "line",
            "generation",
        ]
    {
        return Err(ReaderError::Invalid(
            "provider reading state schema is unsupported".into(),
        ));
    }
    Ok(())
}
