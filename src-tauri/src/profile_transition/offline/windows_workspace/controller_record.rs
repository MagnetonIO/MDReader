//! Discovery data is private, durable metadata; only a live server owns closure.
use super::*;
use rusqlite::{params, Connection, OptionalExtension};
const NAME: &str = "native_windows_controller_v1";
const DDL: &str = "CREATE TABLE native_windows_controller_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),record TEXT NOT NULL CHECK(length(CAST(record AS BLOB))<=262144))";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub(super) operation: Uuid,
    pub(super) generation: u64,
    pub(super) epoch: Uuid,
    pub(super) root: mdreader_profile::RootBinding,
    pub(super) port: u16,
    pub(super) token: String,
}
impl Record {
    pub(super) fn validate(&self) -> Result<()> {
        self.root.validate()?;
        if self.operation.get_version_num() != 4
            || self.epoch.get_version_num() != 4
            || self.port == 0
            || self.token.len() != 64
            || !self
                .token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Integrity);
        }
        Ok(())
    }
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
        .is_some_and(|sql| normal(sql) != normal(DDL))
    {
        return Err(Error::Conflict);
    }
    for namespace in ["sqlite_schema", "sqlite_temp_schema"] {
        let query=format!("SELECT EXISTS(SELECT 1 FROM {namespace} WHERE (name GLOB 'native_windows_controller_*' OR tbl_name GLOB 'native_windows_controller_*') AND NOT(?2='sqlite_schema' AND type='table' AND name=?1))");
        if db
            .query_row(&query, [NAME, namespace], |r| r.get::<_, bool>(0))
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
        .query_row(
            "SELECT count(*) FROM native_windows_controller_v1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| Error::Storage)?;
    if count != 1 {
        return Err(Error::Conflict);
    }
    let text:String=db.query_row("SELECT record FROM native_windows_controller_v1 WHERE singleton=1 AND version=1 AND length(CAST(record AS BLOB))<=262144",[],|r|r.get(0)).map_err(|_|Error::Integrity)?;
    let record: Record = serde_json::from_str(&text).map_err(|_| Error::Integrity)?;
    record.validate()?;
    Ok(Some(record))
}
/// Caller already owns the controller byte lock and the actual ClosedOriginal.
/// An endpoint replacement therefore cannot substitute serialized closure data.
pub(super) fn publish(db: &mut Connection, record: &Record) -> Result<()> {
    record.validate()?;
    let text = serde_json::to_string(record).map_err(|_| Error::Invalid)?;
    if text.len() > 262144 {
        return Err(Error::Bounds);
    }
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    if !schema(&tx)? {
        tx.execute_batch(DDL).map_err(|_| Error::Storage)?;
    }
    let transition = observe(&tx, record.operation)?.0;
    if transition.state != State::Active
        || transition.activation_generation != Some(record.generation)
    {
        return Err(Error::Conflict);
    }
    tx.execute("INSERT INTO native_windows_controller_v1 VALUES(1,1,?1) ON CONFLICT(singleton) DO UPDATE SET record=excluded.record",params![text]).map_err(|_|Error::Storage)?;
    if serde_json::to_vec(&read(&tx)?.ok_or(Error::Conflict)?).map_err(|_| Error::Invalid)?
        != serde_json::to_vec(record).map_err(|_| Error::Invalid)?
    {
        return Err(Error::Conflict);
    }
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}
