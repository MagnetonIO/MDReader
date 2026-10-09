//! Authenticated bounded hydration followed by retained owner work. No raw keys.
use super::*;
use crate::sync::incoming::ResolutionRequest;
use mdreader_sync::{
    incoming::{EncryptedResource, PreparedIncoming},
    transport::Owner,
};

impl Actor {
    async fn prepare_pending(
        &mut self,
        command: &Command,
        scope: &SelectedScope,
    ) -> Result<Option<PreparedIncoming>> {
        self.shared.check(command)?;
        self.shared.phase(command, Phase::Syncing)?;
        let active = self.active()?;
        if SelectedScope::current(&active.authority.scope()?)? != *scope {
            return Err(AccountError::IdentityOrScope);
        }
        let barrier = self
            .setup
            .factory
            .session_controller()
            .active_scope_barrier();
        let reader = self.owner.clone();
        let owner = crate::sync::TransportOwner {
            owner: &reader,
            active_scope: &barrier,
        };
        let staged = owner.with_store(&active.authority.scope()?, |s| {
            let cursor = s.cursors()?;
            Ok(s.staged(cursor.applied, 1)?.into_iter().next())
        })?;
        let Some(staged) = staged else {
            return Ok(None);
        };
        let blobs = active.authority.incoming_dependencies(&staged)?;
        let resources = self
            .setup
            .transport
            .download_resources(&owner, &active.authority, &blobs, 32 * 1024 * 1024)
            .await?;
        self.shared.check(command)?;
        self.blocking(move || {
            let refs = resources
                .iter()
                .map(|r| EncryptedResource {
                    blob_id: &r.blob,
                    envelope_utf8: &r.envelope_utf8,
                })
                .collect::<Vec<_>>();
            Ok(active.authority.prepare_incoming(&staged, &refs)?)
        })
        .await
        .map_err(|_| AccountError::Interrupted)?
        .map(Some)
    }
    pub async fn list_pending(
        &mut self,
        command: &Command,
        scope: SelectedScope,
    ) -> Result<Outcome> {
        let Some(input) = self.prepare_pending(command, &scope).await? else {
            return Ok(Outcome::PendingList {
                scope,
                entries: vec![],
            });
        };
        self.shared.check(command)?;
        let (active, lease) = self.shared.projection_lease()?;
        if SelectedScope::current(&active.authority.scope()?)? != scope {
            return Err(AccountError::IdentityOrScope);
        }
        let entry = self
            .blocking(move || {
                let _lease = lease;
                Ok(active.incoming.inspect_pending(&active.selected, &input)?)
            })
            .await
            .map_err(|_| AccountError::Interrupted)??;
        self.shared.check(command)?;
        Ok(Outcome::PendingList {
            scope,
            entries: vec![entry],
        })
    }
    pub async fn resolve_pending(
        &mut self,
        command: &Command,
        scope: SelectedScope,
        request: ResolutionRequest,
    ) -> Result<Outcome> {
        let input = self
            .prepare_pending(command, &scope)
            .await?
            .ok_or(AccountError::Conflict)?;
        self.shared.check(command)?;
        let (active, lease) = self.shared.projection_lease()?;
        if SelectedScope::current(&active.authority.scope()?)? != scope {
            return Err(AccountError::IdentityOrScope);
        }
        let receipt = self
            .blocking(move || {
                let _lease = lease;
                Ok(active
                    .incoming
                    .resolve_pending(&active.selected, &input, &request)?)
            })
            .await
            .map_err(|_| AccountError::Interrupted)??;
        self.shared.check(command)?;
        Ok(Outcome::PendingResolved {
            scope,
            sequence: receipt.sequence,
            revision_id: receipt.wire_revision,
            choice: crate::sync::incoming::ResolutionChoice::KeepBoth,
            document: mdreader_core::device::catalog::Receipt {
                document_id: receipt.document.id,
                source_id: receipt.document.source_id,
                revision: receipt.document.revision,
            },
        })
    }
}
