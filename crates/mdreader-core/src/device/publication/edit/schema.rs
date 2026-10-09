use crate::device::{Error, Result};
use rusqlite::{OptionalExtension, Transaction};
const MIGRATION: &str = include_str!("../migrations/001-local-edits.sql");
fn normalized(sql: &str) -> String {
    sql.trim()
        .trim_end_matches(';')
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}
pub(super) fn initialize(tx: &Transaction<'_>) -> Result<()> {
    let expected: Vec<_> = MIGRATION.split(';').take(2).collect();
    let mut found = 0;
    for (index, name) in ["device_edit_schema_v1", "device_edits_v1"]
        .iter()
        .enumerate()
    {
        let row: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT type,sql FROM sqlite_schema WHERE name=?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((kind, sql)) = row {
            if kind != "table"
                || sql.as_deref().map(normalized) != Some(normalized(expected[index]))
            {
                return Err(Error::InvalidSchema);
            }
            found += 1;
        }
    }
    if found == 0 {
        tx.execute_batch(MIGRATION)?;
    } else if found != 2 {
        return Err(Error::InvalidSchema);
    }
    let (count, version): (u64, Option<i64>) = tx.query_row(
        "SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM device_edit_schema_v1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || version.is_none() {
        return Err(Error::InvalidSchema);
    }
    if version != Some(1) {
        return Err(Error::UnsupportedSchema);
    }
    Ok(())
}
