use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Transaction};
const SQL: &str = include_str!("migrations/001-selection.sql");
const TABLES: [&str; 4] = [
    "profile_transition_schema_v1",
    "profile_selection_v1",
    "profile_transitions_v1",
    "profile_activation_receipts_v1",
];
fn normalized(s: &str) -> String {
    let mut quoted = false;
    s.chars()
        .filter(|c| {
            if *c == '\'' {
                quoted = !quoted;
            }
            quoted || !c.is_ascii_whitespace()
        })
        .collect()
}
pub(super) fn initialize(connection: &mut Connection) -> Result<()> {
    let tx = connection.transaction().map_err(|_| Error::Storage)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [TABLES[0]],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    if existing.is_none() {
        for name in TABLES {
            let found: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
                    [name],
                    |r| r.get(0),
                )
                .map_err(|_| Error::Storage)?;
            if found {
                return Err(Error::Invalid);
            }
        }
        tx.execute_batch(SQL).map_err(|_| Error::Storage)?;
    }
    validate(&tx)?;
    tx.commit().map_err(|_| Error::Storage)
}
pub(super) fn validate(tx: &Transaction<'_>) -> Result<()> {
    for schema in ["sqlite_schema", "sqlite_temp_schema"] {
        let sql = format!("SELECT type,name,tbl_name,sql FROM {schema} WHERE name GLOB 'profile_transition*' OR tbl_name GLOB 'profile_transition*' OR name GLOB 'profile_selection_*' OR tbl_name GLOB 'profile_selection_*' OR name GLOB 'profile_activation_*' OR tbl_name GLOB 'profile_activation_*'");
        let mut query = tx.prepare(&sql).map_err(|_| Error::Storage)?;
        let rows = query
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|_| Error::Storage)?;
        for row in rows {
            let (kind, name, table, sql) = row.map_err(|_| Error::Storage)?;
            if schema == "sqlite_schema"
                && ((kind == "table" && name == table && TABLES.contains(&name.as_str()))
                    || (kind == "index" && sql.is_none() && TABLES.contains(&table.as_str())))
            {
                continue;
            }
            return Err(Error::Invalid);
        }
    }
    let version: Option<u64> = tx
        .query_row(
            "SELECT version FROM profile_transition_schema_v1 WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Invalid)?;
    if version != Some(1) {
        return Err(Error::Invalid);
    }
    for name in TABLES {
        let found: String = tx
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .map_err(|_| Error::Invalid)?;
        let expected = SQL
            .split(';')
            .find(|s| s.trim_start().starts_with(&format!("CREATE TABLE {name}(")))
            .ok_or(Error::Invalid)?;
        if normalized(&found) != normalized(expected) {
            return Err(Error::Invalid);
        }
    }
    let count: u64 = tx
        .query_row("SELECT COUNT(*) FROM profile_transitions_v1", [], |r| {
            r.get(0)
        })
        .map_err(|_| Error::Storage)?;
    if count > 1024 {
        return Err(Error::Bounds);
    }
    let oversized: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM profile_transitions_v1 \
         WHERE length(CAST(original AS BLOB))>131072 \
         OR length(CAST(candidate AS BLOB))>262144 \
         OR length(CAST(snapshot AS BLOB))>1024) \
         OR EXISTS(SELECT 1 FROM profile_selection_v1 \
         WHERE length(CAST(root AS BLOB))>131072) \
         OR EXISTS(SELECT 1 FROM profile_activation_receipts_v1 \
         WHERE length(CAST(selected_root AS BLOB))>131072)",
            [],
            |r| r.get(0),
        )
        .map_err(|_| Error::Storage)?;
    if oversized {
        return Err(Error::Bounds);
    }
    Ok(())
}
