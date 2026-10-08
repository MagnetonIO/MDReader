use super::{
    heads::{self, Head},
    identity::{self, Identity},
    storage,
    transition_start::{Kind, Verified},
    types::*,
};
use mdreader_core::{
    device::{
        publication::{self, IncomingOrigin},
        state::{RetirementKind, RetirementRecord},
    },
    ReaderStore,
};
use mdreader_sync::SyncScope;
use rusqlite::Transaction;

pub(super) fn pending(plan: &Verified) -> Option<PendingReason> {
    if let Kind::State {
        line: Some(line), ..
    } = &plan.kind
    {
        if plan
            .current
            .as_ref()
            .is_none_or(|current| *line == 0 || *line > current.max_line())
        {
            return Some(PendingReason::PositionStale);
        }
    }
    None
}

fn base(identity: &Identity, receipt: &Receipt, operation: Operation) -> Receipt {
    let mut receipt = receipt.clone();
    receipt.sequence = identity.change.sequence;
    receipt.wire_item_id = identity.change.item_id.clone();
    receipt.wire_revision = identity.change.revision_id.clone();
    receipt.operation = operation;
    receipt.retirement = None;
    receipt
}
fn state_head(identity: &Identity, receipt: &Receipt, target: &Head, tombstone: bool) -> Head {
    Head {
        item: identity.change.item_id.clone(),
        kind: "user_state".into(),
        revision: identity.change.revision_id.clone(),
        document: receipt.document.id.clone(),
        target: Some(target.item.clone()),
        generation: receipt.generation,
        tombstone: tombstone.then(|| identity.change.revision_id.clone()),
    }
}
pub(super) fn commit(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
    plan: &Verified,
    final_fence: impl Fn(&Transaction<'_>) -> Result<()>,
) -> Result<Receipt> {
    let key = identity::scope_key(scope)?;
    match &plan.kind {
        Kind::State {
            previous,
            target,
            receipt,
            pinned,
            line,
        } => {
            let current = plan.current.as_ref().ok_or(Error::Conflict)?;
            publication::with_transition(owner, current, origin, |db, guard| {
                let proof = std::cell::RefCell::new(None);
                storage::commit_on(
                    db,
                    scope,
                    identity,
                    origin,
                    |tx| {
                        heads::verify(tx, &key, target)?;
                        if Some(current.state().generation) != target.generation {
                            return Err(Error::Conflict);
                        }
                        let position = line
                            .map(publication::PositionUpdate::Line)
                            .unwrap_or(publication::PositionUpdate::Start);
                        let published = publication::publish_state(
                            tx,
                            &guard,
                            current,
                            publication::StateUpdate {
                                pinned: *pinned,
                                position,
                            },
                        )?;
                        let mut applied = base(identity, receipt, Operation::UserState);
                        applied.document = published.state().document.clone();
                        applied.generation = Some(published.state().generation);
                        let mut content = target.clone();
                        content.generation = applied.generation;
                        heads::save(tx, &key, Some(target), &content)?;
                        heads::save(
                            tx,
                            &key,
                            previous.as_ref(),
                            &state_head(identity, &applied, target, false),
                        )?;
                        *proof.borrow_mut() = Some(published);
                        Ok(applied)
                    },
                    |tx, applied| {
                        let mut content = target.clone();
                        content.generation = applied.generation;
                        heads::verify(tx, &key, &content)?;
                        heads::verify(tx, &key, &state_head(identity, applied, target, false))?;
                        publication::revalidate_state(
                            tx,
                            &guard,
                            current,
                            proof.borrow().as_ref().ok_or(Error::Conflict)?,
                        )?;
                        Ok(())
                    },
                    final_fence,
                )
            })?
        }
        Kind::DocumentDelete { target, receipt } => {
            let current = plan.current.as_ref().ok_or(Error::Conflict)?;
            publication::with_transition(owner, current, origin, |db, guard| {
                let proof = std::cell::RefCell::new(None);
                storage::commit_on(
                    db,
                    scope,
                    identity,
                    origin,
                    |tx| {
                        heads::verify(tx, &key, target)?;
                        if Some(current.state().generation) != target.generation {
                            return Err(Error::Conflict);
                        }
                        let record = RetirementRecord {
                            kind: RetirementKind::Deleted,
                            operation_id: origin.wire_revision,
                            replacement: None,
                        };
                        let retired =
                            publication::retire_current(tx, &guard, current, record.clone())?;
                        let mut applied = base(identity, receipt, Operation::DocumentDeleted);
                        applied.document = retired.state().document.clone();
                        applied.generation = Some(retired.state().generation);
                        applied.retirement = Some(record);
                        let mut head = target.clone();
                        head.revision = identity.change.revision_id.clone();
                        head.generation = applied.generation;
                        heads::save(tx, &key, Some(target), &head)?;
                        *proof.borrow_mut() = Some(retired);
                        Ok(applied)
                    },
                    |tx, applied| {
                        let mut head = target.clone();
                        head.revision = identity.change.revision_id.clone();
                        head.generation = applied.generation;
                        heads::verify(tx, &key, &head)?;
                        publication::revalidate_retirement(
                            tx,
                            &guard,
                            current,
                            proof.borrow().as_ref().ok_or(Error::Conflict)?,
                        )?;
                        Ok(())
                    },
                    final_fence,
                )
            })?
        }
        Kind::StateDelete { previous, receipt } => owner.with_library_connection(|db| {
            storage::commit_on(
                db,
                scope,
                identity,
                origin,
                |tx| {
                    heads::verify(tx, &key, previous)?;
                    let applied = base(identity, receipt, Operation::UserStateDeleted);
                    let mut head = previous.clone();
                    head.revision = identity.change.revision_id.clone();
                    head.tombstone = Some(identity.change.revision_id.clone());
                    heads::save(tx, &key, Some(previous), &head)?;
                    Ok(applied)
                },
                |tx, _| {
                    let mut head = previous.clone();
                    head.revision = identity.change.revision_id.clone();
                    head.tombstone = Some(identity.change.revision_id.clone());
                    heads::verify(tx, &key, &head)
                },
                final_fence,
            )
        }),
    }
}
