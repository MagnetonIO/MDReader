use super::*;
use rusqlite::{Connection, OptionalExtension};
const LIBRARY: &str = include_str!("migrations/001-library.sql");
const INSERT: &str = include_str!("migrations/002-insert.sql");
const UPDATE: &str = include_str!("migrations/003-update.sql");
const DELETE: &str = include_str!("migrations/004-delete.sql");
const INDEX: &str = include_str!("migrations/005-index.sql");
fn definitions(index: bool) -> Vec<(&'static str, &'static str, &'static str)> {
    if index {
        vec![
            (
                "reader_catalog_index_schema_v1",
                "table",
                INDEX.split(';').next().unwrap_or(""),
            ),
            (
                "reader_catalog_snapshots_v1",
                "table",
                INDEX.split(';').nth(1).unwrap_or(""),
            ),
        ]
    } else {
        vec![
            (
                "reader_catalog_schema_v1",
                "table",
                LIBRARY.split(';').next().unwrap_or(""),
            ),
            (
                "reader_catalog_authorities_v1",
                "table",
                LIBRARY.split(';').nth(1).unwrap_or(""),
            ),
            ("reader_catalog_insert_v1", "trigger", INSERT),
            ("reader_catalog_update_v1", "trigger", UPDATE),
            ("reader_catalog_delete_v1", "trigger", DELETE),
        ]
    }
}
pub(crate) fn initialize(library: &Connection, index: &Connection) -> Result<()> {
    for (db, derived) in [(library, false), (index, true)] {
        let expected = definitions(derived);
        let found: u64 = db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_catalog_*'",
            [],
            |r| r.get(0),
        )?;
        if found == 0 {
            let tx = db.unchecked_transaction()?;
            if derived {
                tx.execute_batch(INDEX)?;
                tx.execute("INSERT INTO reader_catalog_index_schema_v1 VALUES(1,1)", [])?;
                // Preserve already indexed rows, without claiming a known scan time.
                tx.execute("INSERT INTO reader_catalog_snapshots_v1 SELECT DISTINCT source_id,lower(hex(randomblob(16))),NULL FROM documents GROUP BY source_id",[])?;
            } else {
                tx.execute_batch(LIBRARY)?;
                tx.execute_batch(INSERT)?;
                tx.execute_batch(UPDATE)?;
                tx.execute_batch(DELETE)?;
                tx.execute("INSERT INTO reader_catalog_schema_v1 VALUES(1,1)", [])?;
                tx.execute("INSERT INTO reader_catalog_authorities_v1 SELECT id,lower(hex(randomblob(16))) FROM sources",[])?;
            }
            validate(&tx, derived)?;
            tx.commit()?;
        } else if found != expected.len() as u64 {
            return Err(ReaderError::Invalid("Unsupported catalog namespace".into()));
        }
        validate(db, derived)?;
    }
    Ok(())
}
pub(crate) fn validate(db: &Connection, index: bool) -> Result<()> {
    let found: u64 = db.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_catalog_*'",
        [],
        |r| r.get(0),
    )?;
    if found != definitions(index).len() as u64 {
        return Err(ReaderError::Invalid("Unsupported catalog namespace".into()));
    }
    for (name, kind, expected) in definitions(index) {
        let actual: Option<(String, String)> = db
            .query_row(
                "SELECT type,sql FROM sqlite_schema WHERE name=?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if actual.is_none_or(|(actual_kind, sql)| {
            actual_kind != kind || normal(&sql) != normal(expected)
        }) {
            return Err(ReaderError::Invalid("Unsupported catalog namespace".into()));
        }
    }
    let marker = if index {
        "reader_catalog_index_schema_v1"
    } else {
        "reader_catalog_schema_v1"
    };
    let (count, version): (u64, Option<i64>) = db.query_row(
        &format!("SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM {marker}"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || version != Some(1) {
        return Err(ReaderError::Invalid("Unsupported catalog namespace".into()));
    }
    Ok(())
}
/// Source mutation admits only this reader's fully verified catalog triggers.
/// TEMP triggers and foreign main triggers remain invalid producer authority.
#[cfg(all(windows, feature = "device-assets"))]
pub(crate) fn validate_source_triggers(db: &Connection) -> Result<()> {
    validate(db, false)?;
    let foreign: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger' AND tbl_name='sources' AND name NOT IN('reader_catalog_insert_v1','reader_catalog_update_v1','reader_catalog_delete_v1')) OR EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE type='trigger' AND tbl_name='sources')",
        [], |r| r.get(0),
    )?;
    if foreign {
        return Err(ReaderError::Invalid("Unsupported source producer".into()));
    }
    Ok(())
}
fn normal(sql: &str) -> String {
    let mut result = String::new();
    let mut quoted = false;
    let mut chars = sql.trim().trim_end_matches(';').chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            result.push(c);
            if quoted && chars.peek() == Some(&'\'') {
                result.push(chars.next().unwrap_or('\''));
            } else {
                quoted = !quoted;
            }
        } else if quoted {
            result.push(c);
        } else if !c.is_ascii_whitespace() {
            result.push(c.to_ascii_lowercase());
        }
    }
    result
}
/// One update at the derived index transaction's successful publication boundary.
pub(crate) fn publish(tx: &rusqlite::Transaction<'_>, source: &str) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ReaderError::Unavailable)?
        .as_millis();
    let now = i64::try_from(now).map_err(|_| ReaderError::Unavailable)?;
    tx.execute("INSERT INTO reader_catalog_snapshots_v1 VALUES(?1,lower(hex(randomblob(16))),?2) ON CONFLICT(source_id) DO UPDATE SET snapshot=excluded.snapshot,indexed_at_ms=excluded.indexed_at_ms",rusqlite::params![source,now])?;
    Ok(())
}
