//! Explicitly retain local pin/position; archive peer state in the exact ledger.
use super::{identity::Identity, resolution, transition_start::Kind, *};

impl IncomingCoordinator {
    pub(super) fn resolve_local_state(
        &self,
        selected: &SelectedIncoming,
        input: &PreparedIncoming,
        request: &ResolutionRequest,
        identity: &Identity,
        origin: &publication::IncomingOrigin,
    ) -> Result<Receipt> {
        let key = scope_key(input.scope())?;
        let (target, previous, receipt, current, archive) = self.short(selected, |owner, _| {
            resolution::view(owner, input, origin)?.require(request)?;
            let (target, previous, receipt) = owner.with_library_connection(|db| -> Result<_> {
                let tx = db.transaction()?;
                let Kind::State {
                    target,
                    previous,
                    receipt,
                    ..
                } = transition_start::classify(&tx, &key, identity)?
                    .map_err(|_| Error::Conflict)?
                else {
                    return Err(Error::Conflict);
                };
                crate::sync::outgoing::association::require_unqueued(
                    &tx,
                    input.scope(),
                    &target.document,
                )?;
                tx.commit()?;
                Ok((target, previous, receipt))
            })?;
            let assets = self
                .runtime
                .assets(owner, &self.assets)
                .map_err(publication::PublicationError::from)?;
            let current = publication::prepare_current(owner, &target.document, &assets, origin)?;
            let archive =
                publication::prepare_archive(owner, &target.document, &assets, &self.runtime)?;
            owner.with_library_connection(|db| -> Result<()> {
                let tx = db.transaction()?;
                if ledger::inspect(&tx, &key, identity, origin)?.is_none() {
                    ledger::reserve(&tx, &key, identity, origin)?;
                }
                ledger::associate(&tx, &key, identity, origin, &target.document)?;
                tx.commit()?;
                Ok(())
            })?;
            Ok((target, previous, receipt, current, archive))
        })?;
        // Retained actor blocking work owns this operation; no owner lock during reads.
        let current = current.verify()?;
        let archive = archive.read()?;
        self.short(selected, |owner, session| {
            publication::with_transition(owner, &current, origin, |db, guard| {
                let mut content = target.clone();
                content.generation = Some(current.state().generation);
                let state = heads::Head {
                    item: identity.change.item_id.clone(),
                    kind: "user_state".into(),
                    revision: identity.change.revision_id.clone(),
                    document: target.document.clone(),
                    target: Some(target.item.clone()),
                    generation: content.generation,
                    tombstone: None,
                };
                commit::commit_on(
                    db,
                    input.scope(),
                    identity,
                    origin,
                    |tx| {
                        heads::verify(tx, &key, &target)?;
                        crate::sync::outgoing::association::require_unqueued(
                            tx,
                            input.scope(),
                            &target.document,
                        )?;
                        publication::revalidate_current_archive(tx, &guard, &current, &archive)?;
                        let mut applied = receipt.clone();
                        applied.sequence = identity.change.sequence;
                        applied.wire_item_id = identity.change.item_id.clone();
                        applied.wire_revision = identity.change.revision_id.clone();
                        applied.operation = Operation::UserStatePreserved;
                        applied.document = current.state().document.clone();
                        applied.generation = content.generation;
                        applied.retirement = None;
                        heads::save(tx, &key, Some(&target), &content)?;
                        heads::save(tx, &key, previous.as_ref(), &state)?;
                        Ok(applied)
                    },
                    |tx, _| {
                        heads::verify(tx, &key, &content)?;
                        heads::verify(tx, &key, &state)?;
                        crate::sync::outgoing::association::require_unqueued(
                            tx,
                            input.scope(),
                            &target.document,
                        )?;
                        publication::revalidate_current_archive(tx, &guard, &current, &archive)?;
                        Ok(())
                    },
                    |tx| {
                        session.check()?;
                        schema::require_current_library(tx, input.scope().library_id())
                    },
                )
            })?
        })
    }
}
