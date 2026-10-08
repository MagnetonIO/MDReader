use mdreader_core::device::assets as core;
use serde::{Deserialize, Serialize};

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

impl From<core::AssetReceipt> for AssetReceipt {
    fn from(value: core::AssetReceipt) -> Self {
        Self {
            asset_id: value.asset_id,
            document_id: value.document_id,
            document_revision: value.document_revision,
            target: value.target,
            content_hash: value.content_hash,
            mime: value.mime,
            bytes: value.bytes,
            width: value.width,
            height: value.height,
        }
    }
}

impl From<core::AssetPending> for AssetPending {
    fn from(value: core::AssetPending) -> Self {
        Self {
            intent_id: value.intent_id,
            target: value.target,
            bytes: value.bytes,
            received: value.received,
            state: value.state,
        }
    }
}

impl From<core::AssetManifest> for AssetManifest {
    fn from(value: core::AssetManifest) -> Self {
        Self {
            document_id: value.document_id,
            document_revision: value.document_revision,
            manifest: value.manifest,
            assets: value.assets.into_iter().map(Into::into).collect(),
            pending: value.pending.into_iter().map(Into::into).collect(),
            targets: value.targets,
            targets_truncated: value.targets_truncated,
        }
    }
}

impl From<core::AssetInventoryItem> for AssetInventoryItem {
    fn from(value: core::AssetInventoryItem) -> Self {
        Self {
            asset_id: value.asset_id,
            document_id: value.document_id,
            document_revision: value.document_revision,
            target: value.target,
            bytes: value.bytes,
            received: value.received,
            state: value.state,
        }
    }
}

impl From<core::AssetInventory> for AssetInventory {
    fn from(value: core::AssetInventory) -> Self {
        Self {
            items: value.items.into_iter().map(Into::into).collect(),
            reserved_bytes: value.reserved_bytes,
            max_bytes: value.max_bytes,
            max_items: value.max_items,
        }
    }
}

impl From<core::AssetChunk> for AssetReply {
    fn from(value: core::AssetChunk) -> Self {
        Self::Chunk {
            offset: value.offset,
            base64: value.base64,
            complete: value.complete,
        }
    }
}
