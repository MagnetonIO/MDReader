//! Fixed private-safe persistence transport. No caller-selected paths.
use serde::{Deserialize, Serialize};

pub(super) const MAX_JSON_BYTES: usize = 1024 * 1024;
pub(super) const MAX_NODES: usize = 32768;

/// Host implementation choice, not a writer-quiescence or migration proof.
#[derive(Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistenceHost {
    NativeAtomic,
    LegacyHost,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JsonKind {
    Session,
    Settings,
}
impl JsonKind {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Settings => "settings",
        }
    }
    pub(super) fn file(self) -> &'static str {
        match self {
            Self::Session => "session.json",
            Self::Settings => "settings.json",
        }
    }
    pub(super) fn index(self) -> usize {
        match self {
            Self::Session => 0,
            Self::Settings => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistenceError {
    Invalid,
    UnsupportedSchema,
    TooLarge,
    Conflict,
    Frozen,
    Unavailable,
    Storage,
    PublicationUncertain,
}
pub(super) type Result<T> = std::result::Result<T, PersistenceError>;

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RendererIdentity {
    pub owner_epoch: String,
    pub renderer_epoch: String,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WriteReceipt {
    pub kind: JsonKind,
    pub generation: u32,
    pub owner_epoch: String,
    pub renderer_epoch: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PersistedJson {
    pub identity: RendererIdentity,
    /// Complete plugin-compatible wrappers. Unknown fields remain raw on disk;
    /// the renderer uses only recognized state and never rewrites this string.
    pub session_json: Option<String>,
    pub settings_json: Option<String>,
}

/// Exact bytes are archival evidence, never a settings/session restoration DTO.
/// No Debug/Serialize implementation: private data is only explicitly exported.
pub(crate) struct PreflushJson {
    pub kind: JsonKind,
    pub bytes: Option<Vec<u8>>,
}
