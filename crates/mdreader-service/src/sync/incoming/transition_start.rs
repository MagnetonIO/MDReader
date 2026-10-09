use super::{
    heads::{self, Head},
    identity::{self, Identity},
    ledger::{self, Entry},
    storage::{self, Materialization},
    types::*,
};
use mdreader_core::{
    device::{
        self,
        publication::{self, CurrentPlan, IncomingOrigin, VerifiedCurrent},
    },
    ReaderStore,
};
use mdreader_sync::{codec::ReadingPosition, SyncScope};

pub(super) enum Kind {
    State {
        previous: Option<Head>,
        target: Head,
        receipt: Receipt,
        pinned: bool,
        line: Option<u32>,
    },
    DocumentDelete {
        target: Head,
        receipt: Receipt,
    },
    StateDelete {
        previous: Head,
        receipt: Receipt,
    },
}
pub(super) struct Plan {
    pub kind: Kind,
    pub current: Option<CurrentPlan>,
}
pub(super) struct Verified {
    pub kind: Kind,
    pub current: Option<VerifiedCurrent>,
}
impl Plan {
    pub fn verify(self) -> Result<Verified> {
        Ok(Verified {
            kind: self.kind,
            current: self.current.map(CurrentPlan::verify).transpose()?,
        })
    }
}
pub(super) enum Start {
    Plan(Box<Plan>),
    Applied(Box<Receipt>),
    Pending(PendingReason),
}

fn select(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    identity: &Identity,
) -> Result<std::result::Result<Kind, PendingReason>> {
    let head = heads::get(tx, key, &identity.change.item_id)?;
    // Observe an authenticated staged content tombstone even when a conflicting
    // parent/baseline later prevents application. Do not infer an unknown kind.
    let head = if identity.deleted {
        if let Some(head) = head.filter(|head| head.kind == "content") {
            if head
                .tombstone
                .as_deref()
                .is_some_and(|old| old != identity.change.revision_id)
            {
                return Ok(Err(PendingReason::Tombstoned));
            }
            Some(heads::observe_delete(
                tx,
                key,
                &head,
                &identity.change.revision_id,
            )?)
        } else {
            heads::get(tx, key, &identity.change.item_id)?
        }
    } else {
        head
    };
    classify_known(tx, key, identity, head)
}
/// Read-only admission classification; unlike apply, never observes a tombstone
/// or reserves a transition. Used for truthful pending choices/status.
pub(super) fn classify(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    identity: &Identity,
) -> Result<std::result::Result<Kind, PendingReason>> {
    classify_known(
        tx,
        key,
        identity,
        heads::get(tx, key, &identity.change.item_id)?,
    )
}
fn classify_known(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    identity: &Identity,
    head: Option<Head>,
) -> Result<std::result::Result<Kind, PendingReason>> {
    if !heads::parents(identity, head.as_ref()) {
        return Ok(Err(PendingReason::HeadConflict));
    }
    if identity.deleted {
        let Some(head) = head else {
            return Ok(Err(PendingReason::TombstoneUnresolved));
        };
        let receipt = heads::receipt(tx, key, &head)?;
        return Ok(if head.kind == "content" {
            if head.generation.is_none() {
                Err(PendingReason::BaselineUnknown)
            } else {
                Ok(Kind::DocumentDelete {
                    target: head,
                    receipt,
                })
            }
        } else {
            Ok(Kind::StateDelete {
                previous: head,
                receipt,
            })
        });
    }
    let state = identity.user_state.as_ref().ok_or(Error::Conflict)?;
    if head.as_ref().is_some_and(|head| {
        head.kind != "user_state" || head.target.as_deref() != Some(&state.document_id)
    }) {
        return Ok(Err(PendingReason::HeadConflict));
    }
    if head.as_ref().is_some_and(|head| head.tombstone.is_some()) {
        return Ok(Err(PendingReason::Tombstoned));
    }
    let Some(target) = heads::get(tx, key, &state.document_id)? else {
        return Ok(Err(PendingReason::TargetUnavailable));
    };
    if target.kind != "content" {
        return Ok(Err(PendingReason::TargetUnavailable));
    }
    if target.tombstone.is_some() {
        return Ok(Err(PendingReason::Tombstoned));
    }
    if target.generation.is_none() {
        return Ok(Err(PendingReason::BaselineUnknown));
    }
    let position = match &state.position {
        Some(ReadingPosition::BlockPixels { .. }) => {
            return Ok(Err(PendingReason::PositionUnsupported))
        }
        Some(ReadingPosition::Line { revision_id, line }) => {
            if revision_id != &target.revision {
                return Ok(Err(PendingReason::PositionStale));
            }
            Some(*line)
        }
        None => None,
    };
    let receipt = heads::receipt(tx, key, &target)?;
    Ok(Ok(Kind::State {
        previous: head,
        target,
        receipt,
        pinned: state.pinned,
        line: position,
    }))
}
pub(super) fn start(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
    materialization: Materialization<'_>,
) -> Result<Start> {
    let key = identity::scope_key(scope)?;
    let result =
        owner.with_library_connection(|db| -> Result<std::result::Result<Kind, Start>> {
            let (exact, applied) = storage::staged(db, scope, &identity.change)?;
            if !exact {
                return Ok(Err(Start::Pending(PendingReason::Unstaged)));
            }
            let tx = db.transaction()?;
            super::schema::require_library(&tx, scope.library_id())?;
            let entry = ledger::inspect(&tx, &key, identity, origin)?;
            match &entry {
                Some(Entry::Applied(receipt)) if applied >= identity.change.sequence => {
                    return Ok(Err(Start::Applied(receipt.clone())))
                }
                Some(Entry::Applied(_)) => return Err(Error::Conflict),
                Some(Entry::Pending(reason)) => return Ok(Err(Start::Pending(*reason))),
                _ => {}
            }
            if applied.checked_add(1) != Some(identity.change.sequence) {
                return Ok(Err(Start::Pending(PendingReason::NotContiguous)));
            }
            let kind = select(&tx, &key, identity)?;
            if let Ok(kind) = &kind {
                if entry.is_none() {
                    ledger::reserve(&tx, &key, identity, origin)?;
                }
                let receipt = match kind {
                    Kind::State { receipt, .. }
                    | Kind::DocumentDelete { receipt, .. }
                    | Kind::StateDelete { receipt, .. } => receipt,
                };
                ledger::associate(&tx, &key, identity, origin, &receipt.document.id)?;
            }
            tx.commit()?;
            Ok(kind.map_err(Start::Pending))
        })?;
    let kind = match result {
        Ok(kind) => kind,
        Err(result) => return Ok(result),
    };
    let target = match &kind {
        Kind::State { target, .. } | Kind::DocumentDelete { target, .. } => Some(target),
        Kind::StateDelete { .. } => None,
    };
    let current = if let Some(target) = target {
        let state = match device::state::current(owner, &target.document) {
            Ok(state) => state,
            Err(device::Error::Unavailable) => {
                return Ok(Start::Pending(PendingReason::TargetUnavailable))
            }
            Err(error) => return Err(publication::PublicationError::from(error).into()),
        };
        if Some(state.generation) != target.generation {
            return Ok(Start::Pending(PendingReason::LocalStateChanged));
        }
        let assets = materialization
            .runtime
            .assets(owner, materialization.assets)
            .map_err(publication::PublicationError::from)?;
        Some(publication::prepare_current(
            owner,
            &target.document,
            &assets,
            origin,
        )?)
    } else {
        None
    };
    Ok(Start::Plan(Box::new(Plan { kind, current })))
}
