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
    let snapshot = graph_read::snapshot_on(tx, scope, operation)?;
    if snapshot.root.entry.operation.operation_id != operation
        || snapshot.root.indexed_revision != snapshot.root.entry.operation.revision_id
    {
        return Err(SyncError::Schema);
    }
    Ok(snapshot.root.entry)
}
/// An immutable graph snapshot borrowing its exact caller-owned transaction.
/// Not Clone/Debug/Serialize; it cannot certify another transaction or survive
/// commit. Final validation compares raw bytes in SQLite without extracting graph bodies
/// again, hashing, JSON decoding, source/session checks or writes.
pub struct PublishedGraphProof<'tx, 'connection> {
    transaction: &'tx Transaction<'connection>,
    scope: SyncScope,
    graph: graph_read::Snapshot,
    schema_version: u64,
}
impl PublishedGraphProof<'_, '_> {
    pub fn entry(&self) -> &OutboxEntry {
        &self.graph.root.entry
    }
    /// Mandatory after all caller writes and immediately before final native
    /// archive/session fences. Even SQL trigger changes must match exact bytes.
    pub fn revalidate(&self) -> Result<()> {
        super::graph_revalidate::revalidate(
            self.transaction,
            &self.scope,
            &self.graph,
            self.schema_version,
        )
    }
}
/// Validate and retain raw root/dependency/receipt bytes in one existing read/
/// hash pass. Caller must retain this proof through its final same-tx fence.
pub fn published_graph_proof_in<'tx, 'connection>(
    tx: &'tx Transaction<'connection>,
    scope: &SyncScope,
    operation: &str,
    change: &RemoteChange,
) -> Result<PublishedGraphProof<'tx, 'connection>> {
    scope.validate()?;
    require_base(tx)?;
    validate_uuid(operation)?;
    let graph = graph_read::snapshot_on(tx, scope, operation)?;
    let entry = &graph.root.entry;
    let count:u64=tx.query_row("SELECT count(*) FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4",params![scope.account_digest,scope.library_id,scope.device_id,operation],|r|r.get(0))?;
    let exact_scope:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND typeof(account)='text' AND CAST(account AS BLOB)=CAST(?1 AS BLOB) AND typeof(library)='text' AND CAST(library AS BLOB)=CAST(?2 AS BLOB) AND typeof(device)='text' AND CAST(device AS BLOB)=CAST(?3 AS BLOB) AND typeof(operation_id)='text' AND CAST(operation_id AS BLOB)=CAST(?4 AS BLOB))",params![scope.account_digest,scope.library_id,scope.device_id,operation],|r|r.get(0))?;
    if count != 1
        || !exact_scope
        || entry.operation.operation_id != operation
        || graph.root.indexed_revision != entry.operation.revision_id
    {
        return Err(SyncError::Schema);
    }
    if entry.state != OutboxState::Acknowledged || entry.receipt.is_none() {
        return Err(SyncError::NotReady);
    }
    let root_receipt = graph.root_receipt.as_deref().ok_or(SyncError::NotReady)?;
    graph::validate_receipt(
        scope,
        operation,
        &graph.root_blob,
        entry.envelope_utf8.as_ref().ok_or(SyncError::Schema)?.len(),
        root_receipt.as_bytes(),
    )?;
    if graph.dependencies.iter().any(|d| d.raw_receipt.is_none()) {
        return Err(SyncError::NotReady);
    }
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
    let schema_version = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
    Ok(PublishedGraphProof {
        transaction: tx,
        scope: scope.clone(),
        graph,
        schema_version,
    })
}
/// Compatibility read. This does not retain proof through subsequent writes;
/// mutation callers use published_graph_proof_in and its final revalidation.
pub fn published_graph_in(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    operation: &str,
    change: &RemoteChange,
) -> Result<OutboxEntry> {
    Ok(published_graph_proof_in(tx, scope, operation, change)?
        .graph
        .root
        .entry)
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
pub(super) fn require_base(tx: &Transaction<'_>) -> Result<()> {
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
