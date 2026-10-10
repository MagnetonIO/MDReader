use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AssetReferenceKind {
    Image,
    Attachment,
}
#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceAssetRequest {
    pub origin: super::WorkspaceOriginReceipt,
    pub target: String,
    pub kind: AssetReferenceKind,
}
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssetReadError {
    pub kind: AssetFailure,
}
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AssetFailure {
    Invalid,
    Unauthorized,
    Stale,
    Unsupported,
    TooLarge,
    Unavailable,
    Busy,
}
impl AssetFailure {
    pub(super) fn error(self) -> AssetReadError {
        AssetReadError { kind: self }
    }
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Unauthorized => "unauthorized",
            Self::Stale => "stale",
            Self::Unsupported => "unsupported",
            Self::TooLarge => "too_large",
            Self::Unavailable => "unavailable",
            Self::Busy => "busy",
        }
    }
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssetPreview {
    pub path: String,
    pub revision: String,
    pub fragment: Option<String>,
    pub content: AssetContent,
}
#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetContent {
    Text {
        mime: String,
        format: String,
        text: String,
    },
    Raster {
        mime: String,
        width: u32,
        height: u32,
        base64: String,
    },
}
