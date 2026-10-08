//! One callback owns publication, cursor CAS and final transaction-bound proofs.
use super::super::*;
use std::cell::Cell;

/// Consuming cursor capability for this exact transaction/scope/staged change.
/// No Clone/Debug/serialization, operation argument or escaping writer handle.
pub struct AppliedAdvance<'tx, 'connection, 'scope> {
    transaction: &'tx Transaction<'connection>,
    scope: &'scope SyncScope,
    change: &'scope RemoteChange,
    previous: CursorState,
    advanced: &'tx Cell<bool>,
}
impl AppliedAdvance<'_, '_, '_> {
    /// Invoke only after actual typed owner publication and its durable receipt.
    /// Failure requires whole-callback rollback; the outer method owns COMMIT.
    pub fn advance(self) -> Result<()> {
        if self.advanced.get() || cursors_on(self.transaction, self.scope)? != self.previous {
            return Err(SyncError::Reused);
        }
        require_staged(self.transaction, self.scope, self.change)?;
        let changed=self.transaction.execute("UPDATE mdreader_sync_cursors SET applied=?1 WHERE account=?2 AND library=?3 AND device=?4 AND received=?5 AND applied=?6 AND acked=?7",params![self.change.sequence,self.scope.account_digest,self.scope.library_id,self.scope.device_id,self.previous.received,self.previous.applied,self.previous.server_acknowledged])?;
        if changed != 1
            || cursors_on(self.transaction, self.scope)?
                != expected(self.previous, self.change.sequence)
        {
            return Err(SyncError::Reused);
        }
        self.advanced.set(true);
        Ok(())
    }
}
impl SyncStore<'_> {
    /// Native publication needs one callback so a real borrowing graph proof can
    /// remain on its stack through owner/receipt/cursor writes and final fences.
    /// Existing mark_applied_guarded compatibility behavior is unchanged.
    ///
    /// Exactly the next staged change is admitted. Callback failure, missing
    /// advancement or changed cursor rolls back everything. No SQL writes occur
    /// after callback return; caller must run final authority immediately last.
    pub fn with_applied_transaction<T, E>(
        &mut self,
        change: &RemoteChange,
        action: impl FnOnce(&Transaction<'_>, AppliedAdvance<'_, '_, '_>) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E>
    where
        E: From<SyncError> + From<rusqlite::Error>,
    {
        let mut checked = change.clone();
        checked.validate()?;
        if checked != *change {
            return Err(SyncError::Invalid.into());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        graph_proof::require_base(&transaction)?;
        let previous = cursors_on(&transaction, &self.scope)?;
        if previous.applied.checked_add(1) != Some(change.sequence)
            || change.sequence > previous.received
        {
            return Err(SyncError::NotReady.into());
        }
        require_staged(&transaction, &self.scope, change)?;
        let advanced = Cell::new(false);
        let result = action(
            &transaction,
            AppliedAdvance {
                transaction: &transaction,
                scope: &self.scope,
                change,
                previous,
                advanced: &advanced,
            },
        )?;
        graph_proof::require_base(&transaction)?;
        if !advanced.get()
            || cursors_on(&transaction, &self.scope)? != expected(previous, change.sequence)
        {
            return Err(SyncError::Reused.into());
        }
        require_staged(&transaction, &self.scope, change)?;
        transaction.commit()?;
        Ok(result)
    }
}
fn expected(mut previous: CursorState, through: u64) -> CursorState {
    previous.applied = through;
    previous
}
fn require_staged(tx: &Transaction<'_>, scope: &SyncScope, change: &RemoteChange) -> Result<()> {
    let parents = String::from_utf8(encode(&change.parents)?).map_err(|_| SyncError::Invalid)?;
    let exact:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_remote WHERE account=?1 AND library=?2 AND device=?3 AND sequence=?4 AND revision=?5 AND item=?6 AND blob IS ?7 AND deleted=?8 AND parents=?9 AND source_device=?10)",params![scope.account_digest,scope.library_id,scope.device_id,change.sequence,change.revision_id,change.item_id,change.blob,change.deleted,parents,change.device],|r|r.get(0))?;
    if !exact {
        return Err(SyncError::NotReady);
    }
    Ok(())
}
