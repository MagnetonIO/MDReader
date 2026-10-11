use super::*;
use rusqlite::Connection;
const LIBRARY: &str = include_str!("library.sql");
const INDEX: &str = include_str!("index.sql");
pub(crate) fn initialize(library: &Connection, index: &Connection) -> Result<()> {
    for (db, sql, marker) in [
        (library, LIBRARY, "reader_folder_schema_v1"),
        (index, INDEX, "reader_folder_index_schema_v1"),
    ] {
        let found: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [marker],
            |r| r.get(0),
        )?;
        if !found {
            let tx = db.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(&format!("INSERT INTO {marker} VALUES(1,1)"), [])?;
            validate(&tx, sql, marker)?;
            tx.commit()?;
        }
        validate(db, sql, marker)?;
    }
    Ok(())
}
fn validate(db: &Connection, sql: &str, marker: &str) -> Result<()> {
    for definition in sql.split(';').filter(|s| !s.trim().is_empty()) {
        let name = definition
            .trim()
            .split([' ', '('])
            .nth(2)
            .ok_or(ReaderError::Unavailable)?;
        let actual: String = db.query_row(
            "SELECT sql FROM main.sqlite_schema WHERE type='table' AND name=?1",
            [name],
            |r| r.get(0),
        )?;
        let temp: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?;
        if actual.trim() != definition.trim() || temp {
            return Err(invalid("Unsupported folder index schema"));
        }
    }
    let m: (usize, Option<i64>) = db.query_row(
        &format!("SELECT count(*),max(version) FROM {marker}"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let producers:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger' AND tbl_name GLOB 'reader_folder_*') OR EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE tbl_name GLOB 'reader_folder_*')",[],|r|r.get(0))?;
    if producers || m != (1, Some(1)) {
        return Err(invalid("Unsupported folder index schema"));
    }
    Ok(())
}
