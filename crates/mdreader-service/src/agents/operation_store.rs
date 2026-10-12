//! Durable exact-operation mapping and pending text for crash recovery.
use super::{invalid, projects::AgentRequest};
use mdreader_core::{ReaderError, ReaderStore, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
const SQL:&str="CREATE TABLE mdreader_agent_operations_v1(id TEXT PRIMARY KEY,grant_id TEXT NOT NULL,request TEXT NOT NULL,state TEXT NOT NULL,remote TEXT,result TEXT)";
#[derive(Serialize, Deserialize)]
pub(super) struct Operation {
    pub request: AgentRequest,
    pub state: String,
    pub remote: Option<Value>,
    pub result: Option<Value>,
}
fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db|->Result<()>{
        let foreign:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE name='mdreader_agent_operations_v1' OR tbl_name='mdreader_agent_operations_v1') OR EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name='mdreader_agent_operations_v1')",[],|r|r.get(0))?;
        if foreign{return Err(invalid());}
        let actual:Option<String>=db.query_row("SELECT sql FROM main.sqlite_schema WHERE name='mdreader_agent_operations_v1' AND type='table'",[],|r|r.get(0)).optional()?;
        match actual{None=>db.execute_batch(SQL)?,Some(sql)if sql==SQL=>{},_=>return Err(invalid())};Ok(())
    })
}
pub(super) fn get(store: &mut ReaderStore, grant: &str, id: &str) -> Result<Option<Operation>> {
    initialize(store)?;
    store.with_library_connection(|db|->Result<_>{
        let value:Option<(String,String,Option<String>,Option<String>)>=db.query_row("SELECT request,state,remote,result FROM mdreader_agent_operations_v1 WHERE id=?1 AND grant_id=?2",params![id,grant],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let decode=|s:&str|serde_json::from_str(s).map_err(|_|invalid());
        value.map(|(request,state,remote,result)|Ok(Operation{request:serde_json::from_str(&request).map_err(|_|invalid())?,state,remote:remote.as_deref().map(decode).transpose()?,result:result.as_deref().map(decode).transpose()?})).transpose()
    })
}
pub(super) fn prepare(
    store: &mut ReaderStore,
    grant: &str,
    id: &str,
    request: &AgentRequest,
) -> Result<Operation> {
    initialize(store)?;
    let request_json = serde_json::to_string(request).map_err(|_| invalid())?;
    if let Some(prior) = get(store, grant, id)? {
        if serde_json::to_string(&prior.request).map_err(|_| invalid())? != request_json {
            return Err(ReaderError::Conflict);
        }
        return Ok(prior);
    }
    store.with_library_connection(|db| -> Result<()> {
        let (count, bytes): (u64, u64) = db.query_row(
            "SELECT count(*),COALESCE(sum(length(request)),0) FROM mdreader_agent_operations_v1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count >= 4096 || bytes + request_json.len() as u64 > 64 * 1024 * 1024 {
            return Err(ReaderError::Unavailable);
        }
        db.execute(
            "INSERT INTO mdreader_agent_operations_v1 VALUES(?1,?2,?3,'prepared',NULL,NULL)",
            params![id, grant, request_json],
        )?;
        Ok(())
    })?;
    get(store, grant, id)?.ok_or(ReaderError::Unavailable)
}
pub(super) fn update(
    store: &mut ReaderStore,
    grant: &str,
    id: &str,
    state: &str,
    remote: Option<&Value>,
    result: Option<&Value>,
) -> Result<()> {
    initialize(store)?;
    let encode = |v: &Value| serde_json::to_string(v).map_err(|_| invalid());
    store.with_library_connection(|db|->Result<()>{
        if db.execute("UPDATE mdreader_agent_operations_v1 SET state=?3,remote=COALESCE(?4,remote),result=COALESCE(?5,result) WHERE id=?1 AND grant_id=?2",params![id,grant,state,remote.map(encode).transpose()?,result.map(encode).transpose()?])?!=1{return Err(ReaderError::Unavailable);}Ok(())
    })
}
