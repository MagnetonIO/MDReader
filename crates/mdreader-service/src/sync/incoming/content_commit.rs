//! Immutable content publication and replacement on the caller-owned transaction.
use super::{
    commit::commit_on,
    heads::{self, Head},
    identity::{self, Identity},
    types::*,
};
use mdreader_core::{
    device::publication::{self, IncomingOrigin, ReadyIncoming, VerifiedCurrent},
    ReaderStore,
};
use mdreader_sync::SyncScope;
use rusqlite::Transaction;

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
    publication::with_publication(owner, ready, origin, |db, guard| {
        commit_on(
            db,
            scope,
            identity,
            origin,
            |tx| {
                let published = publication::publish_new(tx, &guard, ready)?;
                let receipt = content_receipt(identity, published, 0);
                let head = content_head(&receipt);
                heads::save(tx, &identity::scope_key(scope)?, None, &head)?;
                Ok(receipt)
            },
            |tx, receipt| {
                heads::verify(tx, &identity::scope_key(scope)?, &content_head(receipt))?;
                publication::revalidate_publication(tx, &guard, ready)?;
                Ok(())
            },
            final_fence,
        )
    })?
}
pub(super) fn content_receipt(
    identity: &Identity,
    published: publication::PublicationReceipt,
    generation: u64,
) -> Receipt {
    let materialized_assets = published
        .assets
        .into_iter()
        .map(|asset| MaterializedAsset {
            asset_id: asset.asset_id,
            identity: AssetIdentity {
                target: asset.target,
                mime: asset.mime,
                sha256: asset.content_hash,
                bytes: asset.bytes,
            },
            width: asset.width,
            height: asset.height,
        })
        .collect();
    Receipt {
        sequence: identity.change.sequence,
        wire_item_id: identity.change.item_id.clone(),
        wire_revision: identity.change.revision_id.clone(),
        document: published.document,
        markdown_sha256: identity.markdown_sha256.clone(),
        assets: identity.assets.clone(),
        materialized_assets,
        operation: Operation::Content,
        generation: Some(generation),
        retirement: None,
    }
}
pub(super) fn content_head(receipt: &Receipt) -> Head {
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
pub(super) struct Replacement<'a> {
    pub ready: &'a ReadyIncoming,
    pub current: &'a VerifiedCurrent,
    pub head: &'a Head,
}
pub(super) fn commit_replacement(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &Identity,
    origin: &IncomingOrigin,
    replacement: Replacement<'_>,
    final_fence: impl Fn(&Transaction<'_>) -> Result<()>,
) -> Result<Receipt> {
    let Replacement {
        ready,
        current,
        head,
    } = replacement;
    publication::with_replacement(owner, ready, current, origin, |db, guard| {
        let proof = std::cell::RefCell::new(None);
        commit_on(
            db,
            scope,
            identity,
            origin,
            |tx| {
                let key = identity::scope_key(scope)?;
                heads::verify(tx, &key, head)?;
                if Some(current.state().generation) != head.generation
                    || heads::content_admission(identity, Some(head)).is_some()
                {
                    return Err(Error::Conflict);
                }
                let replaced = publication::publish_replacement(tx, &guard, ready, current)?;
                let receipt = content_receipt(
                    identity,
                    publication::PublicationReceipt {
                        document: replaced.publication().document.clone(),
                        assets: replaced.publication().assets.clone(),
                    },
                    1,
                );
                heads::save(tx, &key, Some(head), &content_head(&receipt))?;
                *proof.borrow_mut() = Some(replaced);
                Ok(receipt)
            },
            |tx, receipt| {
                heads::verify(tx, &identity::scope_key(scope)?, &content_head(receipt))?;
                publication::revalidate_replacement(
                    tx,
                    &guard,
                    ready,
                    current,
                    proof.borrow().as_ref().ok_or(Error::Conflict)?,
                )?;
                Ok(())
            },
            final_fence,
        )
    })?
}
