//! Exact staged metadata, receipt/cursor SQL and final commit fencing.
use super::{
    identity::{self, Identity},
    ledger::{self, Entry},
    storage::staged,
    types::*,
};
use mdreader_core::device::publication::IncomingOrigin;
use mdreader_sync::{RemoteChange, SyncScope, SyncStore};
use rusqlite::{Connection, Transaction};

/// Common single-owner SQL commit; no network, body hashing or decoding here.
pub(super) fn commit_on(
    db: &mut Connection,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
    apply: impl FnOnce(&Transaction<'_>) -> Result<Receipt>,
    final_validate: impl Fn(&Transaction<'_>, &Receipt) -> Result<()>,
    final_fence: impl Fn(&Transaction<'_>) -> Result<()>,
) -> Result<Receipt> {
    let key = identity::scope_key(scope)?;
    let (exact, applied) = staged(db, scope, &identity.change)?;
    if !exact || applied.checked_add(1) != Some(identity.change.sequence) {
        return Err(Error::Conflict);
    }
    let result = std::cell::RefCell::new(None);
    let mut sync = SyncStore::attach(db, scope.clone())?;
    sync.mark_applied_guarded(
        identity.change.sequence,
        |tx| -> Result<()> {
            require_start(tx, scope, identity, origin)?;
            let receipt = apply(tx)?;
            ledger::complete(tx, &key, identity, origin, &receipt)?;
            *result.borrow_mut() = Some(receipt);
            Ok(())
        },
        |tx| -> Result<()> {
            let value = result.borrow();
            let receipt = value.as_ref().ok_or(Error::Conflict)?;
            verify_applied(tx, scope, identity, origin, receipt)?;
            final_validate(tx, receipt)?;
            final_fence(tx)
        },
    )?;
    result.into_inner().ok_or(Error::Conflict)
}

pub(super) fn require_start(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
) -> Result<()> {
    let current: u64 = tx.query_row(
        "SELECT applied FROM mdreader_sync_cursors WHERE account=?1 AND library=?2 AND device=?3",
        rusqlite::params![
            scope.account_digest(),
            scope.library_id(),
            scope.device_id()
        ],
        |r| r.get(0),
    )?;
    if current.checked_add(1) != Some(identity.change.sequence)
        || !matches!(
            ledger::inspect(tx, &identity::scope_key(scope)?, identity, origin)?,
            Some(Entry::Reserved)
        )
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(super) fn verify_applied(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
    receipt: &Receipt,
) -> Result<()> {
    super::schema::require_library(tx, scope.library_id())?;
    verify_staged_commit(tx, scope, &identity.change)?;
    ledger::verify(tx, &identity::scope_key(scope)?, identity, origin, receipt)
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
