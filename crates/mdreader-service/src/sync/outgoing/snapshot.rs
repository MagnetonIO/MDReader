//! Exact selected native bytes. No path discovery or private/source grant changes.
use super::super::incoming::{AssetIdentity, MaterializedAsset};
use super::*;
use mdreader_core::device::publication::{ArchivedDocument, VerifiedCurrent};
use mdreader_sync::outgoing::OutgoingAsset;

pub(in crate::sync) struct Snapshot {
    pub archive: ArchivedDocument,
    pub materialized: Vec<MaterializedAsset>,
    pub markdown_sha256: String,
    pub assets_sha256: String,
}
impl Snapshot {
    pub fn new(archive: ArchivedDocument, current: &VerifiedCurrent) -> Result<Self> {
        if archive.state() != current.state() || archive.assets().len() > 8 {
            return Err(Error::Conflict);
        }
        let mut materialized = Vec::new();
        for asset in archive.assets() {
            let data = asset.bytes().ok_or(Error::Conflict)?;
            let hash = asset.expected_sha256.as_ref().ok_or(Error::Conflict)?;
            let mime = asset.mime.as_ref().ok_or(Error::Conflict)?;
            let width = asset.width.ok_or(Error::Conflict)?;
            let height = asset.height.ok_or(Error::Conflict)?;
            if asset.state != "ready"
                || asset.received != asset.expected_bytes
                || data.len() != asset.expected_bytes as usize
                || mdreader_sync::blob_digest(data) != *hash
                || !["image/png", "image/jpeg"].contains(&mime.as_str())
                || width == 0
                || height == 0
                || width > 4096
                || height > 4096
                || u64::from(width) * u64::from(height) > 4_000_000
            {
                return Err(Error::Conflict);
            }
            materialized.push(MaterializedAsset {
                asset_id: asset.asset_id.clone(),
                identity: AssetIdentity {
                    target: asset.target.clone(),
                    mime: mime.clone(),
                    sha256: hash.clone(),
                    bytes: asset.expected_bytes,
                },
                width,
                height,
            });
        }
        let markdown_sha256 = mdreader_sync::blob_digest(archive.markdown());
        let assets_sha256 = mdreader_sync::blob_digest(&serde_json::to_vec(&materialized)?);
        Ok(Self {
            archive,
            materialized,
            markdown_sha256,
            assets_sha256,
        })
    }
    pub fn asset_inputs(&self) -> Result<Vec<OutgoingAsset<'_>>> {
        self.archive
            .assets()
            .iter()
            .map(|a| {
                Ok(OutgoingAsset {
                    path: &a.target,
                    mime: a.mime.as_deref().ok_or(Error::Conflict)?,
                    bytes: a.bytes().ok_or(Error::Conflict)?,
                })
            })
            .collect()
    }
    /// Compare every supplied decrypted content field before publication. This
    /// runs off the owner lock; wire scope/revision/receipt authority is separate.
    pub fn matches_prepared(&self, input: &mdreader_sync::incoming::PreparedIncoming) -> bool {
        let mdreader_sync::incoming::IncomingPayload::Content(content) = input.payload() else {
            return false;
        };
        self.archive.state().document.name == content.title()
            && self.archive.markdown() == content.markdown()
            && self.markdown_sha256 == content.sha256()
            && self.archive.assets().len() == content.assets().len()
            && self
                .archive
                .assets()
                .iter()
                .zip(content.assets())
                .all(|(local, remote)| {
                    local.target == remote.path()
                        && local.mime.as_deref() == Some(remote.mime())
                        && local.expected_sha256.as_deref() == Some(remote.sha256())
                        && local.bytes() == Some(remote.bytes())
                })
    }
    pub fn matches(&self, intent: &Intent) -> bool {
        self.archive.state().document.id == intent.native_document
            && self.archive.state().document.revision == intent.native_revision
            && self.archive.state().generation == intent.generation
            && self.markdown_sha256 == intent.markdown_sha256
            && self.assets_sha256 == intent.assets_sha256
    }
}
