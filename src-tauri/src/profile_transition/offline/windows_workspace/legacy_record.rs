use super::*;
use rusqlite::{params, Connection, OptionalExtension};
pub(super) const NAME: &str = "native_windows_legacy_v1";
const DDL: &str = "CREATE TABLE native_windows_legacy_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),record TEXT NOT NULL CHECK(length(CAST(record AS BLOB))<=262144))";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub(super) operation: Uuid,
    pub(super) launch: mdreader_profile::legacy_windows::LaunchRecord,
}
fn schema(db: &Connection) -> Result<bool> {
    let definition: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [NAME],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    let normal = |s: &str| {
        s.chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>()
    };
    if definition
        .as_ref()
        .is_some_and(|s| normal(s) != normal(DDL))
    {
        return Err(Error::Conflict);
    }
    for namespace in ["sqlite_schema", "sqlite_temp_schema"] {
        let query = format!("SELECT EXISTS(SELECT 1 FROM {namespace} WHERE (name GLOB 'native_windows_legacy_*' OR tbl_name GLOB 'native_windows_legacy_*') AND NOT(type='table' AND name=?1))");
        if db
            .query_row(&query, [NAME], |r| r.get::<_, bool>(0))
            .map_err(|_| Error::Storage)?
        {
            return Err(Error::Conflict);
        }
    }
    Ok(definition.is_some())
}
pub(super) fn read(db: &Connection) -> Result<Option<Record>> {
    if !schema(db)? {
        return Ok(None);
    }
    let count: u64 = db
        .query_row("SELECT count(*) FROM native_windows_legacy_v1", [], |r| {
            r.get(0)
        })
        .map_err(|_| Error::Storage)?;
    if count != 1 {
        return Err(Error::Conflict);
    }
    let text: String = db.query_row("SELECT record FROM native_windows_legacy_v1 WHERE singleton=1 AND version=1 AND length(CAST(record AS BLOB))<=262144", [], |r| r.get(0)).map_err(|_| Error::Conflict)?;
    let record: Record = serde_json::from_str(&text).map_err(|_| Error::Integrity)?;
    if record.operation.get_version_num() != 4 {
        return Err(Error::Integrity);
    }
    record.launch.validate()?;
    Ok(Some(record))
}
pub(super) fn record(
    db: &mut Connection,
    operation: Uuid,
    launch: &mdreader_profile::legacy_windows::LaunchRecord,
) -> Result<()> {
    launch.validate()?;
    let record = Record {
        operation,
        launch: launch.clone(),
    };
    let text = serde_json::to_string(&record).map_err(|_| Error::Invalid)?;
    if text.len() > 262144 {
        return Err(Error::Bounds);
    }
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    if !schema(&tx)? {
        tx.execute_batch(DDL).map_err(|_| Error::Storage)?;
    }
    tx.execute(
        "INSERT INTO native_windows_legacy_v1 VALUES(1,1,?1)",
        params![text],
    )
    .map_err(|_| Error::Storage)?;
    let persisted = read(&tx)?.ok_or(Error::Conflict)?;
    if serde_json::to_vec(&persisted).map_err(|_| Error::Invalid)?
        != serde_json::to_vec(&record).map_err(|_| Error::Invalid)?
    {
        return Err(Error::Conflict);
    }
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}
