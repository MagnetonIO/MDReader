use mdreader_core::device::{publication::PublicationError, DeviceDocument};
use mdreader_sync::authority::AuthorityError;
use serde::{Deserialize, Serialize};

/// Fixed diagnostics: never include plaintext, SQL, credentials or private paths.
#[derive(Debug)]
pub enum Error {
    Busy,
    Inactive,
    Scope,
    Conflict,
    Storage,
    Schema,
    Limit,
    Publication(PublicationError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("Incoming publication is busy"),
            Self::Inactive => f.write_str("Incoming activation or selection changed"),
            Self::Scope => f.write_str("Incoming account or library does not match"),
            Self::Conflict => f.write_str("Incoming staged revision or receipt changed"),
            Self::Storage => f.write_str("Incoming publication storage unavailable"),
            Self::Schema => f.write_str("Incoming publication schema unsupported"),
            Self::Limit => f.write_str("Incoming publication history limit reached"),
            Self::Publication(e) => std::fmt::Display::fmt(e, f),
        }
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
impl From<mdreader_sync::SyncError> for Error {
    fn from(error: mdreader_sync::SyncError) -> Self {
        match error {
            mdreader_sync::SyncError::Schema => Self::Schema,
            mdreader_sync::SyncError::Scope => Self::Scope,
            mdreader_sync::SyncError::Reused | mdreader_sync::SyncError::NotReady => Self::Conflict,
            mdreader_sync::SyncError::Bounds => Self::Limit,
            _ => Self::Storage,
        }
    }
}
impl From<PublicationError> for Error {
    fn from(e: PublicationError) -> Self {
        Self::Publication(e)
    }
}
impl From<AuthorityError> for Error {
    fn from(_: AuthorityError) -> Self {
        Self::Inactive
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Storage
    }
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingReason {
    ReadyToApply,
    NotContiguous,
    Unstaged,
    TombstoneUnresolved,
    ReauthorizationRequired,
    HeadConflict,
    BaselineUnknown,
    LocalStateChanged,
    TargetUnavailable,
    PositionUnsupported,
    PositionStale,
    Tombstoned,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetIdentity {
    pub target: String,
    pub mime: String,
    pub sha256: String,
    pub bytes: u32,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    #[default]
    Content,
    UserState,
    DocumentDeleted,
    UserStateDeleted,
}
impl Operation {
    pub(super) fn is_content(&self) -> bool {
        *self == Self::Content
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub sequence: u64,
    pub wire_item_id: String,
    pub wire_revision: String,
    /// Native revision is the raw UTF-8 BLAKE3 hash, never the wire UUID.
    pub document: DeviceDocument,
    pub markdown_sha256: String,
    pub assets: Vec<AssetIdentity>,
    pub materialized_assets: Vec<MaterializedAsset>,
    #[serde(default, skip_serializing_if = "Operation::is_content")]
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retirement: Option<mdreader_core::device::state::RetirementRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterializedAsset {
    pub asset_id: String,
    pub identity: AssetIdentity,
    pub width: u32,
    pub height: u32,
}
/// Applied is durable publication, not proof of indexing or present-day source access.
pub enum Outcome {
    Applied {
        receipt: Box<Receipt>,
        replayed: bool,
        index_queued: bool,
    },
    Pending(PendingReason),
}
