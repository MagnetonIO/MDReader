//! Canonical bounded native operation association on the actual selector owner.
use super::*;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
const NAME: &str = "native_windows_selection_v1";
const DDL: &str = "CREATE TABLE native_windows_selection_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),native_operation TEXT NOT NULL CHECK(length(native_operation)=36),receipt TEXT NOT NULL CHECK(length(CAST(receipt AS BLOB))<=262144),digest TEXT NOT NULL CHECK(length(digest)=64))";
fn schema(db: &Connection) -> Result<bool> {
    let definition: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [NAME],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    let normal = |value: &str| {
        value
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>()
    };
    if definition
        .as_ref()
        .is_some_and(|actual| normal(actual) != normal(DDL))
    {
        return Err(Error::Conflict);
    }
    for namespace in ["sqlite_schema", "sqlite_temp_schema"] {
        let sql = format!("SELECT EXISTS(SELECT 1 FROM {namespace} WHERE (name GLOB 'native_windows_selection_*' OR tbl_name GLOB 'native_windows_selection_*') AND NOT(?2='sqlite_schema' AND type='table' AND name=?1))");
        if db
            .query_row(&sql, [NAME, namespace], |r| r.get::<_, bool>(0))
            .map_err(|_| Error::Storage)?
        {
            return Err(Error::Conflict);
        }
    }
    Ok(definition.is_some())
}
pub(super) fn present(db: &Connection) -> Result<bool> {
    schema(db)
}
pub(super) fn record(db: &mut Connection, native: Uuid, receipt: &CandidateReceipt) -> Result<()> {
    receipt.validate()?;
    let text = serde_json::to_string(receipt).map_err(|_| Error::Invalid)?;
    if text.len() > 262144 {
        return Err(Error::Bounds);
    }
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    if !schema(&tx)? {
        tx.execute_batch(DDL).map_err(|_| Error::Storage)?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO native_windows_selection_v1 VALUES(1,1,?1,?2,?3)",
        params![native.to_string(), text, digest],
    )
    .map_err(|_| Error::Storage)?;
    let actual = read(&tx)?;
    if actual.0 != native
        || serde_json::to_vec(&actual.1).map_err(|_| Error::Invalid)?
            != serde_json::to_vec(receipt).map_err(|_| Error::Invalid)?
    {
        return Err(Error::Conflict);
    }
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}
pub(super) fn read(db: &Connection) -> Result<(Uuid, CandidateReceipt)> {
    if !schema(db)? {
        return Err(Error::Invalid);
    }
    let count: u64 = db
        .query_row(
            "SELECT count(*) FROM native_windows_selection_v1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| Error::Storage)?;
    if count != 1 {
        return Err(Error::Conflict);
    }
    let (native, text, digest): (String, String, String) = db.query_row("SELECT native_operation,receipt,digest FROM native_windows_selection_v1 WHERE singleton=1 AND version=1 AND length(native_operation)=36 AND length(CAST(receipt AS BLOB))<=262144 AND length(digest)=64", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_| Error::Conflict)?;
    if format!("{:x}", Sha256::digest(text.as_bytes())) != digest {
        return Err(Error::Integrity);
    }
    let receipt: CandidateReceipt = serde_json::from_str(&text).map_err(|_| Error::Integrity)?;
    receipt.validate()?;
    Ok((
        Uuid::parse_str(&native).map_err(|_| Error::Integrity)?,
        receipt,
    ))
}
