use mdreader_core::device::{assets, catalog::Receipt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceError {
    Invalid,
    Stale,
    Unsupported,
    Unavailable,
    Persistence,
    Busy,
    Unconfigured,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceRequest {
    State {
        receipt: Receipt,
    },
    SetState {
        receipt: Receipt,
        expected_generation: u64,
        pinned: Option<bool>,
        line: Option<u32>,
    },
    Read {
        receipt: Receipt,
    },
    Manifest {
        receipt: Receipt,
    },
    Open {
        receipt: Receipt,
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
impl DeviceRequest {
    pub(super) fn validate(&self) -> Result<(), DeviceError> {
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let id = |value: &str| uuid::Uuid::parse_str(value).is_ok();
        let receipt = match self {
            Self::Read { receipt } | Self::Manifest { receipt } | Self::State { receipt } => {
                Some(receipt)
            }
            Self::SetState {
                receipt,
                expected_generation,
                pinned,
                line,
            } => {
                if *expected_generation > 9_007_199_254_740_991
                    || (pinned.is_none() && line.is_none())
                    || line.is_some_and(|n| n == 0 || n > 1_000_000)
                {
                    return Err(DeviceError::Invalid);
                }
                Some(receipt)
            }
            Self::Open {
                receipt,
                asset_id,
                manifest,
            } => {
                if !id(asset_id) || !hash(manifest) {
                    return Err(DeviceError::Invalid);
                }
                Some(receipt)
            }
            Self::Chunk { session_id, offset } => {
                if !id(session_id) || *offset > 4 * 1024 * 1024 || !offset.is_multiple_of(65536) {
                    return Err(DeviceError::Invalid);
                }
                None
            }
            Self::Close { session_id } => {
                if !id(session_id) {
                    return Err(DeviceError::Invalid);
                }
                None
            }
        };
        if receipt.is_some_and(|receipt| {
            !id(&receipt.document_id) || !hash(&receipt.source_id) || !hash(&receipt.revision)
        }) {
            return Err(DeviceError::Invalid);
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceReply {
    State {
        receipt: Receipt,
        pinned: bool,
        line: u32,
        line_end: u32,
        generation: u64,
    },
    ReadDocument {
        receipt: Receipt,
        name: String,
        markdown: String,
        line: u32,
    },
    Manifest {
        value: Option<assets::AssetManifest>,
    },
    Opened {
        session_id: String,
        receipt: assets::AssetReceipt,
    },
    Chunk {
        offset: u32,
        base64: String,
        complete: bool,
    },
    Complete,
}
pub(super) fn document_error(error: mdreader_core::device::Error) -> DeviceError {
    use mdreader_core::device::Error as E;
    match error {
        E::Conflict => DeviceError::Stale,
        E::UnsupportedSchema => DeviceError::Unsupported,
        E::InvalidSelection => DeviceError::Invalid,
        E::InvalidSchema | E::Storage => DeviceError::Persistence,
        _ => DeviceError::Unavailable,
    }
}
pub(super) fn asset_error(error: assets::Error) -> DeviceError {
    match error {
        assets::Error::UnsupportedSchema | assets::Error::UnsupportedDocumentSchema => {
            DeviceError::Unsupported
        }
        assets::Error::ReadsBusy | assets::Error::ValidationBusy => DeviceError::Busy,
        assets::Error::Metadata => DeviceError::Persistence,
        _ => DeviceError::Unavailable,
    }
}
