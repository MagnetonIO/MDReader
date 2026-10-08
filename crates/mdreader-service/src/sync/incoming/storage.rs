use super::{
    identity::{self, Identity},
    ledger::{self, Entry},
    types::*,
};
use mdreader_core::{
    device::{
        assets::AssetRuntime,
        publication::{self, IncomingDocument, IncomingOrigin, ReadyIncoming, WritePlan},
    },
    ReaderStore,
};
use mdreader_sync::{RemoteChange, SyncScope, SyncStore};
use rusqlite::{Connection, Transaction};
use std::path::Path;

pub(super) enum Start {
    Plan(Box<WritePlan>),
    Applied(Box<Receipt>),
    Pending(PendingReason),
}

pub(super) struct Materialization<'a> {
    pub documents: &'a Path,
    pub assets: &'a Path,
    pub runtime: &'a AssetRuntime,
}

fn staged(db: &mut Connection, scope: &SyncScope, change: &RemoteChange) -> Result<(bool, u64)> {
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
    let result = owner.with_library_connection(|db| -> Result<Option<Start>> {
        let (exact, applied) = staged(db, scope, &identity.change)?;
        if !exact {
            return Ok(Some(Start::Pending(PendingReason::Unstaged)));
        }
        let tx = db.transaction()?;
        super::schema::require_library(&tx, scope.library_id())?;
        let entry = ledger::inspect(&tx, &key, identity, origin)?;
        let result = match entry {
            Some(Entry::Applied(receipt)) if applied >= identity.change.sequence => {
                Some(Start::Applied(receipt))
            }
            Some(Entry::Applied(_)) => return Err(Error::Conflict),
            Some(Entry::Pending(reason)) => Some(Start::Pending(reason)),
            _ if applied.checked_add(1) != Some(identity.change.sequence) => {
                Some(Start::Pending(PendingReason::NotContiguous))
            }
            Some(Entry::Reserved) => None,
            None => {
                ledger::reserve(&tx, &key, identity, origin)?;
                None
            }
        };
        tx.commit()?;
        Ok(result)
    })?;
    if let Some(result) = result {
        return Ok(result);
    }
    let assets = materialization
        .runtime
        .assets(owner, materialization.assets)
        .map_err(publication::PublicationError::from)?;
    let plan =
        publication::reserve_new(owner, materialization.documents, &assets, origin, document)?;
    owner.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        ledger::associate(&tx, &key, identity, origin, plan.document_id())?;
        tx.commit()?;
        Ok(())
    })?;
    Ok(Start::Plan(Box::new(plan)))
}

/// Private seam used by production after expensive materialization, and by storage
/// fixtures. The injected final fence in tests is not a native identity capability.
pub(super) fn commit(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
    ready: &ReadyIncoming,
    final_fence: impl Fn(&Transaction<'_>) -> Result<()>,
) -> Result<Receipt> {
    let key = identity::scope_key(scope)?;
    let (exact, applied) =
        owner.with_library_connection(|db| staged(db, scope, &identity.change))?;
    if !exact || applied.checked_add(1) != Some(identity.change.sequence) {
        return Err(Error::Conflict);
    }
    let result = std::cell::RefCell::new(None);
    publication::with_publication(owner, ready, origin, |db, guard| -> Result<()> {
        let mut sync = SyncStore::attach(db, scope.clone())?;
        sync.mark_applied_guarded(identity.change.sequence,|tx|->Result<()> {
            // Guard the contiguous sequence inside the actual publication transaction.
            let current:u64=tx.query_row("SELECT applied FROM mdreader_sync_cursors WHERE account=?1 AND library=?2 AND device=?3",rusqlite::params![scope.account_digest(),scope.library_id(),scope.device_id()],|r|r.get(0))?;
            if current.checked_add(1)!=Some(identity.change.sequence) { return Err(Error::Conflict); }
            if !matches!(ledger::inspect(tx,&key,identity,origin)?,Some(Entry::Reserved)) { return Err(Error::Conflict); }
            let published=publication::publish_new(tx,&guard,ready)?;
            let materialized_assets=published.assets.into_iter().map(|asset|MaterializedAsset {asset_id:asset.asset_id,identity:AssetIdentity {target:asset.target,mime:asset.mime,sha256:asset.content_hash,bytes:asset.bytes},width:asset.width,height:asset.height}).collect();
            let receipt=Receipt {sequence:identity.change.sequence,wire_item_id:identity.change.item_id.clone(),wire_revision:identity.change.revision_id.clone(),document:published.document,markdown_sha256:identity.markdown_sha256.clone(),assets:identity.assets.clone(),materialized_assets};
            ledger::complete(tx,&key,identity,origin,&receipt)?;
            *result.borrow_mut()=Some(receipt);
            Ok(())
        },|tx|->Result<()> {
            super::schema::require_library(tx,scope.library_id())?;
            verify_staged_commit(tx,scope,&identity.change)?;
            let receipt=result.borrow();
            ledger::verify(tx,&key,identity,origin,receipt.as_ref().ok_or(Error::Conflict)?)?;
            publication::revalidate_publication(tx,&guard,ready)?;
            final_fence(tx)
        })?;
        Ok(())
    })??;
    result.into_inner().ok_or(Error::Conflict)
}

fn verify_staged_commit(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    change: &RemoteChange,
) -> Result<()> {
    let exact:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_remote WHERE account=?1 AND library=?2 AND device=?3 AND sequence=?4 AND revision=?5 AND item=?6 AND blob IS ?7 AND deleted=?8 AND parents=?9 AND source_device=?10)",rusqlite::params![scope.account_digest(),scope.library_id(),scope.device_id(),change.sequence,change.revision_id,change.item_id,change.blob,change.deleted,serde_json::to_string(&change.parents)?,change.device],|r|r.get(0))?;
    let applied: u64 = tx.query_row(
        "SELECT applied FROM mdreader_sync_cursors WHERE account=?1 AND library=?2 AND device=?3",
        rusqlite::params![
            scope.account_digest(),
            scope.library_id(),
            scope.device_id()
        ],
        |r| r.get(0),
    )?;
    if !exact || applied != change.sequence {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(super) fn queue_index(owner: &ReaderStore, receipt: &Receipt) -> bool {
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
