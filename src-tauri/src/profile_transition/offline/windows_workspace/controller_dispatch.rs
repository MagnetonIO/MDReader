//! Never relaunch an original after an unknown controller-lifetime dispatch.
use super::*;
use rusqlite::{params, Connection, OptionalExtension};
const NAME: &str = "native_windows_return_dispatch_v1";
const DDL:&str="CREATE TABLE native_windows_return_dispatch_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),operation TEXT NOT NULL CHECK(length(operation)=36),phase TEXT NOT NULL CHECK(phase IN('requested','failed','launched')))";
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
        let sql=format!("SELECT EXISTS(SELECT 1 FROM {namespace} WHERE (name GLOB 'native_windows_return_dispatch_*' OR tbl_name GLOB 'native_windows_return_dispatch_*') AND NOT(?2='sqlite_schema' AND type='table' AND name=?1))");
        if db
            .query_row(&sql, [NAME, namespace], |r| r.get::<_, bool>(0))
            .map_err(|_| Error::Storage)?
        {
            return Err(Error::Conflict);
        }
    }
    Ok(definition.is_some())
}
pub(super) fn begin(db: &mut Connection, operation: Uuid) -> Result<()> {
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    let transition = observe(&tx, operation)?.0;
    if transition.state != State::RolledBack {
        return Err(Error::Conflict);
    }
    if !schema(&tx)? {
        tx.execute_batch(DDL).map_err(|_| Error::Storage)?;
    }
    let previous:Option<(String,String)>=tx.query_row("SELECT operation,phase FROM native_windows_return_dispatch_v1 WHERE singleton=1 AND version=1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|Error::Storage)?;
    if let Some((id, phase)) = previous {
        if id != operation.to_string() || phase != "failed" {
            return Err(Error::ReconciliationRequired);
        }
        if tx.execute("UPDATE native_windows_return_dispatch_v1 SET phase='requested' WHERE singleton=1 AND operation=?1 AND phase='failed'",[id]).map_err(|_|Error::Storage)?!=1 {return Err(Error::Conflict);}
    } else {
        tx.execute(
            "INSERT INTO native_windows_return_dispatch_v1 VALUES(1,1,?1,'requested')",
            params![operation.to_string()],
        )
        .map_err(|_| Error::Storage)?;
    }
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}
pub(super) fn finish(db: &mut Connection, operation: Uuid, launched: bool) -> Result<()> {
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    if !schema(&tx)? {
        return Err(Error::Conflict);
    }
    if tx.execute("UPDATE native_windows_return_dispatch_v1 SET phase=?2 WHERE singleton=1 AND operation=?1 AND phase='requested'",params![operation.to_string(),if launched {"launched"}else{"failed"}]).map_err(|_|Error::Storage)?!=1 {return Err(Error::Conflict);}
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}
