//! Receipt-proven local publications reuse exact native identities, never import.
use super::*;
use crate::sync::outgoing::{association, snapshot::Snapshot, Intent};
use mdreader_core::device::publication::{ArchivePlan, CurrentPlan};
use mdreader_sync::SyncScope;
use rusqlite::Transaction;

enum Start {
    Ignore,
    Pending(PendingReason),
    Replayed(Box<Receipt>),
    Plan {
        intent: Box<Intent>,
        current: Box<CurrentPlan>,
        archive: Box<ArchivePlan>,
    },
}
impl IncomingCoordinator {
    pub(super) fn apply_self_echo(
        &self,
        selected: &SelectedIncoming,
        input: &PreparedIncoming,
        identity: &identity::Identity,
        origin: &publication::IncomingOrigin,
    ) -> Result<Option<Outcome>> {
        let start = self.short(selected, |owner, _| {
            start(owner, input.scope(), identity, origin, self)
        })?;
        let outcome = match start {
            Start::Ignore => return Ok(None),
            Start::Pending(reason) => Outcome::Pending(reason),
            Start::Replayed(receipt) => self.short(selected, |owner, _| {
                Ok(Outcome::Applied {
                    index_queued: storage::queue_index(owner, &receipt),
                    receipt,
                    replayed: true,
                })
            })?,
            Start::Plan {
                intent,
                current,
                archive,
            } => {
                let current = current.verify()?;
                let snapshot = Snapshot::new(archive.read()?, &current)?;
                if !snapshot.matches(&intent)
                    || !matches_prepared(&snapshot, &intent, input)
                    || !matches_identity(&snapshot, &intent, identity)
                {
                    return Ok(Some(Outcome::Pending(PendingReason::LocalStateChanged)));
                }
                #[cfg(test)]
                self.pause_after_materialization()?;
                self.short(selected, |owner, session| {
                    let receipt = commit(
                        owner,
                        input.scope(),
                        identity,
                        origin,
                        Echo {
                            intent: &intent,
                            current: &current,
                            snapshot: &snapshot,
                        },
                        |_| {
                            session.check()?;
                            Ok(())
                        },
                    )?;
                    let index_queued = storage::queue_index(owner, &receipt);
                    Ok(Outcome::Applied {
                        receipt: Box::new(receipt),
                        replayed: false,
                        index_queued,
                    })
                })?
            }
        };
        Ok(Some(outcome))
    }
}
fn start(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &identity::Identity,
    origin: &publication::IncomingOrigin,
    coordinator: &IncomingCoordinator,
) -> Result<Start> {
    let key = identity::scope_key(scope)?;
    let result =
        owner.with_library_connection(|db| -> Result<std::result::Result<Intent, Start>> {
            let Some(stored) = association::by_revision(db, scope, &identity.change.revision_id)?
            else {
                if identity.change.device == scope.device_id() {
                    let tx = db.transaction()?;
                    let local = mdreader_sync::outbox_revision_in(
                        &tx,
                        scope,
                        &identity.change.revision_id,
                    )?
                    .is_some();
                    tx.commit()?;
                    if local {
                        return Ok(Err(Start::Pending(PendingReason::BaselineUnknown)));
                    }
                }
                return Ok(Err(Start::Ignore));
            };
            let (exact, applied) = storage::staged(db, scope, &identity.change)?;
            if !exact {
                return Ok(Err(Start::Pending(PendingReason::Unstaged)));
            }
            let tx = db.transaction()?;
            schema::require_library(&tx, scope.library_id())?;
            if let Some(sequence) = stored.applied {
                let receipt = match ledger::inspect(&tx, &key, identity, origin)? {
                    Some(ledger::Entry::Applied(receipt))
                        if applied >= sequence && sequence == identity.change.sequence =>
                    {
                        receipt
                    }
                    _ => return Err(Error::Conflict),
                };
                tx.commit()?;
                return Ok(Err(Start::Replayed(receipt)));
            }
            if applied.checked_add(1) != Some(identity.change.sequence) {
                return Ok(Err(Start::Pending(PendingReason::NotContiguous)));
            }
            let intent = stored.intent;
            if intent.origin != *origin {
                return Ok(Err(Start::Pending(PendingReason::ReauthorizationRequired)));
            }
            match outgoing_selection::verify_head(&tx, &key, &intent) {
                Ok(()) => {}
                Err(Error::Conflict) => {
                    return Ok(Err(Start::Pending(PendingReason::HeadConflict)))
                }
                Err(error) => return Err(error),
            }
            match publication_proof(&tx, scope, &intent, &identity.change) {
                Ok(()) => {}
                Err(Error::Conflict) => {
                    return Ok(Err(Start::Pending(PendingReason::HeadConflict)))
                }
                Err(error) => return Err(error),
            }
            match ledger::inspect(&tx, &key, identity, origin)? {
                None => ledger::reserve(&tx, &key, identity, origin)?,
                Some(ledger::Entry::Reserved) => {}
                Some(ledger::Entry::Pending(reason)) => return Ok(Err(Start::Pending(reason))),
                Some(ledger::Entry::Applied(_)) => return Err(Error::Conflict),
            }
            ledger::associate(&tx, &key, identity, origin, &intent.native_document)?;
            tx.commit()?;
            Ok(Ok(intent))
        })?;
    let intent = match result {
        Ok(intent) => intent,
        Err(start) => return Ok(start),
    };
    let state = match mdreader_core::device::state::current(owner, &intent.native_document) {
        Ok(state) => state,
        Err(mdreader_core::device::Error::Unavailable) => {
            return Ok(Start::Pending(PendingReason::TargetUnavailable))
        }
        Err(error) => return Err(publication::PublicationError::from(error).into()),
    };
    if state.generation != intent.generation || state.document.revision != intent.native_revision {
        return Ok(Start::Pending(PendingReason::LocalStateChanged));
    }
    let assets = coordinator
        .runtime
        .existing_assets(owner, &coordinator.assets)
        .map_err(publication::PublicationError::from)?
        .ok_or(Error::Conflict)?;
    let current = publication::prepare_current(owner, &intent.native_document, &assets, origin)?;
    let archive = publication::prepare_archive(
        owner,
        &intent.native_document,
        &assets,
        &coordinator.runtime,
    )?;
    Ok(Start::Plan {
        intent: Box::new(intent),
        current: Box::new(current),
        archive: Box::new(archive),
    })
}
fn matches_identity(snapshot: &Snapshot, intent: &Intent, identity: &identity::Identity) -> bool {
    if let Some(state) = &intent.state {
        return identity.user_state.as_ref() == Some(&state.payload())
            && !identity.deleted
            && snapshot.archive.state().document.pinned == state.pinned
            && snapshot.archive.state().document.line == state.line;
    }
    snapshot.archive.state().document.name == identity.title
        && snapshot.markdown_sha256 == identity.markdown_sha256
        && snapshot
            .materialized
            .iter()
            .map(|a| &a.identity)
            .eq(identity.assets.iter())
        && identity.user_state.is_none()
        && !identity.deleted
}
pub(super) fn publication_proof(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    intent: &Intent,
    change: &mdreader_sync::RemoteChange,
) -> Result<()> {
    let graph =
        mdreader_sync::published_graph_in(tx, scope, &intent.operation.operation_id, change)?;
    if graph.operation != intent.operation {
        return Err(Error::Conflict);
    }
    Ok(())
}
#[path = "self_echo_commit.rs"]
mod publication_commit;
pub(super) use publication_commit::{commit, Echo};

fn matches_prepared(snapshot: &Snapshot, intent: &Intent, input: &PreparedIncoming) -> bool {
    if let Some(state) = &intent.state {
        matches!(input.payload(), mdreader_sync::incoming::IncomingPayload::UserState(actual) if actual == &state.payload())
    } else {
        snapshot.matches_prepared(input)
    }
}
