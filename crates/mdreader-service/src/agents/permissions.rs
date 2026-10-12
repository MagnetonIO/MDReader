//! Additive reporting/edit grants. Old read grants are preserved unchanged.
use super::{grants, invalid};
use mdreader_core::{ReaderStore, Result};
use rusqlite::{params, OptionalExtension};
#[derive(Clone)]
pub(crate) struct WriteFence {
    pub grant_id: String,
    pub source_id: String,
    pub generation: u64,
}
impl WriteFence {
    pub(crate) fn check(&self, db: &rusqlite::Connection) -> mdreader_provider::Result<()> {
        use mdreader_provider::ProviderError;
        let fail = |_: rusqlite::Error| ProviderError::Unauthorized;
        let row:(bool,String,u64,String)=db.query_row("SELECT s.enabled,p.edits,s.generation,s.record FROM mdreader_agent_setup s JOIN mdreader_agent_permissions_v1 p ON p.grant_id=s.id WHERE s.id=?1",[&self.grant_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(fail)?;
        let edits: Vec<String> =
            serde_json::from_str(&row.1).map_err(|_| ProviderError::Unauthorized)?;
        let record: super::Record =
            serde_json::from_str(&row.3).map_err(|_| ProviderError::Unauthorized)?;
        if !row.0
            || row.2 != self.generation
            || record.phase != "installed"
            || !edits.contains(&self.source_id)
        {
            return Err(ProviderError::Unauthorized);
        }
        record
            .selection
            .checked()
            .map_err(|_| ProviderError::Unauthorized)?;
        let enabled: bool = db
            .query_row(
                "SELECT enabled FROM sources WHERE id=?1",
                [&self.source_id],
                |r| r.get(0),
            )
            .map_err(fail)?;
        if !enabled {
            return Err(ProviderError::Unauthorized);
        }
        Ok(())
    }
}
#[derive(Default)]
pub(super) struct Permissions {
    pub projects: Vec<String>,
    pub edits: Vec<String>,
    pub last_seen_ms: Option<i64>,
}
const TABLE:&str="CREATE TABLE mdreader_agent_permissions_v1(grant_id TEXT PRIMARY KEY,projects TEXT NOT NULL,edits TEXT NOT NULL,last_seen_ms INTEGER)";
fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db|->Result<()> {
        let foreign:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE name='mdreader_agent_permissions_v1' OR tbl_name='mdreader_agent_permissions_v1') OR EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name='mdreader_agent_permissions_v1')",[],|r|r.get(0))?;
        if foreign{return Err(invalid());}
        let sql:Option<String>=db.query_row("SELECT sql FROM main.sqlite_schema WHERE name='mdreader_agent_permissions_v1' AND type='table'",[],|r|r.get(0)).optional()?;
        match sql {None=>{db.execute_batch(TABLE)?;},Some(sql) if sql==TABLE=>{},_=>return Err(invalid())}
        Ok(())
    })
}
pub(super) fn get(store: &mut ReaderStore, id: &str) -> Result<Permissions> {
    initialize(store)?;
    store.with_library_connection(|db|->Result<_>{
        let row:Option<(String,String,Option<i64>)>=db.query_row("SELECT projects,edits,last_seen_ms FROM mdreader_agent_permissions_v1 WHERE grant_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        match row {Some((p,e,last_seen_ms))=>Ok(Permissions{projects:serde_json::from_str(&p).map_err(|_|invalid())?,edits:serde_json::from_str(&e).map_err(|_|invalid())?,last_seen_ms}),None=>Ok(Permissions::default())}
    })
}
pub(super) fn set(
    store: &mut ReaderStore,
    id: &str,
    enabled: bool,
    projects: &[String],
    reads: &[String],
    edits: &[String],
) -> Result<()> {
    initialize(store)?;
    // Initialize and validate the existing grant namespace before the shared tx.
    grants::grant(store, id)?;
    let encode = |v: &[String]| serde_json::to_string(v).map_err(|_| invalid());
    let (projects, reads, edits) = (encode(projects)?, encode(reads)?, encode(edits)?);
    store.with_library_connection(|db|->Result<()> {
        let tx=db.transaction()?;
        if tx.execute("UPDATE mdreader_agent_setup SET enabled=?2,sources=?3,generation=generation+1 WHERE id=?1",params![id,enabled,reads])?!=1{return Err(invalid());}
        tx.execute("INSERT INTO mdreader_agent_permissions_v1 VALUES(?1,?2,?3,NULL) ON CONFLICT(grant_id) DO UPDATE SET projects=excluded.projects,edits=excluded.edits,last_seen_ms=NULL",params![id,projects,edits])?;
        tx.commit()?;Ok(())
    })
}
pub(super) fn touch(store: &mut ReaderStore, id: &str) -> Result<()> {
    initialize(store)?;
    let now = mdreader_core::agent_sessions::now_ms()?;
    store.with_library_connection(|db|->Result<()>{db.execute("INSERT INTO mdreader_agent_permissions_v1 VALUES(?1,'[]','[]',?2) ON CONFLICT(grant_id) DO UPDATE SET last_seen_ms=excluded.last_seen_ms",params![id,now])?;Ok(())})
}
