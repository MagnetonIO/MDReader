//! Lazy ordered namespace. No repair/reset or unrelated global schema changes.
use super::*;
use rusqlite::{Connection, Transaction};
const SQL: &str = include_str!("migrations/001-selected-outgoing.sql");
pub(in crate::sync) fn present(db: &Connection) -> Result<bool> {
    let count: u64 = db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name GLOB 'native_outgoing_*'",
        [],
        |r| r.get(0),
    )?;
    match count {
        0 => Ok(false),
        2 => Ok(true),
        _ => Err(Error::Schema),
    }
}
pub(in crate::sync) fn initialize(tx: &Transaction<'_>) -> Result<()> {
    if !present(tx)? {
        tx.execute_batch(SQL)?;
    }
    require(tx)
}
pub(in crate::sync) fn require(db: &Connection) -> Result<()> {
    if !present(db)? {
        return Err(Error::Schema);
    }
    for name in ["native_outgoing_schema_v1", "native_outgoing_intents_v1"] {
        let expected = SQL
            .split(';')
            .find(|s| s.trim_start().starts_with(&format!("CREATE TABLE {name}(")))
            .ok_or(Error::Schema)?;
        let actual: String = db
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name=?1 AND type='table'",
                [name],
                |r| r.get(0),
            )
            .map_err(|_| Error::Schema)?;
        if normalize(&actual) != normalize(expected) {
            return Err(Error::Schema);
        }
    }
    let (count,version):(u64,Option<i64>)=db.query_row("SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM native_outgoing_schema_v1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count != 1 || version != Some(1) {
        return Err(Error::Schema);
    }
    let (count,bad):(u64,bool)=db.query_row("SELECT count(*),coalesce(max(typeof(scope)!='text' OR length(CAST(scope AS BLOB))!=64 OR length(CAST(operation_id AS BLOB))!=36 OR length(CAST(wire_item AS BLOB))!=36 OR length(CAST(wire_revision AS BLOB))!=36 OR length(CAST(native_document AS BLOB))!=36 OR typeof(intent)!='text' OR length(CAST(intent AS BLOB))>4096 OR state NOT IN('queued','applied') OR (state='queued' AND sequence IS NOT NULL) OR (state='applied' AND (typeof(sequence)!='integer' OR sequence NOT BETWEEN 1 AND 9007199254740991))),0) FROM native_outgoing_intents_v1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count > 4096 || bad {
        return Err(Error::Schema);
    }
    Ok(())
}
fn normalize(sql: &str) -> String {
    // Immutable SQL literals contain no whitespace; retain their case/content.
    sql.trim()
        .trim_end_matches(';')
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect()
}
