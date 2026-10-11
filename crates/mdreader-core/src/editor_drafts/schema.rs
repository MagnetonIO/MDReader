//! An additive durable namespace. Existing libraries need no index rebuild.
use crate::{ReaderError, Result};
use rusqlite::{Connection, OptionalExtension};
const V1: &str = include_str!("migrations/001.sql");
fn unavailable() -> ReaderError {
    ReaderError::Unavailable
}
fn normal(s: &str) -> String {
    s.trim()
        .trim_end_matches(';')
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect()
}
pub(super) fn check(db: &Connection) -> Result<bool> {
    let main: u64 = db.query_row("SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_editor_draft*' OR (type='trigger' AND tbl_name IN ('reader_editor_drafts_v1','reader_editor_draft_schema_v1'))", [], |r| r.get(0))?;
    let temp: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name GLOB 'reader_editor_draft*' OR (type='trigger' AND tbl_name IN ('reader_editor_drafts_v1','reader_editor_draft_schema_v1')))", [], |r| r.get(0))?;
    if temp || (main != 0 && main != 2) {
        return Err(unavailable());
    }
    if main == 0 {
        return Ok(false);
    }
    for (name, position) in [
        ("reader_editor_draft_schema_v1", 0),
        ("reader_editor_drafts_v1", 2),
    ] {
        let actual: Option<String> = db
            .query_row(
                "SELECT sql FROM main.sqlite_schema WHERE name=?1 AND type='table'",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        if actual.is_none_or(|s| normal(&s) != normal(V1.split(';').nth(position).unwrap_or(""))) {
            return Err(unavailable());
        }
    }
    let versions = db
        .prepare("SELECT singleton,version FROM main.reader_editor_draft_schema_v1")?
        .query_map([], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if versions != [(1, 1)] {
        return Err(unavailable());
    }
    Ok(true)
}
pub(super) fn initialize(db: &Connection) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    if !check(&tx)? {
        tx.execute_batch(V1)?;
    }
    check(&tx)?;
    tx.commit()?;
    Ok(())
}
