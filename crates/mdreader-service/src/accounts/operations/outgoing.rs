//! Native explicit selection → retained blocking admission → foreground pump.
use super::*;

impl Actor {
    pub async fn selected_sync(
        &mut self,
        command: &Command,
        selection: SelectedOperation,
        purpose: SelectedPurpose,
    ) -> Result<Outcome> {
        self.shared.check(command)?;
        self.shared.phase(command, Phase::Syncing)?;
        // This real lease is held by the issued blocking task, never an await.
        // Freeze denies admission and cannot claim completion until it drops.
        let (active, lease) = self.shared.projection_lease()?;
        selection.require_scope(&active.authority.scope()?)?;
        self.blocking(move || {
            let _lease = lease;
            match purpose {
                SelectedPurpose::Delete => active.incoming.queue_selected_delete(
                    &active.selected,
                    &active.authority,
                    &selection.receipt,
                    selection.operation_id,
                )?,
                SelectedPurpose::Content => active.incoming.queue_selected_document(
                    &active.selected,
                    &active.authority,
                    &selection.receipt,
                    selection.operation_id,
                )?,
                SelectedPurpose::UserState => active.incoming.queue_selected_state(
                    &active.selected,
                    &active.authority,
                    &selection.receipt,
                    selection.operation_id,
                )?,
            };
            Ok(())
        })
        .await
        .map_err(|_| AccountError::Interrupted)??;
        // Late cancellation after durable graph admission preserves its immutable
        // bytes/intents. It never discards or silently reauthorizes them.
        self.shared.check(command)?;
        self.sync(command).await
    }
    pub async fn inspect_selected(
        &mut self,
        command: &Command,
        selection: SelectedOperation,
        purpose: SelectedPurpose,
    ) -> Result<Outcome> {
        self.shared.check(command)?;
        let (active, lease) = self.shared.projection_lease()?;
        selection.require_scope(&active.authority.scope()?)?;
        let inspect = selection.clone();
        let disposition = self
            .blocking(move || {
                let _lease = lease;
                if purpose == SelectedPurpose::Delete {
                    return Ok(active.incoming.inspect_selected_delete(
                        &active.selected,
                        &inspect.receipt,
                        inspect.operation_id,
                    )?);
                }
                Ok(active.incoming.inspect_selected_document(
                    &active.selected,
                    &inspect.receipt,
                    inspect.operation_id,
                    purpose == SelectedPurpose::UserState,
                )?)
            })
            .await
            .map_err(|_| AccountError::Interrupted)??;
        self.shared.check(command)?;
        Ok(Outcome::SelectedStatus {
            selection,
            purpose,
            disposition,
        })
    }
}
