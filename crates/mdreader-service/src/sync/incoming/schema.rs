use super::types::*;
use rusqlite::{OptionalExtension, Transaction};

pub(super) fn bind(tx: &Transaction<'_>, library: uuid::Uuid) -> Result<()> {
    if library.get_version_num() != 4 {
        return Err(Error::Scope);
    }
    let existing:u64=tx.query_row("SELECT count(*) FROM sqlite_master WHERE name IN('native_incoming_schema_v1','native_incoming_items_v1')",[],|r|r.get(0))?;
    if existing == 0 {
        tx.execute_batch(include_str!("migrations/001-incoming.sql"))?;
        tx.execute(
            "INSERT INTO native_incoming_schema_v1 VALUES(1,1,?1)",
            [library.to_string()],
        )?;
    }
    initialize(tx)?;
    require_library(tx, &library.to_string())
}

pub(super) fn require_library(tx: &Transaction<'_>, library: &str) -> Result<()> {
    initialize(tx)?;
    let bound: String = tx.query_row(
        "SELECT remote_library FROM native_incoming_schema_v1 WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if bound != library {
        return Err(Error::Scope);
    }
    Ok(())
}

/// Independent ordered namespace; never changes library/index/global versions.
pub(super) fn initialize(tx: &Transaction<'_>) -> Result<()> {
    let exists = |name: &str| -> Result<bool> {
        Ok(tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?)
    };
    if !exists("native_incoming_schema_v1")? {
        return Err(Error::Schema);
    }
    let version: Option<i64> = tx
        .query_row(
            "SELECT version FROM native_incoming_schema_v1 WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Schema)?;
    if !matches!(version, Some(1 | 2)) {
        return Err(Error::Schema);
    }
    let columns = |name: &str| -> Result<Vec<String>> {
        Ok(tx
            .prepare(&format!("PRAGMA table_info({name})"))?
            .query_map([], |r| r.get(1))?
            .collect::<rusqlite::Result<_>>()?)
    };
    if columns("native_incoming_schema_v1")? != ["singleton", "version", "remote_library"]
        || columns("native_incoming_items_v1")?
            != [
                "scope",
                "wire_revision",
                "wire_item",
                "sequence",
                "identity",
                "origin",
                "document_id",
                "state",
                "receipt",
            ]
    {
        return Err(Error::Schema);
    }
    let (rows, library): (u64, String) = tx.query_row(
        "SELECT count(*),remote_library FROM native_incoming_schema_v1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if rows != 1
        || !uuid::Uuid::parse_str(&library)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == library)
    {
        return Err(Error::Schema);
    }
    let invalid: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_incoming_items_v1 WHERE length(CAST(identity AS BLOB))>16384 OR length(CAST(origin AS BLOB))>1024 OR length(CAST(receipt AS BLOB))>32768 OR length(scope)!=64 OR length(wire_item)!=36 OR length(wire_revision)!=36 OR sequence<=0 OR state NOT IN('reserved','applied') OR (state='reserved' AND receipt IS NOT NULL) OR (state='applied' AND (receipt IS NULL OR document_id IS NULL)))",[],|r|r.get(0))?;
    let count: u64 = tx.query_row("SELECT count(*) FROM native_incoming_items_v1", [], |r| {
        r.get(0)
    })?;
    if invalid || count > 4096 {
        return Err(Error::Schema);
    }
    let migration = if version == Some(1) {
        include_str!("migrations/001-incoming.sql")
    } else {
        include_str!("migrations/002-head-history.sql")
    };
    for name in ["native_incoming_schema_v1", "native_incoming_items_v1"] {
        validate_layout(tx, name, migration)?;
    }
    if version == Some(1) {
        tx.execute_batch(include_str!("migrations/002-head-history.sql"))?;
        super::heads::backfill(tx)?;
    }
    validate_layout(
        tx,
        "native_incoming_heads_v2",
        include_str!("migrations/002-head-history.sql"),
    )?;
    let invalid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_incoming_heads_v2 WHERE length(scope)!=64 OR length(wire_item)!=36 OR length(head_revision)!=36 OR length(document_id)!=36 OR (target_item IS NOT NULL AND length(target_item)!=36) OR kind NOT IN('content','user_state') OR (kind='content' AND target_item IS NOT NULL) OR (kind='user_state' AND target_item IS NULL) OR (baseline_generation IS NOT NULL AND (typeof(baseline_generation)!='integer' OR baseline_generation NOT BETWEEN 0 AND 9007199254740991)) OR (tombstone_revision IS NOT NULL AND length(tombstone_revision)!=36))",[],|r|r.get(0))?;
    let count: u64 = tx.query_row("SELECT count(*) FROM native_incoming_heads_v2", [], |r| {
        r.get(0)
    })?;
    if invalid || count > 4096 {
        return Err(Error::Schema);
    }
    Ok(())
}
fn normalized(sql: &str) -> String {
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
        } else if quoted || !c.is_ascii_whitespace() {
            result.push(c);
        }
    }
    result
}

fn validate_layout(tx: &Transaction<'_>, name: &str, migration: &str) -> Result<()> {
    let expected = migration
        .split(';')
        .find(|sql| {
            sql.trim_start()
                .starts_with(&format!("CREATE TABLE {name}("))
        })
        .ok_or(Error::Schema)?;
    let actual: String = tx
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name=?1 AND type='table'",
            [name],
            |r| r.get(0),
        )
        .map_err(|_| Error::Schema)?;
    if normalized(&actual) != normalized(expected) {
        return Err(Error::Schema);
    }
    Ok(())
}
