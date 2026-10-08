//! Exact durable graph proofs on the caller-owned transaction, without writes.
use super::*;
/// Root/dependency identity for admission replay. This proves durable ciphertext,
/// not remote publication, authority, selection, or typed application.
pub fn graph_entry_in(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    operation: &str,
) -> Result<OutboxEntry> {
    scope.validate()?;
    require_base(tx)?;
    validate_uuid(operation)?;
    if !graph::has_root(tx, scope, operation)? {
        return Err(SyncError::NotReady);
    }
    graph_read::dependencies_on(tx, scope, operation)?;
    exact_entry(tx, scope, operation)
}
/// Receipt-proven immutable graph. Caller must additionally verify its exact
/// selected native association and staged typed revision before applying it.
pub fn published_graph_in(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    operation: &str,
    change: &RemoteChange,
) -> Result<OutboxEntry> {
    scope.validate()?;
    require_base(tx)?;
    validate_uuid(operation)?;
    if !graph::has_root(tx, scope, operation)? {
        return Err(SyncError::NotReady);
    }
    let entry = exact_entry(tx, scope, operation)?;
    if entry.state != OutboxState::Acknowledged || entry.receipt.is_none() {
        return Err(SyncError::NotReady);
    }
    graph::require_published_on(tx, scope, operation)?;
    let receipt = entry.receipt.as_ref().ok_or(SyncError::Schema)?;
    if change.device != scope.device_id {
        return Err(SyncError::Scope);
    }
    if entry.operation.item_id != change.item_id
        || entry.operation.revision_id != change.revision_id
        || entry.operation.blob != change.blob
        || entry.operation.deleted != change.deleted
        || entry.operation.parents != change.parents
        || receipt.sequence != change.sequence
    {
        return Err(SyncError::Reused);
    }
    if receipt.conflict || receipt.heads != [change.revision_id.clone()] {
        return Err(SyncError::NotReady);
    }
    Ok(entry)
}

/// Observe an exact local operation before considering a self-authored incoming
/// root. A missing native association must never trigger a duplicate import.
/// This is not a graph/publication proof and never admits legacy weak receipts.
pub fn outbox_revision_in(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    revision: &str,
) -> Result<Option<OutboxEntry>> {
    scope.validate()?;
    require_base(tx)?;
    validate_uuid(revision)?;
    if graph_schema::present(tx)? {
        graph_schema::require(tx)?;
    }
    let (count,valid):(u64,bool)=tx.query_row("SELECT count(*),coalesce(min(typeof(operation_id)='text' AND length(CAST(operation_id AS BLOB))=36),1) FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND revision_id=?4",params![scope.account_digest,scope.library_id,scope.device_id,revision],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count > 1 || !valid {
        return Err(SyncError::Schema);
    }
    if count == 0 {
        return Ok(None);
    }
    let operation:String=tx.query_row("SELECT operation_id FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND revision_id=?4 AND length(CAST(operation_id AS BLOB))=36",params![scope.account_digest,scope.library_id,scope.device_id,revision],|r|r.get(0))?;
    validate_uuid(&operation)?;
    let entry = exact_entry(tx, scope, &operation)?;
    if entry.operation.revision_id != revision {
        return Err(SyncError::Schema);
    }
    Ok(Some(entry))
}
fn exact_entry(tx: &Transaction<'_>, scope: &SyncScope, operation: &str) -> Result<OutboxEntry> {
    let entry = entry_on(tx, scope, operation)?.ok_or(SyncError::Schema)?;
    if entry.operation.operation_id != operation {
        return Err(SyncError::Schema);
    }
    Ok(entry)
}
fn require_base(tx: &Transaction<'_>) -> Result<()> {
    let (count, version): (u64, Option<i64>) = tx
        .query_row(
            "SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM mdreader_sync_meta",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| SyncError::Schema)?;
    if count != 1 || version != Some(VERSION) {
        return Err(SyncError::Schema);
    }
    Ok(())
}
