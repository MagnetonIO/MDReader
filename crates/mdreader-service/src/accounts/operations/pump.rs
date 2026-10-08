//! Bounded foreground hydrate → selected owner projection → applied-only ack.
use super::*;
use mdreader_sync::{incoming::EncryptedResource, transport::Owner};

impl Actor {
    pub async fn sync(&mut self, command: &Command) -> Result<Outcome> {
        self.shared.phase(command, Phase::Syncing)?;
        let active = self.active()?;
        let barrier = self
            .setup
            .factory
            .session_controller()
            .active_scope_barrier();
        // This adapter owns only Arcs; it holds no guard during a network wait.
        let reader = self.owner.clone();
        let owner = crate::sync::TransportOwner {
            owner: &reader,
            active_scope: &barrier,
        };
        let report = active
            .authority
            .run(
                &self.setup.transport,
                &owner,
                Budget {
                    outgoing: 5,
                    pages: 1,
                    page_size: 5,
                    transfer_bytes: 8 * 1024 * 1024,
                },
            )
            .await?;
        self.shared.check(command)?;
        let scope = active.authority.scope()?;
        let mut wire_remaining = 32 * 1024 * 1024;
        let mut pending = None;
        for _ in 0..5 {
            self.shared.check(command)?;
            let staged = owner.with_store(&scope, |s| {
                let cursor = s.cursors()?;
                Ok(s.staged(cursor.applied, 1)?.into_iter().next())
            })?;
            let Some(revision) = staged else {
                break;
            };
            let sequence = revision.change.sequence;
            let blobs = active.authority.incoming_dependencies(&revision)?;
            let resources = self
                .setup
                .transport
                .download_resources(&owner, &active.authority, &blobs, wire_remaining)
                .await?;
            wire_remaining = resources.iter().try_fold(wire_remaining, |left, b| {
                left.checked_sub(b.envelope_utf8.len())
                    .ok_or(AccountError::Limit)
            })?;
            self.shared.check(command)?;
            // Expensive decoding/materialization happens on a retained blocking
            // handle. Cancellation cannot orphan this work or claim it settled.
            let selected = active.clone();
            let result = self
                .blocking(move || {
                    let references = resources
                        .iter()
                        .map(|r| EncryptedResource {
                            blob_id: &r.blob,
                            envelope_utf8: &r.envelope_utf8,
                        })
                        .collect::<Vec<_>>();
                    let prepared = selected
                        .authority
                        .prepare_incoming(&revision, &references)?;
                    Ok(selected.incoming.apply(&selected.selected, &prepared)?)
                })
                .await
                .map_err(|_| AccountError::Interrupted)??;
            self.shared.check(command)?;
            match result {
                crate::sync::incoming::Outcome::Applied { .. } => {}
                crate::sync::incoming::Outcome::Pending(reason) => {
                    pending = Some(SyncPending { sequence, reason });
                    break;
                }
            }
            if wire_remaining == 0 {
                break;
            }
        }
        // Zero-page/zero-outgoing run only sends the durable applied cursor. A
        // staged/failed/pending body can never be acknowledged by this channel.
        let ack = active
            .authority
            .run(
                &self.setup.transport,
                &owner,
                Budget {
                    outgoing: 0,
                    pages: 0,
                    page_size: 1,
                    transfer_bytes: 4096,
                },
            )
            .await?;
        self.shared.check(command)?;
        Ok(Outcome::Sync {
            committed: report.committed,
            conflicts: report.conflicts,
            received: ack.received,
            applied: ack.applied,
            acknowledged: ack.acknowledged,
            remote_pending: report.remote_pending || ack.received > ack.applied,
            local_pending: ack.local_pending,
            pending,
        })
    }
}
