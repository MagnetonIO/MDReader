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
    if !snapshot.matches(intent) || !matches_identity(snapshot, identity) {
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
                markdown_sha256: identity.markdown_sha256.clone(),
                assets: identity.assets.clone(),
                materialized_assets: snapshot.materialized.clone(),
                operation: Operation::Content,
                generation: Some(current.state().generation),
                retirement: None,
            };
            let key = identity::scope_key(scope)?;
            heads::save(tx, &key, intent.head.as_ref(), &head(&receipt))?;
            association::complete(tx, scope, intent, identity.change.sequence)?;
            ledger::complete(tx, &key, identity, origin, &receipt)?;
            advance.advance()?;
            commit::verify_applied(tx, scope, identity, origin, &receipt)?;
            association::verify(tx, scope, intent, Some(identity.change.sequence))?;
            proof.revalidate()?;
            heads::verify(tx, &key, &head(&receipt))?;
            publication::revalidate_current_archive(tx, &guard, current, &snapshot.archive)?;
            final_fence(tx)?;
            Ok(receipt)
        })
    })?
}

fn head(receipt: &Receipt) -> Head {
    Head {
        item: receipt.wire_item_id.clone(),
        kind: "content".into(),
        revision: receipt.wire_revision.clone(),
        document: receipt.document.id.clone(),
        target: None,
        generation: receipt.generation,
        tombstone: None,
    }
}
