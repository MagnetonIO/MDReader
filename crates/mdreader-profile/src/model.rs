use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootBinding {
    pub locator: crate::RootLocator,
    pub device: u64,
    pub inode: u64,
    /// Full NTFS identity; absent in existing Unix receipts, never truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_file_id: Option<[u8; 16]>,
}
impl RootBinding {
    /// Capture actual host authority; this never grants a source or repairs ACLs.
    #[cfg(any(unix, windows))]
    pub fn capture(path: &Path) -> Result<Self> {
        Ok(crate::platform::Root::capture(path)?.binding)
    }
    /// Capture a protected current-user root without repairing inherited ACLs.
    #[cfg(any(unix, windows))]
    pub fn capture_private(path: &Path) -> Result<Self> {
        Ok(crate::platform::Root::private(path)?.binding)
    }
    pub fn validate(&self) -> Result<()> {
        let path = self.locator.restore().map_err(|_| Error::Invalid)?;
        if !path.is_absolute()
            || (self.inode == 0 && self.windows_file_id.is_none())
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(Error::Invalid);
        }
        #[cfg(unix)]
        if self.windows_file_id.is_some() {
            return Err(Error::Invalid);
        }
        #[cfg(windows)]
        {
            let id = self.windows_file_id.ok_or(Error::Invalid)?;
            let mut low = [0; 8];
            low.copy_from_slice(&id[..8]);
            if id == [0; 16] || self.inode != u64::from_le_bytes(low) {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
}
/// Required registered owners. Tokens/booleans from a caller are not evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Participant {
    RendererSession,
    RendererSettings,
    UnsavedEditor,
    WorkspaceAndReviews,
    LegacyCloudAndEnterprise,
    ReaderDaemon,
    CaptureProducers,
    IntegrationAndAdmissionCustody,
}
pub const REQUIRED_PARTICIPANTS: [Participant; 8] = [
    Participant::RendererSession,
    Participant::RendererSettings,
    Participant::UnsavedEditor,
    Participant::WorkspaceAndReviews,
    Participant::LegacyCloudAndEnterprise,
    Participant::ReaderDaemon,
    Participant::CaptureProducers,
    Participant::IntegrationAndAdmissionCustody,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exclusion {
    RebuildableIndex,
    ModelCache,
    Diagnostics,
    RuntimeEndpointOrLock,
    SqliteRecoverySidecar,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reason", rename_all = "snake_case")]
pub enum Disposition {
    Raw,
    Sqlite,
    Excluded(Exclusion),
    Unclassified,
}
#[derive(Clone)]
pub struct Limits {
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_manifest_bytes: usize,
    pub max_backup_steps: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 8192,
            max_depth: 24,
            max_file_bytes: 1024 * 1024 * 1024,
            max_total_bytes: 4 * 1024 * 1024 * 1024,
            max_manifest_bytes: 8 * 1024 * 1024,
            max_backup_steps: 32768,
        }
    }
}
impl Limits {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.max_entries == 0
            || self.max_entries > 65536
            || self.max_depth == 0
            || self.max_depth > 64
            || self.max_file_bytes == 0
            || self.max_file_bytes > 16 * 1024 * 1024 * 1024
            || self.max_total_bytes < self.max_file_bytes
            || self.max_total_bytes > 64 * 1024 * 1024 * 1024
            || self.max_manifest_bytes == 0
            || self.max_manifest_bytes > 32 * 1024 * 1024
            || self.max_backup_steps == 0
            || self.max_backup_steps > 1_000_000
        {
            return Err(Error::Bounds);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotIdentity {
    pub id: Uuid,
    pub manifest_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    pub id: Uuid,
    pub original: RootBinding,
    pub inventory_sha256: String,
    pub entries: Vec<ManifestEntry>,
    pub directories: Vec<ManifestDirectory>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestEntry {
    pub relative: crate::RelativeLocator,
    pub source_stamp: (u64, u64, u64, i64, i64, i64, i64),
    pub disposition: Disposition,
    pub member: Option<String>,
    pub bytes: u64,
    pub sha256: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestDirectory {
    pub relative: crate::RelativeLocator,
    pub source_stamp: (u64, u64, u64, i64, i64, i64, i64),
}
pub(crate) fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(bytes))
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String> {
    Ok(hash(
        &serde_json::to_vec(value).map_err(|_| Error::Invalid)?,
    ))
}
pub(crate) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
