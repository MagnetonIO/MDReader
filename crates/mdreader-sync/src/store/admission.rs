//! Root outbox admission on the caller's borrowed transaction. No hidden writer.
use super::*;
pub(super) fn enqueue_on(
    transaction: &Transaction<'_>,
    scope: &SyncScope,
    limits: &SyncLimits,
    mut operation: CommitOperation,
    envelope: Option<&[u8]>,
) -> Result<OutboxEntry> {
    scope.validate()?;
    limits.validate()?;
    operation.validate()?;
    operation.parents.sort();
    validate_encrypted_body(&operation, envelope)?;
    let canonical = String::from_utf8(operation.wire_bytes()?).map_err(|_| SyncError::Invalid)?;
    let bytes = (canonical.len() + envelope.map_or(0, <[u8]>::len) + MAX_RECEIPT_BYTES) as u64;
    if let Some(old) = entry_on(transaction, scope, &operation.operation_id)? {
        if old.operation != operation || old.envelope_utf8.as_deref() != envelope {
            return Err(SyncError::Reused);
        }
        return Ok(old);
    }
    let reused:u64=transaction.query_row("SELECT count(*) FROM mdreader_sync_outbox WHERE account=?1 AND (operation_id=?2 OR revision_id=?3)",params![scope.account_digest,operation.operation_id,operation.revision_id],|r|r.get(0))?;
    if reused > 0 {
        return Err(SyncError::Reused);
    }
    let (count, total): (u64, u64) = transaction.query_row(
        "SELECT count(*),coalesce(sum(bytes),0) FROM mdreader_sync_outbox",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count >= limits.max_outbox_entries
        || total
            .saturating_add(graph::charged_bytes(transaction)?)
            .saturating_add(bytes)
            > limits.max_outbox_bytes
    {
        return Err(SyncError::Bounds);
    }
    let state = if operation.deleted {
        OutboxState::Uploaded
    } else {
        OutboxState::Pending
    };
    transaction.execute("INSERT INTO mdreader_sync_outbox(account,library,device,operation_id,revision_id,operation,envelope,state,receipt,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9)",params![scope.account_digest,scope.library_id,scope.device_id,operation.operation_id,operation.revision_id,canonical,envelope,state.sql(),bytes])?;
    Ok(OutboxEntry {
        operation,
        envelope_utf8: envelope.map(<[u8]>::to_vec),
        state,
        receipt: None,
    })
}
