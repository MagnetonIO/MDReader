//! Explicit, bounded export reads of retained originals, without granting access.
use super::{current::source_root, files::FileEvidence, *};
use crate::{
    device::{
        self,
        assets::{ArchivedAssetPlan, AssetRuntime, AssetStore, CurrentAssets, ReadPermit},
        state::DocumentState,
    },
    ReaderStore,
};
use std::{path::Path, sync::Arc};

pub struct ArchivePlan {
    state: DocumentState,
    root: crate::SourceRoot,
    assets: Vec<ArchivedAssetPlan>,
    asset_store: AssetStore,
    asset_state: CurrentAssets,
    owner: Arc<()>,
    permit: ReadPermit,
}
pub struct ArchivedAsset {
    pub asset_id: String,
    pub target: String,
    pub expected_bytes: u32,
    pub received: u32,
    pub state: String,
    pub expected_sha256: Option<String>,
    pub mime: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    bytes: Option<Vec<u8>>,
}
impl ArchivedAsset {
    pub fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }
}
/// Not Debug/Serialize/Clone; caller explicitly chooses where to export originals.
pub struct ArchivedDocument {
    pub(super) state: DocumentState,
    markdown: Vec<u8>,
    assets: Vec<ArchivedAsset>,
    pub(super) asset_store: AssetStore,
    pub(super) asset_state: CurrentAssets,
    pub(super) owner: Arc<()>,
    pub(super) evidence: Vec<FileEvidence>,
    _permit: ReadPermit,
}
impl ArchivedDocument {
    pub fn state(&self) -> &DocumentState {
        &self.state
    }
    pub fn markdown(&self) -> &[u8] {
        &self.markdown
    }
    pub fn assets(&self) -> &[ArchivedAsset] {
        &self.assets
    }
}
/// Explicit document selection includes retired/disabled owners. Never infer a
/// source grant, enumerate a directory or reset retirement to make export work.
pub fn prepare_archive(
    store: &mut ReaderStore,
    id: &str,
    assets: &AssetStore,
    runtime: &AssetRuntime,
) -> PublicationResult<ArchivePlan> {
    super::reservation::require_platform()?;
    device::schema(store)?;
    assets.incoming_owner(store)?;
    let permit = runtime.archive_permit(store, assets)?;
    let (state, root, records) = store.with_library_connection(|db| -> PublicationResult<_> {
        let tx = db.transaction()?;
        let state = device::state::load(&tx, id, false)?;
        let root = source_root(&tx, &state.document.source_id)?;
        let records = assets.current_assets_in(&tx, id, &state.document.revision)?;
        tx.commit()?;
        Ok((state, root, records))
    })?;
    Ok(ArchivePlan {
        state,
        root,
        assets: assets.archived_asset_plans(&records),
        asset_store: assets.clone(),
        asset_state: records,
        owner: store.device_owner_session(),
        permit,
    })
}
impl ArchivePlan {
    /// Body/resource reads and hashes occur outside owner locks. Incomplete tracked
    /// assets retain metadata and exact available prefixes; missing copies stay None.
    pub fn read(self) -> PublicationResult<ArchivedDocument> {
        let (body_evidence, markdown) = FileEvidence::read(
            &self.root,
            Path::new("document.md").into(),
            self.state.document.bytes,
        )?;
        if blake3::hash(&markdown).to_hex().as_str() != self.state.document.revision
            || std::str::from_utf8(&markdown).is_err()
        {
            return Err(PublicationError::Conflict);
        }
        let mut assets = Vec::new();
        let mut evidence = vec![body_evidence];
        for asset in self.assets {
            let path = std::path::PathBuf::from(format!("{}.bin", asset.id));
            let present = asset.root.open_optional_file_relative(&path)?;
            let bytes = if let Some(file) = present {
                let size = file.metadata()?.len();
                if size > u64::from(asset.expected_bytes) || size < u64::from(asset.received) {
                    return Err(PublicationError::Conflict);
                }
                let (proof, bytes) = FileEvidence::read(&asset.root, path, size as u32)?;
                if size == u64::from(asset.expected_bytes)
                    && asset.received == asset.expected_bytes
                    && asset.width.is_some()
                    && asset.height.is_some()
                    && asset.hash.as_deref() != Some(mdreader_asset_digest(&bytes).as_str())
                {
                    return Err(PublicationError::Conflict);
                }
                evidence.push(proof);
                Some(bytes)
            } else {
                None
            };
            assets.push(ArchivedAsset {
                asset_id: asset.id,
                target: asset.target,
                expected_bytes: asset.expected_bytes,
                received: asset.received,
                state: asset.state,
                expected_sha256: asset.hash,
                mime: asset.mime,
                width: asset.width,
                height: asset.height,
                bytes,
            });
        }
        for proof in &evidence {
            proof.verify()?;
        }
        Ok(ArchivedDocument {
            state: self.state,
            markdown,
            assets,
            asset_store: self.asset_store,
            asset_state: self.asset_state,
            owner: self.owner,
            evidence,
            _permit: self.permit,
        })
    }
}
fn mdreader_asset_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
