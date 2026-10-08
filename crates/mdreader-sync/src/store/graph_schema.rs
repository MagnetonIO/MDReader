//! Independent additive graph namespace. Applied SQL is immutable.
use super::*;
const SQL: &str = include_str!("migrations/001-graphs.sql");

pub(super) fn present(db: &Connection) -> Result<bool> {
    let count: u64 = db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name GLOB 'mdreader_sync_graph_*'",
        [],
        |r| r.get(0),
    )?;
    match count {
        0 => Ok(false),
        3 => Ok(true),
        _ => Err(SyncError::Schema),
    }
}
pub(super) fn initialize(tx: &Transaction<'_>) -> Result<()> {
    if !present(tx)? {
        tx.execute_batch(SQL)?;
    }
    require(tx)
}
pub(super) fn require(db: &Connection) -> Result<()> {
    if !present(db)? {
        return Err(SyncError::Schema);
    }
    let version: Option<i64> = db
        .query_row(
            "SELECT version FROM mdreader_sync_graph_schema WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| SyncError::Schema)?;
    let count: u64 = db.query_row("SELECT count(*) FROM mdreader_sync_graph_schema", [], |r| {
        r.get(0)
    })?;
    if version != Some(1) || count != 1 {
        return Err(SyncError::Schema);
    }
    for name in [
        "mdreader_sync_graph_schema",
        "mdreader_sync_graph_roots",
        "mdreader_sync_graph_dependencies",
    ] {
        let expected = SQL
            .split(';')
            .find(|s| s.trim_start().starts_with(&format!("CREATE TABLE {name}(")))
            .ok_or(SyncError::Schema)?;
        let actual: String = db
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name=?1 AND type='table'",
                [name],
                |r| r.get(0),
            )
            .map_err(|_| SyncError::Schema)?;
        if normalize(&actual) != normalize(expected) {
            return Err(SyncError::Schema);
        }
    }
    Ok(())
}
fn normalize(sql: &str) -> String {
    // This namespace's immutable SQL contains no quoted whitespace values.
    sql.trim()
        .trim_end_matches(';')
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect()
}
