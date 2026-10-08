//! Borrowed-transaction opaque deletion admission and exact publication proof.
//! No encrypted body, upload receipt, source grant or schema initialization.
use super::*;

pub fn enqueue_tombstone_in(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    limits: &SyncLimits,
    operation: &CommitOperation,
) -> Result<OutboxEntry> {
    graph_proof::require_base(tx)?;
    if !operation.deleted || operation.blob.is_some() {
        return Err(SyncError::Invalid);
    }
    admission::enqueue_on(tx, scope, limits, operation.clone(), None)
}

pub struct PublishedTombstoneProof<'tx, 'connection> {
    tx: &'tx Transaction<'connection>,
    scope: SyncScope,
    root: outbox_read::Snapshot,
    schema: u64,
}
impl PublishedTombstoneProof<'_, '_> {
    pub fn entry(&self) -> &OutboxEntry {
        &self.root.entry
    }
    pub fn revalidate(&self) -> Result<()> {
        graph_proof::require_base(self.tx)?;
        let schema: u64 = self
            .tx
            .query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        if schema != self.schema {
            return Err(SyncError::Schema);
        }
        let s = &self.scope;
        let r = &self.root;
        let operation = &r.entry.operation.operation_id;
        let count: u64 = self.tx.query_row("SELECT count(*) FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4",params![s.account_digest,s.library_id,s.device_id,operation],|r|r.get(0))?;
        let exact: bool = self.tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND typeof(account)='text' AND CAST(account AS BLOB)=CAST(?1 AS BLOB) AND typeof(library)='text' AND CAST(library AS BLOB)=CAST(?2 AS BLOB) AND typeof(device)='text' AND CAST(device AS BLOB)=CAST(?3 AS BLOB) AND typeof(operation_id)='text' AND CAST(operation_id AS BLOB)=CAST(?4 AS BLOB) AND sequence=?5 AND typeof(revision_id)='text' AND CAST(revision_id AS BLOB)=?6 AND typeof(operation)='text' AND CAST(operation AS BLOB)=?7 AND envelope IS NULL AND typeof(state)='text' AND CAST(state AS BLOB)=?8 AND typeof(receipt)='text' AND CAST(receipt AS BLOB)=?9 AND typeof(bytes)='integer' AND bytes=?10)",params![s.account_digest,s.library_id,s.device_id,operation,r.sequence,r.indexed_revision.as_bytes(),r.raw_operation.as_bytes(),r.raw_state.as_bytes(),r.raw_receipt.as_deref().ok_or(SyncError::Schema)?.as_bytes(),r.bytes],|r|r.get(0))?;
        if count != 1 || !exact {
            return Err(SyncError::Schema);
        }
        if graph_schema::present(self.tx)? {
            graph_schema::require(self.tx)?;
            let count: u64 = self.tx.query_row("SELECT (SELECT count(*) FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4)+(SELECT count(*) FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4)",params![s.account_digest,s.library_id,s.device_id,operation],|r|r.get(0))?;
            if count != 0 {
                return Err(SyncError::Schema);
            }
        }
        Ok(())
    }
}
pub fn published_tombstone_proof_in<'tx, 'connection>(
    tx: &'tx Transaction<'connection>,
    scope: &SyncScope,
    operation: &str,
    change: &RemoteChange,
) -> Result<PublishedTombstoneProof<'tx, 'connection>> {
    scope.validate()?;
    graph_proof::require_base(tx)?;
    validate_uuid(operation)?;
    let root = outbox_read::snapshot_on(tx, scope, operation)?.ok_or(SyncError::NotReady)?;
    let entry = &root.entry;
    let receipt = entry.receipt.as_ref().ok_or(SyncError::NotReady)?;
    if !entry.operation.deleted
        || entry.operation.blob.is_some()
        || entry.envelope_utf8.is_some()
        || entry.operation.operation_id != operation
        || entry.operation.revision_id != root.indexed_revision
        || entry.state != OutboxState::Acknowledged
        || receipt.conflict
        || receipt.heads != [change.revision_id.clone()]
        || receipt.sequence != change.sequence
        || !change.deleted
        || change.blob.is_some()
        || change.device != scope.device_id
        || entry.operation.item_id != change.item_id
        || entry.operation.revision_id != change.revision_id
        || entry.operation.parents != change.parents
    {
        return Err(SyncError::Reused);
    }
    let schema = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
    let proof = PublishedTombstoneProof {
        tx,
        scope: scope.clone(),
        root,
        schema,
    };
    proof.revalidate()?;
    Ok(proof)
}
