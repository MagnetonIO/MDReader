use crate::{ReaderError, ReaderStore, Result};
use rusqlite::Connection;
const SQL: &str = include_str!("schema.sql");
/// Version 2 added associations with files in other chosen sources.
const VERSION: i64 = 2;
const SOURCE_DOCUMENTS: &str = "reader_agent_source_documents";
fn validate(db: &Connection) -> Result<()> {
    let count: usize = db.query_row(
        "SELECT count(*) FROM main.sqlite_schema WHERE name GLOB 'reader_agent_*'",
        [],
        |r| r.get(0),
    )?;
    if count != 4 {
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
    if foreign || marker != (1, Some(VERSION)) {
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
            tx.execute(
                "INSERT INTO reader_agent_sessions_meta VALUES(1,?1)",
                [VERSION],
            )?;
            validate(&tx)?;
            tx.commit()?;
        } else if version(db)? == Some(1) {
            // Additive upgrade; the full exact-schema check still gates commit.
            let definition = SQL
                .split(';')
                .find(|definition| definition.contains(SOURCE_DOCUMENTS))
                .ok_or(ReaderError::Unavailable)?;
            let tx = db.transaction()?;
            tx.execute_batch(definition.trim())?;
            tx.execute(
                "UPDATE reader_agent_sessions_meta SET version=?1 WHERE id=1",
                [VERSION],
            )?;
            validate(&tx)?;
            tx.commit()?;
        }
        validate(db)
    })
}
fn version(db: &Connection) -> Result<Option<i64>> {
    let meta: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name='reader_agent_sessions_meta')",
        [],
        |r| r.get(0),
    )?;
    if !meta {
        return Ok(None);
    }
    Ok(db.query_row(
        "SELECT max(CASE WHEN id=1 THEN version END) FROM main.reader_agent_sessions_meta",
        [],
        |r| r.get(0),
    )?)
}
