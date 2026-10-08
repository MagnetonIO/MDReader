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
    if version != Some(1) {
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
    let invalid: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_incoming_items_v1 WHERE length(identity)>16384 OR length(origin)>1024 OR length(receipt)>32768 OR length(scope)!=64 OR length(wire_item)!=36 OR length(wire_revision)!=36 OR sequence<=0 OR state NOT IN('reserved','applied') OR (state='reserved' AND receipt IS NOT NULL) OR (state='applied' AND (receipt IS NULL OR document_id IS NULL)))",[],|r|r.get(0))?;
    let count: u64 = tx.query_row("SELECT count(*) FROM native_incoming_items_v1", [], |r| {
        r.get(0)
    })?;
    if invalid || count > 4096 {
        return Err(Error::Schema);
    }
    Ok(())
}
