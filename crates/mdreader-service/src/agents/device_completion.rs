//! Preserve only this issuing connection's exact immutable document permission.
use super::WriteFence;
use mdreader_core::device::{
    catalog,
    publication::{PublicationError, PublicationResult},
    DeviceDocument,
};
use rusqlite::{params, Transaction};
use serde_json::json;

pub(crate) fn complete(
    tx: &Transaction<'_>,
    fence: &WriteFence,
    operation: &str,
    document: &DeviceDocument,
    replace: bool,
) -> PublicationResult<()> {
    fence
        .check_grant(tx)
        .map_err(|_| PublicationError::Conflict)?;
    let enabled: bool = tx.query_row(
        "SELECT enabled FROM sources WHERE id=?1",
        [&document.source_id],
        |r| r.get(0),
    )?;
    if !enabled {
        return Err(PublicationError::Conflict);
    }
    let (reads,edits):(String,String)=tx.query_row("SELECT s.sources,p.edits FROM mdreader_agent_setup s JOIN mdreader_agent_permissions_v1 p ON p.grant_id=s.id WHERE s.id=?1",[&fence.grant_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let transfer = |body: String| -> PublicationResult<String> {
        let mut ids: Vec<String> = serde_json::from_str(&body)?;
        if ids.contains(&fence.source_id) {
            if replace {
                ids.retain(|s| s != &fence.source_id);
            }
            if !ids.contains(&document.source_id) {
                ids.push(document.source_id.clone());
            }
        }
        Ok(serde_json::to_string(&ids)?)
    };
    let reads = transfer(reads)?;
    let edits = transfer(edits)?;
    if tx.execute("UPDATE mdreader_agent_setup SET sources=?2,generation=generation+1 WHERE id=?1 AND generation=?3 AND enabled=1",params![fence.grant_id,reads,fence.generation])?!=1{return Err(PublicationError::Conflict);}
    tx.execute(
        "UPDATE mdreader_agent_permissions_v1 SET edits=?2 WHERE grant_id=?1",
        params![fence.grant_id, edits],
    )?;
    let result = json!({"operation_id":operation,"state":"saved","document":{"operation":"saved","receipt":catalog::receipt(document),"name":document.name}});
    if tx.execute("UPDATE mdreader_agent_operations_v1 SET state='saved',result=?3 WHERE id=?1 AND grant_id=?2 AND state IN ('prepared','dispatched')",params![operation,fence.grant_id,serde_json::to_string(&result)?])?!=1{return Err(PublicationError::Conflict);}
    Ok(())
}
