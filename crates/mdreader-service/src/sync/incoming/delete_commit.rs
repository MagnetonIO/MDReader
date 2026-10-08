//! One retirement/selected-intent/ledger/applied-cursor transaction. Originals
//! remain retained; exact tombstone proof lives through every write/final fence.
use super::{commit, heads, identity, ledger, types::*};
use crate::sync::outgoing::association;
use mdreader_core::{
    device::{
        publication::{self, IncomingOrigin, VerifiedCurrent},
        state::{RetirementKind, RetirementRecord},
    },
    ReaderStore,
};
use mdreader_sync::{SyncScope, SyncStore};
use rusqlite::Transaction;

pub(super) struct Deletion<'a> {
    pub current: &'a VerifiedCurrent,
    pub target: &'a heads::Head,
    pub previous: &'a Receipt,
}
pub(super) fn commit(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &identity::Identity,
    origin: &IncomingOrigin,
    deletion: Deletion<'_>,
    final_fence: impl Fn(&Transaction<'_>) -> Result<()>,
) -> Result<Receipt> {
    let Deletion {
        current,
        target,
        previous,
    } = deletion;
    publication::with_transition(owner, current, origin, |db, guard| {
        let mut sync = SyncStore::attach(db, scope.clone())?;
        sync.with_applied_transaction(&identity.change, |tx, advance| -> Result<Receipt> {
            commit::require_start(tx, scope, identity, origin)?;
            let selected = association::by_revision(tx, scope, &identity.change.revision_id)?;
            let proof = if let Some(stored) = &selected {
                let intent = &stored.intent;
                let mut expected = intent.head.clone().ok_or(Error::Conflict)?;
                expected.tombstone = Some(identity.change.revision_id.clone());
                if stored.applied.is_some()
                    || !intent.operation.deleted
                    || intent.origin != *origin
                    || expected != *target
                    || intent.native_document != current.state().document.id
                    || intent.native_revision != current.state().document.revision
                    || intent.generation != current.state().generation
                {
                    return Err(Error::Conflict);
                }
                association::verify(tx, scope, intent, None)?;
                let proof = mdreader_sync::published_tombstone_proof_in(
                    tx,
                    scope,
                    &intent.operation.operation_id,
                    &identity.change,
                )?;
                if proof.entry().operation != intent.operation {
                    return Err(Error::Conflict);
                }
                Some(proof)
            } else {
                None
            };
            let key = identity::scope_key(scope)?;
            heads::verify(tx, &key, target)?;
            if Some(current.state().generation) != target.generation {
                return Err(Error::Conflict);
            }
            let record = RetirementRecord {
                kind: RetirementKind::Deleted,
                operation_id: origin.wire_revision,
                replacement: None,
            };
            let retired = publication::retire_current(tx, &guard, current, record.clone())?;
            let mut applied = previous.clone();
            applied.sequence = identity.change.sequence;
            applied.wire_item_id = identity.change.item_id.clone();
            applied.wire_revision = identity.change.revision_id.clone();
            applied.operation = Operation::DocumentDeleted;
            applied.document = retired.state().document.clone();
            applied.generation = Some(retired.state().generation);
            applied.retirement = Some(record);
            let mut head = target.clone();
            head.revision = identity.change.revision_id.clone();
            head.generation = applied.generation;
            heads::save(tx, &key, Some(target), &head)?;
            if let Some(stored) = &selected {
                association::complete(tx, scope, &stored.intent, identity.change.sequence)?;
            }
            ledger::complete(tx, &key, identity, origin, &applied)?;
            advance.advance()?;
            commit::verify_applied(tx, scope, identity, origin, &applied)?;
            if let Some(stored) = &selected {
                association::verify(tx, scope, &stored.intent, Some(identity.change.sequence))?;
            }
            if let Some(proof) = &proof {
                proof.revalidate()?;
            }
            heads::verify(tx, &key, &head)?;
            publication::revalidate_retirement(tx, &guard, current, &retired)?;
            final_fence(tx)?;
            Ok(applied)
        })
    })?
}
