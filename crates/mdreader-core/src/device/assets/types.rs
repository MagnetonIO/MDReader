use super::*;

#[derive(Clone, Debug, Serialize)]
pub struct AssetReceipt {
    pub asset_id: String,
    pub document_id: String,
    pub document_revision: String,
    pub target: String,
    pub content_hash: String,
    pub mime: String,
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct AssetPending {
    pub intent_id: String,
    pub target: String,
    pub bytes: u32,
    pub received: u32,
    pub state: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct AssetManifest {
    pub document_id: String,
    pub document_revision: String,
    pub manifest: String,
    pub assets: Vec<AssetReceipt>,
    pub pending: Vec<AssetPending>,
    pub targets: Vec<String>,
    pub targets_truncated: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct AssetInventoryItem {
    pub asset_id: String,
    pub document_id: String,
    pub document_revision: String,
    pub target: String,
    pub bytes: u32,
    pub received: u32,
    pub state: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct AssetInventory {
    pub items: Vec<AssetInventoryItem>,
    pub reserved_bytes: u32,
    pub max_bytes: u32,
    pub max_items: u32,
}

/// One bounded chunk from a verified, revision-gated image session.
#[derive(Serialize)]
pub struct AssetChunk {
    pub offset: u32,
    pub base64: String,
    pub complete: bool,
}
