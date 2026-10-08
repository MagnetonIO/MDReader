//! Authenticated-channel receipt admission. Graph roots never accept weak receipts.
use super::*;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ScopedReceipt {
    pub v: u8,
    pub account: String,
    pub library_id: String,
    pub device: String,
    pub operation_id: String,
    pub blob: String,
    pub bytes: u64,
}
impl SyncStore<'_> {
    /// Caller supplies the authenticated account/device HTTP response. Strict v2
    /// validation does not itself authenticate that channel. Graph root receipt
    /// and uploaded state commit together, after all dependency receipts exist.
    pub fn mark_uploaded_scoped(
        &mut self,
        scope: &SyncScope,
        operation: &str,
        status: u16,
        bytes: &[u8],
    ) -> Result<()> {
        self.response_scope(scope, status, &[200, 201])?;
        validate_uuid(operation)?;
        let receipt: ScopedReceipt = parse(bytes, MAX_RECEIPT_BYTES)?;
        if receipt.v != 2
            || receipt.account != scope.account_digest
            || receipt.library_id != scope.library_id
            || receipt.device != scope.device_id
            || receipt.operation_id != operation
        {
            return Err(SyncError::Authentication);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_upload(&tx, scope, operation, &receipt.blob, receipt.bytes)?;
        graph::save_root_receipt(&tx, scope, operation, bytes)?;
        uploaded(&tx, scope, operation)?;
        tx.commit()?;
        Ok(())
    }
    /// Legacy root-only compatibility. A graph operation must use the stronger
    /// v2 channel; an unscoped receipt cannot publish it, even after all chunks.
    pub fn mark_uploaded(
        &mut self,
        scope: &SyncScope,
        operation: &str,
        status: u16,
        bytes: &[u8],
    ) -> Result<()> {
        self.response_scope(scope, status, &[200, 201])?;
        validate_uuid(operation)?;
        let receipt: UploadReceipt = parse(bytes, MAX_RECEIPT_BYTES)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if graph::has_root(&tx, scope, operation)? {
            return Err(SyncError::Scope);
        }
        validate_upload(&tx, scope, operation, &receipt.blob, receipt.bytes)?;
        uploaded(&tx, scope, operation)?;
        tx.commit()?;
        Ok(())
    }
}
fn validate_upload(
    db: &Connection,
    scope: &SyncScope,
    operation: &str,
    blob: &str,
    bytes: u64,
) -> Result<()> {
    validate_digest(blob)?;
    let entry = entry_on(db, scope, operation)?.ok_or(SyncError::NotReady)?;
    if entry.operation.deleted
        || entry.operation.blob.as_deref() != Some(blob)
        || entry.envelope_utf8.as_ref().map(|b| b.len() as u64) != Some(bytes)
    {
        return Err(SyncError::Authentication);
    }
    Ok(())
}
fn uploaded(tx: &Transaction<'_>, scope: &SyncScope, operation: &str) -> Result<()> {
    tx.execute("UPDATE mdreader_sync_outbox SET state='uploaded' WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND state='pending'",params![scope.account_digest,scope.library_id,scope.device_id,operation])?;
    Ok(())
}
