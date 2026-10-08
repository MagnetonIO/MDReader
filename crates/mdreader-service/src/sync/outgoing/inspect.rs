//! Same actor/owner readonly reconciliation; never queues, uploads or reauthorizes.
use super::*;
use crate::sync::outgoing::{self, association, Intent, SelectedDisposition};
use mdreader_core::device::catalog::Receipt as ReadingReceipt;
use rusqlite::OptionalExtension;

impl IncomingCoordinator {
    pub fn inspect_selected_document(
        &self,
        selected: &SelectedIncoming,
        expected: &ReadingReceipt,
        operation: Uuid,
        user_state: bool,
    ) -> Result<SelectedDisposition> {
        let _operation = self.operation.try_lock().map_err(|_| Error::Busy)?;
        outgoing::validate_selected(expected)?;
        if operation.get_version_num() != 4 {
            return Err(Error::Scope);
        }
        let scope = selected.permit.scope();
        self.short_readonly(selected, |owner, session| {
            let stored = owner.with_library_connection(|db| -> Result<_> {
                let tx = db.transaction()?;
                let stored = association::get(&tx, scope, &operation.to_string())?;
                if stored.is_none() {
                    let orphan: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4)", rusqlite::params![scope.account_digest(),scope.library_id(),scope.device_id(),operation.to_string()], |r|r.get(0))?;
                    if orphan { return Err(Error::Conflict); }
                }
                tx.commit()?;
                Ok(stored)
            })?;
            let Some(stored) = stored else { session.check()?; return Ok(SelectedDisposition::Absent); };
            let intent = &stored.intent;
            if intent.state.is_some() != user_state || intent.native_document != expected.document_id || intent.native_revision != expected.revision {
                return Err(Error::Conflict);
            }
            if let Some(sequence) = stored.applied {
                owner.with_library_connection(|db| -> Result<_> {
                    let tx = db.transaction()?;
                    schema::require_current_library(&tx, scope.library_id())?;
                    applied(&tx, scope, intent, expected, sequence)?;
                    session.check()?;
                    tx.commit()?;
                    Ok(SelectedDisposition::Applied { sequence })
                })
            } else {
                let document = mdreader_core::device::catalog::document(owner, &expected.document_id)
                    .map_err(publication::PublicationError::from)?;
                if !outgoing::matches_selected(expected, &document) { return Err(Error::Conflict); }
                session.check()?;
                Ok(if intent.origin.session_epoch != selected.permit.session_token()
                    || intent.origin.selection_epoch != selected.token {
                    SelectedDisposition::ReauthorizationRequired
                } else { SelectedDisposition::Queued })
            }
        })
    }
}
fn applied(
    tx: &rusqlite::Transaction<'_>,
    scope: &mdreader_sync::SyncScope,
    intent: &Intent,
    expected: &ReadingReceipt,
    sequence: u64,
) -> Result<()> {
    if sequence == 0 || sequence > 9_007_199_254_740_991 {
        return Err(Error::Schema);
    }
    let key = identity::scope_key(scope)?;
    // A status string alone cannot clear a dispatched identity. Bind the durable
    // typed receipt, original selected source/revision, identity, origin, staged
    // metadata, applied cursor and exact immutable commit receipt. No body read.
    let (raw_identity,raw_origin,raw_receipt):(String,String,String)=tx.query_row(
        "SELECT identity,origin,receipt FROM native_incoming_items_v1 WHERE scope=?1 AND wire_revision=?2 AND wire_item=?3 AND sequence=?4 AND document_id=?5 AND state='applied' AND typeof(identity)='text' AND length(CAST(identity AS BLOB))<=16384 AND typeof(origin)='text' AND length(CAST(origin AS BLOB))<=1024 AND typeof(receipt)='text' AND length(CAST(receipt AS BLOB))<=32768",
        rusqlite::params![key,intent.operation.revision_id,intent.operation.item_id,sequence,expected.document_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?.ok_or(Error::Conflict)?;
    let identity: identity::Identity = serde_json::from_str(&raw_identity)?;
    let receipt: Receipt = serde_json::from_str(&raw_receipt)?;
    let change = mdreader_sync::RemoteChange {
        sequence,
        revision_id: intent.operation.revision_id.clone(),
        item_id: intent.operation.item_id.clone(),
        blob: intent.operation.blob.clone(),
        deleted: false,
        parents: intent.operation.parents.clone(),
        device: scope.device_id().into(),
    };
    ledger::validate_receipt(&identity, &receipt)?;
    if identity.user_state != intent.state.as_ref().map(|state| state.payload())
        || identity.change != change
        || raw_origin != serde_json::to_string(&intent.origin)?
        || !outgoing::matches_selected(expected, &receipt.document)
        || receipt.operation
            != if intent.state.is_some() {
                Operation::UserState
            } else {
                Operation::Content
            }
        || receipt.generation != Some(intent.generation)
        || receipt.markdown_sha256 != intent.markdown_sha256
        || mdreader_sync::blob_digest(&serde_json::to_vec(&receipt.materialized_assets)?)
            != intent.assets_sha256
    {
        return Err(Error::Conflict);
    }
    let staged:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_remote r JOIN mdreader_sync_cursors c USING(account,library,device) WHERE r.account=?1 AND r.library=?2 AND r.device=?3 AND r.sequence=?4 AND r.revision=?5 AND r.item=?6 AND r.blob=?7 AND r.deleted=0 AND r.parents=?8 AND r.source_device=?3 AND c.applied>=?4)", rusqlite::params![scope.account_digest(),scope.library_id(),scope.device_id(),sequence,change.revision_id,change.item_id,change.blob,serde_json::to_string(&change.parents)?], |r|r.get(0))?;
    let (raw_operation,raw_commit):(String,String)=tx.query_row("SELECT operation,receipt FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND revision_id=?5 AND state='acknowledged' AND typeof(operation)='text' AND length(CAST(operation AS BLOB))<=4096 AND typeof(receipt)='text' AND length(CAST(receipt AS BLOB))<=4096",rusqlite::params![scope.account_digest(),scope.library_id(),scope.device_id(),intent.operation.operation_id,change.revision_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(Error::Conflict)?;
    let operation: mdreader_sync::CommitOperation = serde_json::from_str(&raw_operation)?;
    let commit: mdreader_sync::CommitReceipt = serde_json::from_str(&raw_commit)?;
    if !staged
        || operation != intent.operation
        || commit.operation_id != operation.operation_id
        || commit.revision_id != operation.revision_id
        || commit.sequence != sequence
        || commit.conflict
        || commit.heads != [operation.revision_id.clone()]
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
