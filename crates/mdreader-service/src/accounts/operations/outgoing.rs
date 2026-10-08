//! Native explicit selection → retained blocking admission → foreground pump.
use super::*;

impl Actor {
    pub async fn selected_sync(
        &mut self,
        command: &Command,
        document: Uuid,
        operation: Uuid,
    ) -> Result<Outcome> {
        self.shared.check(command)?;
        self.shared.phase(command, Phase::Syncing)?;
        // This real lease is held by the issued blocking task, never an await.
        // Freeze denies admission and cannot claim completion until it drops.
        let (active, lease) = self.shared.projection_lease()?;
        self.blocking(move || {
            let _lease = lease;
            active.incoming.queue_selected_document(
                &active.selected,
                &active.authority,
                document,
                operation,
            )?;
            Ok(())
        })
        .await
        .map_err(|_| AccountError::Interrupted)??;
        // Late cancellation after durable graph admission preserves its immutable
        // bytes/intents. It never discards or silently reauthorizes them.
        self.shared.check(command)?;
        self.sync(command).await
    }
}
