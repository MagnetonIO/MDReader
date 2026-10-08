use super::{
    heads::{self, Head},
    identity::{self, Identity},
    ledger::{self, Entry},
    types::*,
};
use mdreader_core::{
    device::{
        assets::AssetRuntime,
        publication::{self, CurrentPlan, IncomingDocument, IncomingOrigin, WritePlan},
    },
    ReaderStore,
};
use mdreader_sync::{RemoteChange, SyncScope, SyncStore};
use rusqlite::Connection;
use std::path::Path;

pub(super) use super::commit::commit_on;
pub(super) use super::content_commit::{commit, commit_replacement, Replacement};

pub(super) enum Start {
    Plan(Box<WritePlan>),
    Replacement {
        plan: Box<WritePlan>,
        current: Box<CurrentPlan>,
        head: Head,
    },
    Applied(Box<Receipt>),
    Pending(PendingReason),
}

pub(super) struct Materialization<'a> {
    pub documents: &'a Path,
    pub assets: &'a Path,
    pub runtime: &'a AssetRuntime,
}

pub(super) fn staged(
    db: &mut Connection,
    scope: &SyncScope,
    change: &RemoteChange,
) -> Result<(bool, u64)> {
    let sync = SyncStore::attach(db, scope.clone())?;
    let cursors = sync.cursors()?;
    if change.sequence == 0 || change.sequence > cursors.received {
        return Ok((false, cursors.applied));
    }
    let exact = sync
        .staged(change.sequence.saturating_sub(1), 1)?
        .first()
        .is_some_and(|r| r.change == *change);
    Ok((exact, cursors.applied))
}
pub(super) fn start(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
    document: &IncomingDocument,
    materialization: Materialization<'_>,
) -> Result<Start> {
    let key = identity::scope_key(scope)?;
    let (result, head) =
        owner.with_library_connection(|db| -> Result<(Option<Start>, Option<Head>)> {
            let (exact, applied) = staged(db, scope, &identity.change)?;
            if !exact {
                return Ok((Some(Start::Pending(PendingReason::Unstaged)), None));
            }
            let tx = db.transaction()?;
            super::schema::require_library(&tx, scope.library_id())?;
            let entry = ledger::inspect(&tx, &key, identity, origin)?;
            let fresh = entry.is_none();
            let mut checked_head = None;
            let result = match entry {
                Some(Entry::Applied(receipt)) if applied >= identity.change.sequence => {
                    Some(Start::Applied(receipt))
                }
                Some(Entry::Applied(_)) => return Err(Error::Conflict),
                Some(Entry::Pending(reason)) => Some(Start::Pending(reason)),
                _ if applied.checked_add(1) != Some(identity.change.sequence) => {
                    Some(Start::Pending(PendingReason::NotContiguous))
                }
                _ => {
                    let head = heads::get(&tx, &key, &identity.change.item_id)?;
                    if let Some(reason) = heads::content_admission(identity, head.as_ref()) {
                        Some(Start::Pending(reason))
                    } else {
                        checked_head = head;
                        if fresh {
                            ledger::reserve(&tx, &key, identity, origin)?;
                        }
                        None
                    }
                }
            };
            tx.commit()?;
            Ok((result, checked_head))
        })?;
    if let Some(result) = result {
        return Ok(result);
    }
    let assets = materialization
        .runtime
        .assets(owner, materialization.assets)
        .map_err(publication::PublicationError::from)?;
    let current = if let Some(head) = &head {
        let state = match mdreader_core::device::state::current(owner, &head.document) {
            Ok(state) => state,
            Err(mdreader_core::device::Error::Unavailable) => {
                return Ok(Start::Pending(PendingReason::TargetUnavailable))
            }
            Err(error) => return Err(publication::PublicationError::from(error).into()),
        };
        if Some(state.generation) != head.generation {
            return Ok(Start::Pending(PendingReason::LocalStateChanged));
        }
        Some(publication::prepare_current(
            owner,
            &head.document,
            &assets,
            origin,
        )?)
    } else {
        None
    };
    let plan =
        publication::reserve_new(owner, materialization.documents, &assets, origin, document)?;
    owner.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        ledger::associate(&tx, &key, identity, origin, plan.document_id())?;
        tx.commit()?;
        Ok(())
    })?;
    Ok(match (head, current) {
        (Some(head), Some(current)) => Start::Replacement {
            plan: Box::new(plan),
            current: Box::new(current),
            head,
        },
        _ => Start::Plan(Box::new(plan)),
    })
}

pub(super) fn queue_index(owner: &ReaderStore, receipt: &Receipt) -> bool {
    if receipt.operation != Operation::Content {
        return false;
    }
    // Optional derived work after durable commit. A replay repairs an interrupted enqueue.
    if !owner.list_sources().is_ok_and(|sources| {
        sources
            .iter()
            .any(|source| source.id == receipt.document.source_id && source.enabled)
    }) {
        return false;
    }
    match owner.index_job(&receipt.document.source_id) {
        Ok(Some(_)) => true,
        Ok(None) => owner.queue_index(&receipt.document.source_id).is_ok(),
        Err(_) => false,
    }
}

pub(super) fn classify(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    change: &RemoteChange,
) -> Result<Option<PendingReason>> {
    let (exact, applied) = owner.with_library_connection(|db| staged(db, scope, change))?;
    Ok(if !exact {
        Some(PendingReason::Unstaged)
    } else if applied.checked_add(1) != Some(change.sequence) {
        Some(PendingReason::NotContiguous)
    } else {
        None
    })
}

pub(super) fn reconcile(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
) -> Result<Option<Box<Receipt>>> {
    let key = identity::scope_key(scope)?;
    owner.with_library_connection(|db| -> Result<Option<Box<Receipt>>> {
        let (exact, applied) = staged(db, scope, &identity.change)?;
        if !exact {
            return Err(Error::Conflict);
        }
        let tx = db.transaction()?;
        super::schema::require_library(&tx, scope.library_id())?;
        let receipt = match ledger::inspect(&tx, &key, identity, origin)? {
            Some(Entry::Applied(receipt)) if applied >= identity.change.sequence => Some(receipt),
            Some(Entry::Applied(_)) => return Err(Error::Conflict),
            _ => None,
        };
        tx.commit()?;
        Ok(receipt)
    })
}
