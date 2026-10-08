//! Exact SQLite comparisons against a validated same-transaction graph snapshot.
//! No hashing, decrypting, JSON parsing, ciphertext extraction or SQL writes.
use super::*;
pub(super) fn revalidate(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    graph: &graph_read::Snapshot,
    schema_version: u64,
) -> Result<()> {
    graph_proof::require_base(tx)?;
    graph_schema::require(tx)?;
    let version: u64 = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
    if version != schema_version {
        return Err(SyncError::Schema);
    }
    let root = &graph.root;
    let operation = &root.entry.operation.operation_id;
    let count:u64=tx.query_row("SELECT count(*) FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4",params![scope.account_digest,scope.library_id,scope.device_id,operation],|r|r.get(0))?;
    let exact:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND typeof(account)='text' AND CAST(account AS BLOB)=CAST(?1 AS BLOB) AND typeof(library)='text' AND CAST(library AS BLOB)=CAST(?2 AS BLOB) AND typeof(device)='text' AND CAST(device AS BLOB)=CAST(?3 AS BLOB) AND typeof(operation_id)='text' AND CAST(operation_id AS BLOB)=CAST(?4 AS BLOB) AND sequence=?5 AND typeof(revision_id)='text' AND CAST(revision_id AS BLOB)=?6 AND typeof(operation)='text' AND CAST(operation AS BLOB)=?7 AND typeof(envelope)='blob' AND envelope=?8 AND typeof(state)='text' AND CAST(state AS BLOB)=?9 AND typeof(receipt)='text' AND CAST(receipt AS BLOB)=?10 AND typeof(bytes)='integer' AND bytes=?11)",params![scope.account_digest,scope.library_id,scope.device_id,operation,root.sequence,root.indexed_revision.as_bytes(),root.raw_operation.as_bytes(),root.entry.envelope_utf8.as_deref().ok_or(SyncError::Schema)?,root.raw_state.as_bytes(),root.raw_receipt.as_deref().ok_or(SyncError::Schema)?.as_bytes(),root.bytes],|r|r.get(0))?;
    if count != 1 || !exact {
        return Err(SyncError::Schema);
    }
    let (roots,dependencies):(u64,usize)=tx.query_row("SELECT (SELECT count(*) FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4),(SELECT count(*) FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4)",params![scope.account_digest,scope.library_id,scope.device_id,operation],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let exact:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND rowid=?5 AND typeof(root_blob)='text' AND CAST(root_blob AS BLOB)=?6 AND typeof(dependency_count)='integer' AND dependency_count=?7 AND typeof(root_receipt)='text' AND CAST(root_receipt AS BLOB)=?8)",params![scope.account_digest,scope.library_id,scope.device_id,operation,graph.root_row,graph.root_blob.as_bytes(),graph.dependencies.len(),graph.root_receipt.as_deref().ok_or(SyncError::Schema)?.as_bytes()],|r|r.get(0))?;
    if roots != 1 || dependencies != graph.dependencies.len() || !exact {
        return Err(SyncError::Schema);
    }
    let mut query=tx.prepare("SELECT EXISTS(SELECT 1 FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND rowid=?5 AND typeof(ordinal)='integer' AND ordinal=?6 AND typeof(blob)='text' AND CAST(blob AS BLOB)=?7 AND typeof(context)='text' AND CAST(context AS BLOB)=?8 AND typeof(envelope)='blob' AND envelope=?9 AND typeof(bytes)='integer' AND bytes=?10 AND typeof(receipt)='text' AND CAST(receipt AS BLOB)=?11)")?;
    for dependency in &graph.dependencies {
        let exact: bool = query.query_row(
            params![
                scope.account_digest,
                scope.library_id,
                scope.device_id,
                operation,
                dependency.rowid,
                dependency.ordinal,
                dependency.entry.blob.as_bytes(),
                dependency.raw_context.as_bytes(),
                dependency.entry.envelope_utf8.as_slice(),
                dependency.entry.envelope_utf8.len(),
                dependency
                    .raw_receipt
                    .as_deref()
                    .ok_or(SyncError::Schema)?
                    .as_bytes()
            ],
            |r| r.get(0),
        )?;
        if !exact {
            return Err(SyncError::Schema);
        }
    }
    Ok(())
}
