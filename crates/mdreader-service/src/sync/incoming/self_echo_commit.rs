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
        commit::commit_on(
            db,
            scope,
            identity,
            origin,
            |tx| {
                association::verify(tx, scope, intent, None)?;
                publication_proof(tx, scope, intent, &identity.change)?;
                outgoing_selection::verify_head(tx, &identity::scope_key(scope)?, intent)?;
                publication::revalidate_current_archive(tx, &guard, current, &snapshot.archive)?;
                // The native UUID/source/BLAKE3 revision/asset UUIDs remain unchanged.
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
                heads::save(
                    tx,
                    &identity::scope_key(scope)?,
                    intent.head.as_ref(),
                    &head(&receipt),
                )?;
                association::complete(tx, scope, intent, identity.change.sequence)?;
                Ok(receipt)
            },
            |tx, receipt| {
                association::verify(tx, scope, intent, Some(identity.change.sequence))?;
                publication_proof(tx, scope, intent, &identity.change)?;
                heads::verify(tx, &identity::scope_key(scope)?, &head(receipt))?;
                publication::revalidate_current_archive(tx, &guard, current, &snapshot.archive)?;
                Ok(())
            },
            final_fence,
        )
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
