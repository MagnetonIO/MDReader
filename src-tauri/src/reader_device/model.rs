use mdreader_service::device as service;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceReceipt {
    pub document_id: String,
    pub source_id: String,
    pub revision: String,
}
impl From<DeviceReceipt> for mdreader_core::device::catalog::Receipt {
    fn from(value: DeviceReceipt) -> Self {
        Self {
            document_id: value.document_id,
            source_id: value.source_id,
            revision: value.revision,
        }
    }
}
#[derive(Deserialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DeviceRequest {
    Read {
        receipt: DeviceReceipt,
    },
    Manifest {
        receipt: DeviceReceipt,
    },
    Open {
        receipt: DeviceReceipt,
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
impl From<DeviceRequest> for service::DeviceRequest {
    fn from(value: DeviceRequest) -> Self {
        match value {
            DeviceRequest::Read { receipt } => Self::Read {
                receipt: receipt.into(),
            },
            DeviceRequest::Manifest { receipt } => Self::Manifest {
                receipt: receipt.into(),
            },
            DeviceRequest::Open {
                receipt,
                asset_id,
                manifest,
            } => Self::Open {
                receipt: receipt.into(),
                asset_id,
                manifest,
            },
            DeviceRequest::Chunk { session_id, offset } => Self::Chunk { session_id, offset },
            DeviceRequest::Close { session_id } => Self::Close { session_id },
        }
    }
}
#[derive(Debug, Serialize, Type)]
pub struct DeviceError {
    pub kind: String,
}
pub(super) fn unavailable() -> DeviceError {
    DeviceError {
        kind: "unavailable".into(),
    }
}
impl From<service::DeviceError> for DeviceError {
    fn from(value: service::DeviceError) -> Self {
        use service::DeviceError as E;
        Self {
            kind: match value {
                E::Invalid => "invalid",
                E::Stale => "stale",
                E::Unsupported => "unsupported",
                E::Unavailable => "unavailable",
                E::Persistence => "persistence",
                E::Busy => "busy",
                E::Unconfigured => "unconfigured",
            }
            .into(),
        }
    }
}
#[derive(Serialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum DeviceReply {
    ReadDocument {
        document_id: String,
        source_id: String,
        path: String,
        revision: String,
        markdown: String,
        line_start: u32,
        line_end: u32,
    },
    Manifest {
        value: Option<crate::mobile_assets::AssetManifest>,
    },
    Opened {
        session_id: String,
        receipt: crate::mobile_assets::AssetReceipt,
    },
    Chunk {
        offset: u32,
        base64: String,
        complete: bool,
    },
    Complete,
}
impl DeviceReply {
    pub(super) fn project(value: service::DeviceReply) -> Result<Self, DeviceError> {
        Ok(match value {
            service::DeviceReply::ReadDocument {
                receipt, markdown, ..
            } => {
                let line_end =
                    u32::try_from(markdown.lines().count().max(1)).map_err(|_| unavailable())?;
                Self::ReadDocument {
                    document_id: receipt.document_id,
                    source_id: receipt.source_id,
                    revision: receipt.revision,
                    path: "document.md".into(),
                    markdown,
                    line_start: 1,
                    line_end,
                }
            }
            service::DeviceReply::Manifest { value } => Self::Manifest {
                value: value.map(Into::into),
            },
            service::DeviceReply::Opened {
                session_id,
                receipt,
            } => Self::Opened {
                session_id,
                receipt: receipt.into(),
            },
            service::DeviceReply::Chunk {
                offset,
                base64,
                complete,
            } => Self::Chunk {
                offset,
                base64,
                complete,
            },
            service::DeviceReply::Complete => Self::Complete,
        })
    }
}
