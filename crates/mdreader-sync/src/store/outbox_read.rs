//! Bounded outbox decoding, retaining raw bytes for exact transaction proofs.
use super::*;

pub(super) struct Snapshot {
    pub entry: OutboxEntry,
    pub raw_operation: String,
    pub raw_receipt: Option<String>,
    pub indexed_revision: String,
    pub sequence: u64,
    pub bytes: u64,
    pub raw_state: String,
}

pub(super) fn entry_on(
    db: &Connection,
    scope: &SyncScope,
    operation: &str,
) -> Result<Option<OutboxEntry>> {
    Ok(snapshot_on(db, scope, operation)?.map(|snapshot| snapshot.entry))
}

pub(super) fn snapshot_on(
    db: &Connection,
    scope: &SyncScope,
    operation_id: &str,
) -> Result<Option<Snapshot>> {
    // Read byte lengths while SQLite owns values. A corrupt oversized row is
    // distinct from a missing operation. No second ciphertext/hash capture pass.
    let row: Option<Result<Snapshot>> = db.query_row(
        "SELECT operation,envelope,state,receipt,length(CAST(operation AS BLOB)),length(CAST(envelope AS BLOB)),length(CAST(state AS BLOB)),length(CAST(receipt AS BLOB)),revision_id,sequence,bytes,length(CAST(revision_id AS BLOB)) FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4",
        params![scope.account_digest,scope.library_id,scope.device_id,operation_id], |r| {
            if r.get::<_,usize>(4)? > 4096
                || r.get::<_,Option<usize>>(5)?.is_some_and(|n| n > crate::crypto::MAX_ENVELOPE_BYTES)
                || r.get::<_,usize>(6)? > 32
                || r.get::<_,Option<usize>>(7)?.is_some_and(|n| n > MAX_RECEIPT_BYTES)
                || r.get::<_,usize>(11)? != 36 {
                return Ok(Err(SyncError::Bounds));
            }
            let raw_operation:String=r.get(0)?;
            let envelope:Option<Vec<u8>>=r.get(1)?;
            let raw_state:String=r.get(2)?;
            let raw_receipt:Option<String>=r.get(3)?;
            let indexed_revision:String=r.get(8)?;
            let sequence:u64=r.get(9)?;
            let bytes:u64=r.get(10)?;
            Ok(decode(raw_operation,envelope,raw_state,raw_receipt,indexed_revision,sequence,bytes))
        }).optional()?;
    row.transpose()
}
fn decode(
    raw_operation: String,
    envelope: Option<Vec<u8>>,
    raw_state: String,
    raw_receipt: Option<String>,
    indexed_revision: String,
    sequence: u64,
    bytes: u64,
) -> Result<Snapshot> {
    let operation: CommitOperation = parse(raw_operation.as_bytes(), 4096)?;
    operation.validate()?;
    validate_encrypted_body(&operation, envelope.as_deref())?;
    let state = OutboxState::parse(&raw_state)?;
    let receipt = raw_receipt
        .as_ref()
        .map(|raw| {
            let mut receipt: CommitReceipt = parse(raw.as_bytes(), MAX_RECEIPT_BYTES)?;
            receipt.validate()?;
            if receipt.operation_id != operation.operation_id
                || receipt.revision_id != operation.revision_id
            {
                return Err(SyncError::Schema);
            }
            Ok(receipt)
        })
        .transpose()?;
    if (state == OutboxState::Acknowledged) != receipt.is_some() {
        return Err(SyncError::Schema);
    }
    Ok(Snapshot {
        entry: OutboxEntry {
            operation,
            envelope_utf8: envelope,
            state,
            receipt,
        },
        raw_operation,
        raw_receipt,
        indexed_revision,
        sequence,
        bytes,
        raw_state,
    })
}
