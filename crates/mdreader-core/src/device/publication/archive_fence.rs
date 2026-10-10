//! Read-only final validation of exact current originals and asset files.
use super::*;
use std::sync::Arc;

/// Validate the same owner, document, asset metadata and retained opened files
/// used for a selected snapshot. Call before provisional writes and immediately
/// before commit on the actual borrowed owner transaction. This performs no
/// hashing, decoding, migration, source admission or writes. It never commits.
///
/// Ordinary archival export may preserve missing/incomplete resources. This
/// current-publication fence refuses them rather than certifying absent bytes.
pub fn revalidate_current_archive(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
    archive: &ArchivedDocument,
) -> PublicationResult<()> {
    if !Arc::ptr_eq(&archive.owner, &current.plan.owner)
        || archive.state != current.plan.state
        || archive.asset_state != current.plan.asset_state
    {
        return Err(PublicationError::Conflict);
    }
    if archive.assets().iter().any(|asset| {
        asset.state != "ready"
            || asset.received != asset.expected_bytes
            || asset.bytes().is_none()
            || asset.expected_sha256.is_none()
            || asset.mime.is_none()
            || asset.width.is_none_or(|n| n == 0)
            || asset.height.is_none_or(|n| n == 0)
    }) {
        return Err(PublicationError::Incomplete);
    }
    revalidate_current(tx, guard, current)?;
    archive.verify_assets_in(tx)
}
impl ArchivedDocument {
    /// Final owner-bound archival fence. Unlike live publication, partial and
    /// retired records are preserved exactly without granting read authority.
    pub fn revalidate(&self, store: &mut crate::ReaderStore) -> PublicationResult<()> {
        if !Arc::ptr_eq(&self.owner, &store.device_owner_session()) {
            return Err(PublicationError::Conflict);
        }
        self.asset_store.incoming_owner(store)?;
        store.with_library_connection(|db| -> PublicationResult<()> {
            let tx = db.transaction()?;
            if crate::device::state::load(&tx, &self.state.document.id, false)? != self.state {
                return Err(PublicationError::Conflict);
            }
            let root = super::current::source_root(&tx, &self.state.document.source_id)?;
            if root.path != self.root.path || root.identity != self.root.identity {
                return Err(PublicationError::Conflict);
            }
            self.root.verify()?;
            self.verify_assets_in(&tx)?;
            tx.commit()?;
            Ok(())
        })
    }
    fn verify_assets_in(&self, tx: &Transaction<'_>) -> PublicationResult<()> {
        self.asset_store.incoming_verify_tx(tx)?;
        self.asset_store
            .verify_current_assets_in(tx, &self.asset_state)?;
        #[cfg(windows)]
        self.asset_store.verify_archived_bytes_in(
            tx,
            &self.asset_state,
            &self
                .assets()
                .iter()
                .map(|asset| asset.bytes())
                .collect::<Vec<_>>(),
        )?;
        for evidence in &self.evidence {
            evidence.verify()?;
        }
        Ok(())
    }
    pub(super) fn verify_retired_assets_in(&self, tx: &Transaction<'_>) -> PublicationResult<()> {
        self.asset_store.incoming_verify_tx(tx)?;
        self.asset_store
            .verify_retired_assets_in(tx, &self.asset_state)?;
        #[cfg(windows)]
        self.asset_store.verify_retired_archived_bytes_in(
            tx,
            &self.asset_state,
            &self
                .assets()
                .iter()
                .map(|asset| asset.bytes())
                .collect::<Vec<_>>(),
        )?;
        for evidence in &self.evidence {
            evidence.verify()?;
        }
        Ok(())
    }
}
