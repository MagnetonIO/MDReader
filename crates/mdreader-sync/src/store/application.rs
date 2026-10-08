//! Applied publication and its final authority fence share one transaction.
use super::*;
mod transaction;
pub use transaction::AppliedAdvance;

impl SyncStore<'_> {
    /// Callback failure rolls back owner writes and the applied cursor together.
    pub fn mark_applied(
        &mut self,
        through: u64,
        apply: impl FnOnce(&Transaction<'_>) -> Result<()>,
    ) -> Result<CursorState> {
        self.mark_applied_guarded(through, apply, |_| Ok(()))
    }

    /// Native projection supplies typed owner errors and a final session/selection
    /// check after all application/cursor SQL and immediately before COMMIT.
    /// Expensive I/O/decoding must have finished before entering this method.
    pub fn mark_applied_guarded<E>(
        &mut self,
        through: u64,
        apply: impl FnOnce(&Transaction<'_>) -> std::result::Result<(), E>,
        final_check: impl FnOnce(&Transaction<'_>) -> std::result::Result<(), E>,
    ) -> std::result::Result<CursorState, E>
    where
        E: From<SyncError> + From<rusqlite::Error>,
    {
        validate_sequence(through, true)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = cursors_on(&transaction, &self.scope)?;
        if through > current.received {
            return Err(SyncError::NotReady.into());
        }
        if through > current.applied {
            apply(&transaction)?;
            transaction.execute("UPDATE mdreader_sync_cursors SET applied=?1 WHERE account=?2 AND library=?3 AND device=?4",params![through,self.scope.account_digest,self.scope.library_id,self.scope.device_id])?;
        }
        final_check(&transaction)?;
        transaction.commit()?;
        Ok(self.cursors()?)
    }
}
