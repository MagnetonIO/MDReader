use crate::{device::assets, SourceRoot};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Account/library scope digest plus non-reusable live activation and selection fences.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomingOrigin {
    pub scope_digest: [u8; 32],
    pub item_id: uuid::Uuid,
    pub wire_revision: uuid::Uuid,
    pub session_epoch: uuid::Uuid,
    pub selection_epoch: uuid::Uuid,
}
#[derive(Debug, thiserror::Error)]
pub enum PublicationError {
    #[error(transparent)]
    Device(#[from] crate::device::Error),
    #[error(transparent)]
    Asset(#[from] assets::Error),
    #[error("Incoming selection, revision or activation changed")]
    Conflict,
    #[error("Incoming document or image selection is invalid")]
    InvalidInput,
    #[error("Incoming document publication is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("Incoming intent history reached its supported bound")]
    IntentLimit,
    #[error("Incoming copy is missing, incomplete or changed")]
    Incomplete,
}
impl From<rusqlite::Error> for PublicationError {
    fn from(_: rusqlite::Error) -> Self {
        crate::device::Error::Storage.into()
    }
}
impl From<std::io::Error> for PublicationError {
    fn from(_: std::io::Error) -> Self {
        crate::device::Error::Filesystem.into()
    }
}
impl From<crate::SourceRootError> for PublicationError {
    fn from(_: crate::SourceRootError) -> Self {
        crate::device::Error::AuthorityChanged.into()
    }
}
impl From<serde_json::Error> for PublicationError {
    fn from(_: serde_json::Error) -> Self {
        crate::device::Error::Storage.into()
    }
}
pub type PublicationResult<T> = std::result::Result<T, PublicationError>;
pub struct IncomingAsset {
    pub(super) target: String,
    pub(super) mime: String,
    pub(super) bytes: Vec<u8>,
}
pub struct IncomingDocument {
    pub(super) name: String,
    pub(super) markdown: Vec<u8>,
    pub(super) assets: Vec<IncomingAsset>,
    pub(super) revision: String,
    pub(super) digest: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::device) struct AssetRecord {
    pub id: String,
    pub target: String,
    pub bytes: u32,
    pub hash: String,
    pub mime: String,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Intent {
    pub id: String,
    pub origin: String,
    pub name: String,
    pub revision: String,
    pub bytes: u32,
    pub digest: String,
    pub parent_identity: String,
    pub root_identity: Option<String>,
    pub assets_identity: String,
    pub assets: Vec<AssetRecord>,
    pub state: String,
}
pub struct WritePlan {
    pub(super) intent: Intent,
    pub(super) parent: SourceRoot,
    pub(super) root: SourceRoot,
    pub(super) assets: assets::AssetStore,
    pub(super) owner: Arc<()>,
}
pub struct PreparedIncomingCopy {
    pub(super) plan: WritePlan,
    pub(super) document: IncomingDocument,
    pub(super) evidence: Vec<super::files::FileEvidence>,
}
pub struct ReadyIncoming {
    pub(super) plan: WritePlan,
    pub(super) _permit: assets::ValidationPermit,
    pub(super) rasters: Vec<assets::IncomingRaster>,
    pub(super) evidence: Vec<super::files::FileEvidence>,
}
/// Provisional until the caller's entire transaction commits.
pub struct PublicationReceipt {
    pub document: crate::device::DeviceDocument,
    pub assets: Vec<assets::AssetReceipt>,
}
impl WritePlan {
    pub fn document_id(&self) -> &str {
        &self.intent.id
    }
}
impl ReadyIncoming {
    pub fn document_id(&self) -> &str {
        &self.plan.intent.id
    }
    pub fn revision(&self) -> &str {
        &self.plan.intent.revision
    }
}
