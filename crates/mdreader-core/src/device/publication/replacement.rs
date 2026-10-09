//! Immutable revision replacement: publish new, retire old, never overwrite files.
use super::{
    current::{bind_current, verify_live},
    *,
};
use crate::{
    device::{
        self,
        state::{RetirementKind, RetirementRecord},
    },
    ReaderStore,
};
use rusqlite::{Connection, Transaction};
use std::{marker::PhantomData, sync::Arc};

pub struct ReplacementGuard<'owner> {
    pub(super) new: PublicationGuard<'owner>,
    pub(super) old: CurrentGuard<'owner>,
}
pub struct ReplacementReceipt {
    publication: PublicationReceipt,
    retirement: RetirementReceipt,
}
impl ReplacementReceipt {
    pub fn publication(&self) -> &PublicationReceipt {
        &self.publication
    }
    pub fn retirement(&self) -> &RetirementReceipt {
        &self.retirement
    }
}
pub fn with_replacement<T, E>(
    store: &mut ReaderStore,
    ready: &ReadyIncoming,
    current: &VerifiedCurrent,
    origin: &IncomingOrigin,
    action: impl for<'owner> FnOnce(
        &'owner mut Connection,
        ReplacementGuard<'owner>,
    ) -> std::result::Result<T, E>,
) -> PublicationResult<std::result::Result<T, E>> {
    bind_current(store, current, origin)?;
    if !Arc::ptr_eq(&ready.plan.owner, &current.plan.owner)
        || ready.plan.intent.id == current.plan.state.document.id
    {
        return Err(PublicationError::Conflict);
    }
    current.plan.assets.incoming_owner(store)?;
    with_publication(store, ready, origin, |db, new| {
        let old = CurrentGuard {
            connection: db as *const Connection as usize,
            owner: current.plan.owner.clone(),
            origin: current.plan.origin.clone(),
            _borrow: PhantomData,
        };
        action(db, ReplacementGuard { new, old })
    })
}
pub fn publish_replacement(
    tx: &Transaction<'_>,
    guard: &ReplacementGuard<'_>,
    ready: &ReadyIncoming,
    current: &VerifiedCurrent,
) -> PublicationResult<ReplacementReceipt> {
    verify_live(tx, &guard.old, current)?;
    let mut publication = publish_new(tx, &guard.new, ready)?;
    let new = device::state::load(tx, &publication.document.id, true)?;
    // Pin is copied exactly under state CAS; an old revision position is archived,
    // never projected onto new content, even when raw content hashes happen to match.
    device::state::update_in(tx, &new, current.plan.state.document.pinned, 1)?;
    publication.document = device::state::load(tx, &new.document.id, true)?.document;
    let origin: IncomingOrigin = serde_json::from_str(&guard.old.origin)?;
    let retirement = retire_current(
        tx,
        &guard.old,
        current,
        RetirementRecord {
            kind: RetirementKind::Replaced,
            operation_id: origin.wire_revision,
            replacement: Some(
                uuid::Uuid::parse_str(&publication.document.id)
                    .map_err(|_| PublicationError::Conflict)?,
            ),
        },
    )?;
    let receipt = ReplacementReceipt {
        publication,
        retirement,
    };
    revalidate_replacement(tx, guard, ready, current, &receipt)?;
    Ok(receipt)
}
pub fn revalidate_replacement(
    tx: &Transaction<'_>,
    guard: &ReplacementGuard<'_>,
    ready: &ReadyIncoming,
    current: &VerifiedCurrent,
    receipt: &ReplacementReceipt,
) -> PublicationResult<()> {
    super::publish::revalidate_with_state(
        tx,
        &guard.new,
        ready,
        current.plan.state.document.pinned,
        1,
        1,
    )?;
    if receipt.publication.document.id != ready.document_id()
        || receipt.publication.document.revision != ready.revision()
        || receipt.publication.document.pinned != current.plan.state.document.pinned
        || receipt.publication.document.line != 1
        || receipt
            .retirement
            .state
            .retirement
            .as_ref()
            .and_then(|r| r.replacement)
            .map(|id| id.to_string())
            .as_deref()
            != Some(ready.document_id())
    {
        return Err(PublicationError::Conflict);
    }
    revalidate_retirement(tx, &guard.old, current, &receipt.retirement)
}
