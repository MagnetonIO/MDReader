//! Same-document receipt/head/cursor transaction, with no source or file writes.
use super::*;
use mdreader_core::device::publication::VerifiedCurrent;

pub(in crate::sync::incoming) struct Echo<'a> {
    pub intent: &'a Intent,
    pub current: &'a VerifiedCurrent,
    pub snapshot: &'a Snapshot,
}
pub(in crate::sync::incoming) fn commit(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &identity::Identity,
    origin: &publication::IncomingOrigin,
    echo: Echo<'_>,
    final_fence: impl Fn(&Transaction<'_>) -> Result<()>,
) -> Result<Receipt> {
    let Echo {
        intent,
        current,
        snapshot,
    } = echo;
    if !snapshot.matches(intent) || !matches_identity(snapshot, intent, identity) {
        return Err(Error::Conflict);
    }
    publication::with_transition(owner, current, origin, |db, guard| {
        let mut sync = mdreader_sync::SyncStore::attach(db, scope.clone())?;
        sync.with_applied_transaction(&identity.change, |tx, advance| -> Result<Receipt> {
            commit::require_start(tx, scope, identity, origin)?;
            association::verify(tx, scope, intent, None)?;
            // This proof genuinely borrows this transaction and stays on this
            // callback's stack through cursor writes and the final byte fence.
            let proof = mdreader_sync::published_graph_proof_in(
                tx,
                scope,
                &intent.operation.operation_id,
                &identity.change,
            )?;
            if proof.entry().operation != intent.operation {
                return Err(Error::Conflict);
            }
            outgoing_selection::verify_head(tx, &identity::scope_key(scope)?, intent)?;
            publication::revalidate_current_archive(tx, &guard, current, &snapshot.archive)?;
            let receipt = Receipt {
                sequence: identity.change.sequence,
                wire_item_id: identity.change.item_id.clone(),
                wire_revision: identity.change.revision_id.clone(),
                document: current.state().document.clone(),
                markdown_sha256: snapshot.markdown_sha256.clone(),
                assets: snapshot
                    .materialized
                    .iter()
                    .map(|a| a.identity.clone())
                    .collect(),
                materialized_assets: snapshot.materialized.clone(),
                operation: if intent.state.is_some() {
                    Operation::UserState
                } else {
                    Operation::Content
                },
                generation: Some(current.state().generation),
                retirement: None,
            };
            let key = identity::scope_key(scope)?;
            let published = head(&receipt, intent);
            if let Some(state) = &intent.state {
                let mut target = state.target.clone();
                target.generation = receipt.generation;
                heads::save(tx, &key, Some(&state.target), &target)?;
            }
            heads::save(tx, &key, intent.head.as_ref(), &published)?;
            association::complete(tx, scope, intent, identity.change.sequence)?;
            ledger::complete(tx, &key, identity, origin, &receipt)?;
            advance.advance()?;
            commit::verify_applied(tx, scope, identity, origin, &receipt)?;
            association::verify(tx, scope, intent, Some(identity.change.sequence))?;
            proof.revalidate()?;
            heads::verify(tx, &key, &published)?;
            if let Some(state) = &intent.state {
                let mut target = state.target.clone();
                target.generation = receipt.generation;
                heads::verify(tx, &key, &target)?;
            }
            publication::revalidate_current_archive(tx, &guard, current, &snapshot.archive)?;
            final_fence(tx)?;
            Ok(receipt)
        })
    })?
}

fn head(receipt: &Receipt, intent: &Intent) -> Head {
    Head {
        item: receipt.wire_item_id.clone(),
        kind: if intent.state.is_some() {
            "user_state"
        } else {
            "content"
        }
        .into(),
        revision: receipt.wire_revision.clone(),
        document: receipt.document.id.clone(),
        target: intent.state.as_ref().map(|state| state.target.item.clone()),
        generation: receipt.generation,
        tombstone: None,
    }
}
