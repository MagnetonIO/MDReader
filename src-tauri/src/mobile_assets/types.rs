use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssetRequest {
    Inventory,
    Forget {
        asset_id: String,
    },
    Manifest {
        document_id: String,
        revision: String,
    },
    Begin {
        document_id: String,
        revision: String,
        target: String,
        bytes: u32,
        expected_manifest: String,
    },
    Write {
        intent_id: String,
        offset: u32,
        base64: String,
    },
    Commit {
        intent_id: String,
    },
    Discard {
        intent_id: String,
    },
    Remove {
        document_id: String,
        revision: String,
        asset_id: String,
        expected_manifest: String,
    },
    Open {
        document_id: String,
        revision: String,
        asset_id: String,
        manifest: String,
    },
    Chunk {
        session_id: String,
        offset: u32,
    },
    Close {
        session_id: String,
    },
}
#[derive(Clone, Debug, Serialize, specta::Type)]
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
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct AssetPending {
    pub intent_id: String,
    pub target: String,
    pub bytes: u32,
    pub received: u32,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct AssetManifest {
    pub document_id: String,
    pub document_revision: String,
    pub manifest: String,
    pub assets: Vec<AssetReceipt>,
    pub pending: Vec<AssetPending>,
    pub targets: Vec<String>,
    pub targets_truncated: bool,
}
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct AssetInventoryItem {
    pub asset_id: String,
    pub document_id: String,
    pub document_revision: String,
    pub target: String,
    pub bytes: u32,
    pub received: u32,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct AssetInventory {
    pub items: Vec<AssetInventoryItem>,
    pub reserved_bytes: u32,
    pub max_bytes: u32,
    pub max_items: u32,
}
#[derive(Serialize, specta::Type)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AssetReply {
    Inventory {
        value: AssetInventory,
    },
    Manifest {
        value: AssetManifest,
    },
    Intent {
        intent_id: String,
        received: u32,
    },
    Published {
        receipt: AssetReceipt,
        manifest: String,
    },
    Opened {
        session_id: String,
        receipt: AssetReceipt,
    },
    Chunk {
        offset: u32,
        base64: String,
        complete: bool,
    },
    Complete,
}
