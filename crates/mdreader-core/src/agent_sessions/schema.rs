use crate::{ReaderError, ReaderStore, Result};
use rusqlite::Connection;
const SQL: &str = include_str!("schema.sql");
fn validate(db: &Connection) -> Result<()> {
    let count: usize = db.query_row(
        "SELECT count(*) FROM main.sqlite_schema WHERE name GLOB 'reader_agent_*'",
        [],
        |r| r.get(0),
    )?;
    if count != 3 {
        return Err(ReaderError::Unavailable);
    }
    for definition in SQL.split(';').filter(|s| !s.trim().is_empty()) {
        let definition = definition.trim();
        let name = definition
            .split([' ', '('])
            .nth(2)
            .ok_or(ReaderError::Unavailable)?;
        let actual: String = db.query_row(
            "SELECT sql FROM main.sqlite_schema WHERE type='table' AND name=?1",
            [name],
            |r| r.get(0),
        )?;
        if actual != definition {
            return Err(ReaderError::Unavailable);
        }
    }
    let foreign: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE name GLOB 'reader_agent_*' OR tbl_name GLOB 'reader_agent_*') OR EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name GLOB 'reader_agent_*')", [], |r|r.get(0))?;
    let marker: (usize, Option<i64>) = db.query_row(
        "SELECT count(*),max(CASE WHEN id=1 THEN version END) FROM main.reader_agent_sessions_meta",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if foreign || marker != (1, Some(1)) {
        return Err(ReaderError::Unavailable);
    }
    Ok(())
}
pub(super) fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db| {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name GLOB 'reader_agent_*')",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            let tx = db.transaction()?;
            tx.execute_batch(SQL)?;
            tx.execute("INSERT INTO reader_agent_sessions_meta VALUES(1,1)", [])?;
            validate(&tx)?;
            tx.commit()?;
        }
        validate(db)
    })
}
