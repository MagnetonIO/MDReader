//! Backend-owned Library membership.
//!
//! The active editing engine remains deliberately single-root. Query execution
//! takes one immutable snapshot of existing local folder membership. Retired
//! connection rows are compatibility metadata, never runtime grants. Paths are
//! canonicalized before admission and
//! persisted as native OS units so a lossy webview display string is never the
//! authority.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use tauri_plugin_dialog::DialogExt;

#[cfg(test)]
use crate::cloud_accounts::CloudSourceError;
use crate::cloud_accounts::{validate_cloud_source_records, CloudSourceRecord};
use crate::file_lock::AdvisoryFileLock;
use mdreader_core::{SourceRoot, SourceRootIdentity};

mod legacy_connections;
use legacy_connections::{
    validate_connected_catdb_source, validate_connected_catdb_source_v7,
    ConnectedCatdbSourceRecord, ConnectedCatdbSourceRecordV7,
};

pub(crate) const LIBRARY_VERSION: u32 = 9;
const MAX_LIBRARY_SOURCES: usize = 128;

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum LibraryError {
    InvalidRoot,
    DuplicateRoot,
    SourceNotFound,
    SourceUnavailable,
    InvalidDocumentPath,
    ActiveSource,
    GrantMismatch,
    OpenFailed,
    CorruptRegistry,
    UnsupportedVersion { found: u32, expected: u32 },
    PersistFailed,
    CleanupFailed { reason: LibraryCleanupReason },
    NeedsNativeRepick,
    OfflineMigrationRequired,
}

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LibraryCleanupReason {
    ManagedStateUnavailable,
    RollbackFailed,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum LibraryCleanupStatus {
    Complete,
    Pending { source_count: u32 },
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot => formatter.write_str("invalid-library-root"),
            Self::DuplicateRoot => formatter.write_str("duplicate-library-root"),
            Self::SourceNotFound => formatter.write_str("library-source-not-found"),
            Self::SourceUnavailable => formatter.write_str("library-source-unavailable"),
            Self::InvalidDocumentPath => formatter.write_str("invalid-library-document-path"),
            Self::ActiveSource => formatter.write_str("active-library-source"),
            Self::GrantMismatch => formatter.write_str("library-grant-mismatch"),
            Self::OpenFailed => formatter.write_str("library-open-failed"),
            Self::CorruptRegistry => formatter.write_str("library-registry-corrupt"),
            Self::UnsupportedVersion { found, expected } => write!(
                formatter,
                "unsupported library registry version {found}; expected {expected}"
            ),
            Self::PersistFailed => formatter.write_str("library-persist-failed"),
            Self::CleanupFailed { reason } => write!(
                formatter,
                "library source cleanup failed: {}",
                match reason {
                    LibraryCleanupReason::ManagedStateUnavailable => {
                        "managed-state-unavailable"
                    }
                    LibraryCleanupReason::RollbackFailed => "rollback-failed",
                }
            ),
            Self::OfflineMigrationRequired => {
                formatter.write_str("offline-legacy-migration-required")
            }
            Self::NeedsNativeRepick => formatter.write_str("needs-native-repick"),
        }
    }
}

impl std::error::Error for LibraryError {}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LibraryIndexState {
    Present,
    NotCreated,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum LibraryAvailability {
    Available,
    Missing,
    Unreadable,
    NotDirectory,
}

impl From<LibrarySourceState> for LibraryAvailability {
    fn from(state: LibrarySourceState) -> Self {
        match state {
            LibrarySourceState::Available => Self::Available,
            LibrarySourceState::Missing => Self::Missing,
            LibrarySourceState::Unreadable => Self::Unreadable,
            LibrarySourceState::NotDirectory => Self::NotDirectory,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct LibrarySourceDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub state: LibraryAvailability,
    pub index_state: LibraryIndexState,
    pub active: bool,
    pub restore_candidate: bool,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct LibraryCloudSourceDto {
    pub id: String,
    pub provider: crate::cloud_accounts::CloudProvider,
    pub name: String,
    pub enabled: bool,
    pub contract: LibrarySourceContract,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct LibrarySnapshot {
    pub version: u32,
    pub sources: Vec<LibrarySourceDto>,
    pub cloud_sources: Vec<LibraryCloudSourceDto>,
    pub cleanup: LibraryCleanupStatus,
}

/// Native close-and-forget state after the active workspace authority is gone.
#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloseAndForgetStatus {
    /// Membership and private managed state were removed.
    Complete,
    /// Membership is gone; deletion of quarantined private state will retry.
    CleanupPending,
    /// The workspace is closed, but membership remains available for retry.
    Inactive { error: LibraryError },
}

#[derive(Debug)]
struct CloseAndForgetStoreOutcome {
    status: CloseAndForgetStatus,
    workspace_generation: u32,
}

/// Result returned after the native active-source close transaction has crossed
/// its authority-revocation boundary.
#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct CloseAndForgetLibrarySourceOutcome {
    pub status: CloseAndForgetStatus,
    pub workspace_generation: u32,
}

/// Immutable source membership captured under the Library registry lease.
#[derive(Debug, Clone)]
pub(crate) struct LibraryQuerySnapshot {
    pub local_sources: Vec<LocalFolderSource>,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct PickedLibrarySource {
    pub source: Option<LibrarySourceDto>,
    pub added: bool,
}

#[derive(Default)]
pub(crate) struct LibraryState(pub(crate) Mutex<()>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibrarySourceState {
    Available,
    Missing,
    Unreadable,
    NotDirectory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum NativeEncoding {
    UnixBytes,
    WindowsWide,
    Utf8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NativePath {
    encoding: NativeEncoding,
    units: Vec<u32>,
}

impl NativePath {
    fn capture(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self {
                encoding: NativeEncoding::UnixBytes,
                units: path
                    .as_os_str()
                    .as_bytes()
                    .iter()
                    .map(|unit| u32::from(*unit))
                    .collect(),
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self {
                encoding: NativeEncoding::WindowsWide,
                units: path.as_os_str().encode_wide().map(u32::from).collect(),
            }
        }

        #[cfg(not(any(unix, windows)))]
        Self {
            encoding: NativeEncoding::Utf8,
            units: path
                .to_string_lossy()
                .as_bytes()
                .iter()
                .map(|unit| u32::from(*unit))
                .collect(),
        }
    }

    fn restore(&self) -> Result<PathBuf, LibraryError> {
        if self.units.is_empty() || self.units.contains(&0) {
            return Err(LibraryError::CorruptRegistry);
        }

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            if self.encoding != NativeEncoding::UnixBytes {
                return Err(LibraryError::CorruptRegistry);
            }
            let bytes = self
                .units
                .iter()
                .copied()
                .map(u8::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| LibraryError::CorruptRegistry)?;
            Ok(PathBuf::from(OsString::from_vec(bytes)))
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            if self.encoding != NativeEncoding::WindowsWide {
                return Err(LibraryError::CorruptRegistry);
            }
            let wide = self
                .units
                .iter()
                .copied()
                .map(u16::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| LibraryError::CorruptRegistry)?;
            Ok(PathBuf::from(OsString::from_wide(&wide)))
        }

        #[cfg(not(any(unix, windows)))]
        {
            if self.encoding != NativeEncoding::Utf8 {
                return Err(LibraryError::CorruptRegistry);
            }
            let bytes = self
                .units
                .iter()
                .copied()
                .map(u8::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| LibraryError::CorruptRegistry)?;
            let path = String::from_utf8(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
            Ok(PathBuf::from(path))
        }
    }

    fn source_id(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        match self.encoding {
            NativeEncoding::UnixBytes => hasher.update(b"unix-bytes\0"),
            NativeEncoding::WindowsWide => hasher.update(b"windows-wide\0"),
            NativeEncoding::Utf8 => hasher.update(b"utf8\0"),
        };
        for unit in &self.units {
            hasher.update(&unit.to_le_bytes());
        }
        hasher.finalize().to_hex().to_string()
    }
}

fn new_source_id() -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-library-source-id-v1\0");
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    hasher.finalize().to_hex().to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LibraryIndexPolicy {
    Managed,
    ReadThrough,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LibrarySourceCapabilities {
    pub read: bool,
    pub write: bool,
    pub search: bool,
    pub watch: bool,
    pub open_original: bool,
    pub download: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LibrarySourceContract {
    pub connector_version: String,
    pub capabilities: LibrarySourceCapabilities,
    pub mapping_version: String,
    pub index_policy: LibraryIndexPolicy,
}

impl LibrarySourceContract {
    fn local_folder() -> Self {
        Self {
            connector_version: "local-folder/v1".to_string(),
            capabilities: LibrarySourceCapabilities {
                read: true,
                write: true,
                search: true,
                watch: true,
                open_original: true,
                download: false,
            },
            mapping_version: "mdreader.Document.local-folder/v2".to_string(),
            index_policy: LibraryIndexPolicy::Managed,
        }
    }

    pub(crate) fn cloud(provider: crate::cloud_accounts::CloudProvider) -> Self {
        let connector_version = match provider {
            crate::cloud_accounts::CloudProvider::GoogleDrive => "google-drive/v1",
            crate::cloud_accounts::CloudProvider::Dropbox => "dropbox/v1",
        };
        Self {
            connector_version: connector_version.to_string(),
            capabilities: LibrarySourceCapabilities {
                read: true,
                write: false,
                search: true,
                watch: false,
                open_original: true,
                download: true,
            },
            mapping_version: crate::cloud_accounts::CLOUD_DOCUMENT_MAPPING_VERSION.to_string(),
            index_policy: LibraryIndexPolicy::ReadThrough,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalFolderSource {
    pub(crate) id: String,
    path: NativePath,
    #[serde(deserialize_with = "deserialize_identity_binding")]
    identity: Option<SourceRootIdentity>,
    engine_source_id: uuid::Uuid,
    #[serde(default)]
    contract: Option<LibrarySourceContract>,
}

impl LocalFolderSource {
    fn refresh_contract(&mut self) {
        self.contract = Some(LibrarySourceContract::local_folder());
    }
}

fn deserialize_identity_binding<'de, D>(
    deserializer: D,
) -> Result<Option<SourceRootIdentity>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<SourceRootIdentity>::deserialize(deserializer)
}

impl LocalFolderSource {
    /// Stable reader source identity owned and persisted by this Library
    /// membership, never derived from its current filesystem address.
    pub(crate) fn engine_source_id(&self) -> uuid::Uuid {
        self.engine_source_id
    }

    pub(crate) fn path(&self) -> Result<PathBuf, LibraryError> {
        self.path.restore()
    }

    pub(crate) fn root(&self) -> Result<SourceRoot, LibraryError> {
        let identity = self
            .identity
            .clone()
            .ok_or(LibraryError::SourceUnavailable)?;
        SourceRoot::restore(self.path()?, identity).map_err(|_| LibraryError::SourceUnavailable)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum LibrarySource {
    LocalFolder(LocalFolderSource),
    GoogleDrive(CloudSourceRecord),
    Dropbox(CloudSourceRecord),
    ConnectedCatdb(ConnectedCatdbSourceRecord),
}

impl LibrarySource {
    fn id(&self) -> &str {
        match self {
            Self::LocalFolder(source) => &source.id,
            Self::GoogleDrive(source) | Self::Dropbox(source) => source.id(),
            Self::ConnectedCatdb(source) => &source.id,
        }
    }

    fn local(&self) -> Option<&LocalFolderSource> {
        match self {
            Self::LocalFolder(source) => Some(source),
            Self::GoogleDrive(_) | Self::Dropbox(_) | Self::ConnectedCatdb(_) => None,
        }
    }

    fn local_mut(&mut self) -> Option<&mut LocalFolderSource> {
        match self {
            Self::LocalFolder(source) => Some(source),
            Self::GoogleDrive(_) | Self::Dropbox(_) | Self::ConnectedCatdb(_) => None,
        }
    }

    fn cloud(&self) -> Option<&CloudSourceRecord> {
        match self {
            Self::GoogleDrive(source) | Self::Dropbox(source) => Some(source),
            Self::LocalFolder(_) | Self::ConnectedCatdb(_) => None,
        }
    }

    fn connected_catdb(&self) -> Option<&ConnectedCatdbSourceRecord> {
        match self {
            Self::ConnectedCatdb(source) => Some(source),
            Self::LocalFolder(_) | Self::GoogleDrive(_) | Self::Dropbox(_) => None,
        }
    }

    fn from_cloud(source: CloudSourceRecord) -> Self {
        match source.provider() {
            crate::cloud_accounts::CloudProvider::GoogleDrive => Self::GoogleDrive(source),
            crate::cloud_accounts::CloudProvider::Dropbox => Self::Dropbox(source),
        }
    }

    fn refresh_contract(&mut self) {
        match self {
            Self::LocalFolder(source) => source.refresh_contract(),
            Self::GoogleDrive(source) | Self::Dropbox(source) => source.refresh_contract(),
            Self::ConnectedCatdb(source) => source.refresh_contract(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibrary {
    version: u32,
    active_source_id: Option<String>,
    #[serde(default)]
    restore_suppressed_source_ids: Vec<String>,
    sources: Vec<LibrarySource>,
    #[serde(default)]
    connected_catdb_rediscovery_required: bool,
    #[serde(default)]
    cleanup_pending: Vec<PendingCleanup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibraryV8 {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LibrarySource>,
    #[serde(default)]
    connected_catdb_rediscovery_required: bool,
    #[serde(default)]
    cleanup_pending: Vec<PendingCleanup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibraryV7 {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LibrarySourceV7>,
    #[serde(default)]
    cleanup_pending: Vec<PendingCleanup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum LibrarySourceV7 {
    LocalFolder(LocalFolderSource),
    GoogleDrive(CloudSourceRecord),
    Dropbox(CloudSourceRecord),
    ConnectedCatdb(ConnectedCatdbSourceRecordV7),
}

impl LibrarySourceV7 {
    fn id(&self) -> &str {
        match self {
            Self::LocalFolder(source) => &source.id,
            Self::GoogleDrive(source) | Self::Dropbox(source) => source.id(),
            Self::ConnectedCatdb(source) => &source.id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibraryV4 {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LocalFolderSource>,
    cloud_sources: Vec<CloudSourceRecord>,
    #[serde(default)]
    cleanup_pending: Vec<PendingCleanup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibraryV3 {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LocalFolderSource>,
    #[serde(default)]
    cleanup_pending: Vec<PendingCleanup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibraryV1 {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LibrarySourceV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibrarySourceV1 {
    id: String,
    path: NativePath,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibraryV2 {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LibrarySourceV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibrarySourceV2 {
    id: String,
    path: NativePath,
    #[serde(deserialize_with = "deserialize_identity_binding")]
    identity: Option<SourceRootIdentity>,
}

#[cfg(test)]
pub(crate) fn write_v2_registry_for_test(
    path: &Path,
    root: &SourceRoot,
) -> Result<(), Box<dyn std::error::Error>> {
    let native = NativePath::capture(&root.path);
    let source_id = native.source_id();
    let stored = StoredLibraryV2 {
        version: 2,
        active_source_id: Some(source_id.clone()),
        sources: vec![LibrarySourceV2 {
            id: source_id,
            path: native,
            identity: Some(root.identity.clone()),
        }],
    };
    let parent = path.parent().ok_or("registry has no parent")?;
    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_vec_pretty(&stored)?)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PendingCleanup {
    source_id: String,
    tombstone: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LibraryRegistry {
    active_source_id: Option<String>,
    restore_suppressed_source_ids: Vec<String>,
    sources: Vec<LibrarySource>,
    connected_catdb_rediscovery_required: bool,
    cleanup_pending: Vec<PendingCleanup>,
}

impl LibraryRegistry {
    pub(crate) fn empty() -> Self {
        Self {
            active_source_id: None,
            restore_suppressed_source_ids: Vec::new(),
            sources: Vec::new(),
            connected_catdb_rediscovery_required: false,
            cleanup_pending: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn sources(&self) -> Vec<&LocalFolderSource> {
        self.sources
            .iter()
            .filter_map(LibrarySource::local)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn cloud_source_count(&self) -> usize {
        self.sources.iter().filter_map(LibrarySource::cloud).count()
    }

    pub(crate) fn path_for(&self, source_id: &str) -> Option<PathBuf> {
        self.source_for(source_id)
            .and_then(|source| source.path().ok())
    }

    fn source_for(&self, source_id: &str) -> Option<&LocalFolderSource> {
        self.sources
            .iter()
            .filter_map(LibrarySource::local)
            .find(|source| source.id == source_id)
    }

    fn local_sources(&self) -> impl Iterator<Item = &LocalFolderSource> {
        self.sources.iter().filter_map(LibrarySource::local)
    }

    fn local_sources_mut(&mut self) -> impl Iterator<Item = &mut LocalFolderSource> {
        self.sources.iter_mut().filter_map(LibrarySource::local_mut)
    }

    #[cfg(test)]
    pub(crate) fn restore_source_id(&self) -> Option<&str> {
        self.active_source_id.as_deref()
    }

    pub(crate) fn add_root(&mut self, requested: &Path) -> Result<LocalFolderSource, LibraryError> {
        let canonical = fs::canonicalize(requested).map_err(|_| LibraryError::InvalidRoot)?;
        if !canonical.is_dir() {
            return Err(LibraryError::InvalidRoot);
        }
        let admitted =
            SourceRoot::capture(canonical.clone()).map_err(|_| LibraryError::InvalidRoot)?;
        let native = NativePath::capture(&canonical);
        if let Some(source) = self
            .local_sources_mut()
            .find(|source| source.identity.as_ref() == Some(&admitted.identity))
        {
            if source.path != native {
                source.path = native;
                return Ok(source.clone());
            }
            return Err(LibraryError::DuplicateRoot);
        }
        if self.sources.len() >= MAX_LIBRARY_SOURCES {
            return Err(LibraryError::InvalidRoot);
        }
        let mut source_id = new_source_id();
        while self.sources.iter().any(|source| source.id() == source_id) {
            source_id = new_source_id();
        }
        let source = LocalFolderSource {
            id: source_id,
            path: native,
            identity: Some(admitted.identity),
            engine_source_id: new_engine_source_id(
                &self.local_sources().cloned().collect::<Vec<_>>(),
            ),
            contract: Some(LibrarySourceContract::local_folder()),
        };
        self.sources
            .push(LibrarySource::LocalFolder(source.clone()));
        Ok(source)
    }

    pub(crate) fn source_state(&self, source_id: &str) -> Result<LibrarySourceState, LibraryError> {
        let source = self
            .source_for(source_id)
            .ok_or(LibraryError::SourceNotFound)?;
        let path = source.path()?;
        let canonical = match fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(error) => return Ok(classify_probe_error(error)),
        };
        if canonical != path {
            return Ok(LibrarySourceState::Unreadable);
        }
        let root = match source.root() {
            Ok(root) => root,
            Err(LibraryError::SourceUnavailable) => return Ok(LibrarySourceState::Unreadable),
            Err(error) => return Err(error),
        };
        if root.verify().is_err() {
            return Ok(LibrarySourceState::Unreadable);
        }
        match fs::metadata(&canonical) {
            Ok(metadata) if !metadata.is_dir() => Ok(LibrarySourceState::NotDirectory),
            Ok(_) => match fs::read_dir(&canonical) {
                Ok(_) => Ok(LibrarySourceState::Available),
                Err(error) => Ok(classify_probe_error(error)),
            },
            Err(error) => Ok(classify_probe_error(error)),
        }
    }

    /// Resolve an available member to its exact native path for a grant/watch
    /// command. This is the only activation lookup: DTO display text is never
    /// converted back into filesystem authority.
    pub(crate) fn activation_root(&self, source_id: &str) -> Result<SourceRoot, LibraryError> {
        let source = self
            .source_for(source_id)
            .ok_or(LibraryError::SourceNotFound)?;
        match self.source_state(source_id)? {
            LibrarySourceState::Available => source.root(),
            LibrarySourceState::Missing
            | LibrarySourceState::Unreadable
            | LibrarySourceState::NotDirectory => Err(LibraryError::SourceUnavailable),
        }
    }

    fn resolve_document_edit_path(
        &self,
        source_id: &str,
        relative_path: &Path,
        active_root: Option<&SourceRoot>,
    ) -> Result<PathBuf, LibraryError> {
        if relative_path.is_absolute() {
            return Err(LibraryError::InvalidDocumentPath);
        }
        let source_root = self.activation_root(source_id)?;
        if active_root != Some(&source_root) {
            return Err(LibraryError::GrantMismatch);
        }
        let candidate = source_root.path.join(relative_path);
        let relative = source_root
            .relative_path(&candidate)
            .map_err(|_| LibraryError::InvalidDocumentPath)?;
        if relative.as_os_str().is_empty() {
            return Err(LibraryError::InvalidDocumentPath);
        }
        source_root
            .verify()
            .map_err(|_| LibraryError::SourceUnavailable)?;
        Ok(source_root.path.join(relative))
    }

    #[cfg(test)]
    pub(crate) fn activation_path(&self, source_id: &str) -> Result<PathBuf, LibraryError> {
        self.activation_root(source_id).map(|root| root.path)
    }

    pub(crate) fn forget(
        &mut self,
        source_id: &str,
        active_root: Option<&SourceRoot>,
    ) -> Result<(), LibraryError> {
        let index = self
            .sources
            .iter()
            .position(|source| source.local().is_some_and(|source| source.id == source_id))
            .ok_or(LibraryError::SourceNotFound)?;
        let source = self.sources[index]
            .local()
            .ok_or(LibraryError::SourceNotFound)?;
        let source_path = source.path()?;
        if active_root.is_some_and(|root| {
            source.identity.as_ref() == Some(&root.identity) && source_path == root.path
        }) {
            return Err(LibraryError::ActiveSource);
        }
        self.sources.remove(index);
        self.restore_suppressed_source_ids
            .retain(|suppressed| suppressed != source_id);
        if self.active_source_id.as_deref() == Some(source_id) {
            self.active_source_id = None;
        }
        Ok(())
    }

    pub(crate) fn mark_active(
        &mut self,
        source_id: &str,
        active_root: Option<&SourceRoot>,
    ) -> Result<(), LibraryError> {
        let source_root = self.activation_root(source_id)?;
        if active_root != Some(&source_root) {
            return Err(LibraryError::GrantMismatch);
        }
        source_root
            .verify()
            .map_err(|_| LibraryError::SourceUnavailable)?;
        self.active_source_id = Some(source_id.to_string());
        self.restore_suppressed_source_ids
            .retain(|suppressed| suppressed != source_id);
        Ok(())
    }

    pub(crate) fn snapshot(
        &self,
        app_data: &Path,
        active_root: Option<&SourceRoot>,
    ) -> Result<LibrarySnapshot, LibraryError> {
        let mut sources = Vec::with_capacity(self.sources.len());
        for source in self.local_sources() {
            let path = source.path()?;
            let bound_root = source.root().ok();
            let name = path
                .file_name()
                .filter(|name| !name.is_empty())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Local folder".to_string());
            sources.push(LibrarySourceDto {
                id: source.id.clone(),
                name,
                path: path.to_string_lossy().into_owned(),
                state: self.source_state(&source.id)?.into(),
                index_state: source.identity.as_ref().map_or(
                    LibraryIndexState::Unknown,
                    |identity| {
                        let db =
                            mdreader_core::source_root::workspace_index_path_for_stable_identity(
                                app_data, identity,
                            );
                        if db.is_file() {
                            LibraryIndexState::Present
                        } else {
                            LibraryIndexState::NotCreated
                        }
                    },
                ),
                active: bound_root
                    .as_ref()
                    .is_some_and(|root| active_root == Some(root)),
                restore_candidate: self.active_source_id.as_deref() == Some(source.id.as_str()),
            });
        }
        let cloud_sources = self
            .sources
            .iter()
            .filter_map(LibrarySource::cloud)
            .map(|source| {
                Ok(LibraryCloudSourceDto {
                    id: source.id().to_string(),
                    provider: source.provider(),
                    name: source.label().to_string(),
                    enabled: source.enabled(),
                    contract: source
                        .contract()
                        .cloned()
                        .ok_or(LibraryError::CorruptRegistry)?,
                })
            })
            .collect::<Result<Vec<_>, LibraryError>>()?;
        Ok(LibrarySnapshot {
            version: LIBRARY_VERSION,
            sources,
            cloud_sources,
            cleanup: self.cleanup_status(),
        })
    }

    fn cleanup_status(&self) -> LibraryCleanupStatus {
        if self.cleanup_pending.is_empty() {
            LibraryCleanupStatus::Complete
        } else {
            LibraryCleanupStatus::Pending {
                source_count: self.cleanup_pending.len().try_into().unwrap_or(u32::MAX),
            }
        }
    }

    pub(crate) fn reconcile_restore(
        &self,
        requested_root: Option<&Path>,
    ) -> Result<Option<String>, LibraryError> {
        let Some(requested_root) = requested_root else {
            return Ok(None);
        };

        // The session store can only carry a webview string. For a non-UTF-8
        // native root that string is necessarily lossy, so first reconcile it
        // against the backend-owned restore candidate that was recorded only
        // after a successful grant. The string is identity evidence here, never
        // path authority: activation below still resolves the source ID to the
        // registry's lossless native units.
        if let Some(active_id) = self.active_source_id.as_deref() {
            let active_path = self
                .path_for(active_id)
                .ok_or(LibraryError::CorruptRegistry)?;
            if active_path.to_string_lossy() == requested_root.to_string_lossy()
                && matches!(self.source_state(active_id)?, LibrarySourceState::Available)
            {
                return Ok(Some(active_id.to_string()));
            }
        }

        let Ok(canonical) = fs::canonicalize(requested_root) else {
            return Ok(None);
        };
        for source in self.local_sources() {
            if source.path()? != canonical {
                continue;
            }
            if self
                .restore_suppressed_source_ids
                .iter()
                .any(|suppressed| suppressed == &source.id)
            {
                return Ok(None);
            }
            return Ok(matches!(
                self.source_state(&source.id)?,
                LibrarySourceState::Available
            )
            .then(|| source.id.clone()));
        }
        Ok(None)
    }

    pub(crate) fn load_from(path: &Path) -> Result<Self, LibraryError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(_) => return Err(LibraryError::CorruptRegistry),
        };
        #[derive(Deserialize)]
        struct VersionProbe {
            version: u32,
        }
        let version: VersionProbe =
            serde_json::from_slice(&bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        if version.version == 1 {
            return Self::migrate_v1(path, &bytes);
        }
        if version.version == 2 {
            return Self::migrate_v2(path, &bytes);
        }
        if version.version == 3 {
            return Self::migrate_v3(path, &bytes);
        }
        if version.version == 4 {
            return Self::migrate_v4(path, &bytes);
        }
        if version.version == 5 {
            return Self::migrate_v5(path, &bytes);
        }
        if version.version == 6 {
            return Self::migrate_v6(path, &bytes);
        }
        if version.version == 7 {
            return Self::migrate_v7(path, &bytes);
        }
        if version.version == 8 {
            return Self::migrate_v8(path, &bytes);
        }
        if version.version != LIBRARY_VERSION {
            return Err(LibraryError::UnsupportedVersion {
                found: version.version,
                expected: LIBRARY_VERSION,
            });
        }
        let stored: StoredLibrary =
            serde_json::from_slice(&bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        Self::validate_membership(stored.active_source_id.as_deref(), &stored.sources)?;
        Self::validate_restore_suppression(
            stored.active_source_id.as_deref(),
            &stored.restore_suppressed_source_ids,
            &stored.sources,
        )?;
        Self::validate_pending_cleanup(&stored.cleanup_pending, &stored.sources)?;
        Ok(Self {
            active_source_id: stored.active_source_id,
            restore_suppressed_source_ids: stored.restore_suppressed_source_ids,
            sources: stored.sources,
            connected_catdb_rediscovery_required: stored.connected_catdb_rediscovery_required,
            cleanup_pending: stored.cleanup_pending,
        })
    }

    fn migrate_v5(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let stored: StoredLibrary =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        let mut migrated = Self {
            active_source_id: stored.active_source_id,
            restore_suppressed_source_ids: stored.restore_suppressed_source_ids,
            sources: stored.sources,
            connected_catdb_rediscovery_required: false,
            cleanup_pending: stored.cleanup_pending,
        };
        for source in &mut migrated.sources {
            source.refresh_contract();
        }
        Self::validate_membership(migrated.active_source_id.as_deref(), &migrated.sources)?;
        Self::validate_pending_cleanup(&migrated.cleanup_pending, &migrated.sources)?;
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn migrate_v6(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let stored: StoredLibrary =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        let mut migrated = Self {
            active_source_id: stored.active_source_id,
            restore_suppressed_source_ids: Vec::new(),
            sources: stored.sources,
            connected_catdb_rediscovery_required: false,
            cleanup_pending: stored.cleanup_pending,
        };
        for source in &mut migrated.sources {
            source.refresh_contract();
        }
        Self::validate_membership(migrated.active_source_id.as_deref(), &migrated.sources)?;
        Self::validate_pending_cleanup(&migrated.cleanup_pending, &migrated.sources)?;
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn migrate_v7(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let stored: StoredLibraryV7 =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        if stored.sources.len() > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry);
        }
        let mut ids = HashSet::new();
        if stored.sources.iter().any(|source| !ids.insert(source.id())) {
            return Err(LibraryError::CorruptRegistry);
        }

        let mut rediscovery_required = false;
        let mut connected_count = 0usize;
        let mut quarantined_connected_id = None;
        let mut sources = Vec::with_capacity(stored.sources.len());
        for source in stored.sources {
            match source {
                LibrarySourceV7::LocalFolder(source) => {
                    sources.push(LibrarySource::LocalFolder(source));
                }
                LibrarySourceV7::GoogleDrive(source) => {
                    sources.push(LibrarySource::GoogleDrive(source));
                }
                LibrarySourceV7::Dropbox(source) => {
                    sources.push(LibrarySource::Dropbox(source));
                }
                LibrarySourceV7::ConnectedCatdb(source) => {
                    connected_count += 1;
                    validate_connected_catdb_source_v7(&source)?;
                    quarantined_connected_id = Some(source.id);
                    rediscovery_required = true;
                }
            }
        }
        if connected_count > 1 {
            return Err(LibraryError::CorruptRegistry);
        }

        let migrated = Self {
            active_source_id: stored
                .active_source_id
                .filter(|active| quarantined_connected_id.as_deref() != Some(active.as_str())),
            restore_suppressed_source_ids: Vec::new(),
            sources,
            connected_catdb_rediscovery_required: rediscovery_required,
            cleanup_pending: stored.cleanup_pending,
        };
        Self::validate_membership(migrated.active_source_id.as_deref(), &migrated.sources)?;
        Self::validate_pending_cleanup(&migrated.cleanup_pending, &migrated.sources)?;
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn migrate_v8(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let stored: StoredLibraryV8 =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        Self::validate_membership(stored.active_source_id.as_deref(), &stored.sources)?;
        Self::validate_pending_cleanup(&stored.cleanup_pending, &stored.sources)?;
        let migrated = Self {
            active_source_id: stored.active_source_id,
            restore_suppressed_source_ids: Vec::new(),
            sources: stored.sources,
            connected_catdb_rediscovery_required: stored.connected_catdb_rediscovery_required,
            cleanup_pending: stored.cleanup_pending,
        };
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn migrate_v1(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let stored: StoredLibraryV1 =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        Self::validate_legacy_membership(
            stored.active_source_id.as_deref(),
            stored
                .sources
                .iter()
                .map(|source| (&source.id, &source.path)),
            stored.sources.len(),
        )?;

        quarantine_v1_workspaces(path, &stored.sources)?;
        let migrated = Self {
            active_source_id: None,
            restore_suppressed_source_ids: Vec::new(),
            // V1 stores only historical paths. They are recovery hints, not
            // live membership; the native picker must explicitly re-admit the
            // filesystem object. Legacy managed bytes were quarantined above
            // and are never inherited by the object currently at that path.
            sources: Vec::new(),
            connected_catdb_rediscovery_required: false,
            cleanup_pending: Vec::new(),
        };
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn migrate_v2(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        Self::migrate_v2_with_persist(path, bytes, Self::save_to)
    }

    fn migrate_v2_with_persist(
        path: &Path,
        bytes: &[u8],
        persist: impl FnOnce(&Self, &Path) -> Result<(), LibraryError>,
    ) -> Result<Self, LibraryError> {
        let stored: StoredLibraryV2 =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        Self::validate_legacy_membership(
            stored.active_source_id.as_deref(),
            stored
                .sources
                .iter()
                .map(|source| (&source.id, &source.path)),
            stored.sources.len(),
        )?;
        if stored
            .sources
            .iter()
            .any(|source| source.identity.is_none())
        {
            return Err(LibraryError::CorruptRegistry);
        }

        let app_data = path.parent().ok_or(LibraryError::PersistFailed)?;
        let mut id_map = HashMap::with_capacity(stored.sources.len());
        let mut engine_source_ids = HashSet::with_capacity(stored.sources.len());
        let sources = stored
            .sources
            .iter()
            .map(|source| {
                let id = new_source_id();
                id_map.insert(source.id.clone(), id.clone());
                let engine_source_id = recover_v2_engine_source_id(app_data, source)?
                    .unwrap_or_else(uuid::Uuid::new_v4);
                if engine_source_id.is_nil() || !engine_source_ids.insert(engine_source_id) {
                    return Err(LibraryError::CorruptRegistry);
                }
                Ok(LibrarySource::LocalFolder(LocalFolderSource {
                    id,
                    path: source.path.clone(),
                    identity: source.identity.clone(),
                    engine_source_id,
                    contract: Some(LibrarySourceContract::local_folder()),
                }))
            })
            .collect::<Result<Vec<_>, LibraryError>>()?;
        let migrated = Self {
            active_source_id: stored
                .active_source_id
                .as_ref()
                .and_then(|old| id_map.get(old).cloned()),
            restore_suppressed_source_ids: Vec::new(),
            sources,
            connected_catdb_rediscovery_required: false,
            cleanup_pending: Vec::new(),
        };
        Self::validate_membership(migrated.active_source_id.as_deref(), &migrated.sources)?;

        let moves = migrate_v2_managed_workspaces(app_data, &stored.sources)?;
        if let Err(error) = persist(&migrated, path) {
            if rollback_managed_moves(&moves).is_err() {
                return Err(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::RollbackFailed,
                });
            }
            return Err(error);
        }
        Ok(migrated)
    }

    fn migrate_v3(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let mut stored: StoredLibraryV3 =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        for source in &mut stored.sources {
            source.refresh_contract();
        }
        Self::validate_local_membership(stored.active_source_id.as_deref(), &stored.sources)?;
        Self::validate_pending_local_cleanup(&stored.cleanup_pending, &stored.sources)?;
        let migrated = Self {
            active_source_id: stored.active_source_id,
            restore_suppressed_source_ids: Vec::new(),
            sources: stored
                .sources
                .into_iter()
                .map(LibrarySource::LocalFolder)
                .collect(),
            connected_catdb_rediscovery_required: false,
            cleanup_pending: stored.cleanup_pending,
        };
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn migrate_v4(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let mut stored: StoredLibraryV4 =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        for source in &mut stored.sources {
            source.refresh_contract();
        }
        for source in &mut stored.cloud_sources {
            source.refresh_contract();
        }
        Self::validate_local_membership(stored.active_source_id.as_deref(), &stored.sources)?;
        validate_cloud_source_records(&stored.cloud_sources)
            .map_err(|_| LibraryError::CorruptRegistry)?;
        Self::validate_pending_local_cleanup(&stored.cleanup_pending, &stored.sources)?;
        let mut sources = stored
            .sources
            .into_iter()
            .map(LibrarySource::LocalFolder)
            .collect::<Vec<_>>();
        sources.extend(
            stored
                .cloud_sources
                .into_iter()
                .map(LibrarySource::from_cloud),
        );
        let migrated = Self {
            active_source_id: stored.active_source_id,
            restore_suppressed_source_ids: Vec::new(),
            sources,
            connected_catdb_rediscovery_required: false,
            cleanup_pending: stored.cleanup_pending,
        };
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn validate_legacy_membership<'a>(
        active_source_id: Option<&str>,
        sources: impl Iterator<Item = (&'a String, &'a NativePath)>,
        source_count: usize,
    ) -> Result<(), LibraryError> {
        if source_count > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry);
        }
        let mut ids = HashSet::new();
        for (source_id, native_path) in sources {
            let path = native_path.restore()?;
            if !path.is_absolute()
                || native_path.source_id() != *source_id
                || !ids.insert(source_id.as_str())
            {
                return Err(LibraryError::CorruptRegistry);
            }
        }
        if active_source_id.is_some_and(|active| !ids.contains(active)) {
            return Err(LibraryError::CorruptRegistry);
        }
        Ok(())
    }

    fn validate_local_membership(
        active_source_id: Option<&str>,
        sources: &[LocalFolderSource],
    ) -> Result<(), LibraryError> {
        if sources.len() > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry);
        }
        let mut ids = HashSet::new();
        let mut engine_source_ids = HashSet::new();
        for (index, source) in sources.iter().enumerate() {
            let path = source.path.restore()?;
            let valid_id = source.id.len() == 64
                && source
                    .id
                    .as_bytes()
                    .iter()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
            let identity = source
                .identity
                .as_ref()
                .ok_or(LibraryError::CorruptRegistry)?;
            if !path.is_absolute()
                || !valid_id
                || !ids.insert(source.id.as_str())
                || source.engine_source_id.is_nil()
                || source.contract.as_ref() != Some(&LibrarySourceContract::local_folder())
                || !engine_source_ids.insert(source.engine_source_id)
                || sources[..index]
                    .iter()
                    .any(|prior| prior.identity.as_ref() == Some(identity))
            {
                return Err(LibraryError::CorruptRegistry);
            }
        }
        if active_source_id.is_some_and(|active| !ids.contains(active)) {
            return Err(LibraryError::CorruptRegistry);
        }
        Ok(())
    }

    fn validate_membership(
        active_source_id: Option<&str>,
        sources: &[LibrarySource],
    ) -> Result<(), LibraryError> {
        if sources.len() > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry);
        }
        let local_sources = sources
            .iter()
            .filter_map(LibrarySource::local)
            .cloned()
            .collect::<Vec<_>>();
        Self::validate_local_membership(active_source_id, &local_sources)?;
        let cloud_sources = sources
            .iter()
            .filter_map(LibrarySource::cloud)
            .cloned()
            .collect::<Vec<_>>();
        validate_cloud_source_records(&cloud_sources).map_err(|_| LibraryError::CorruptRegistry)?;
        let connected_sources = sources
            .iter()
            .filter_map(LibrarySource::connected_catdb)
            .collect::<Vec<_>>();
        if connected_sources.len() > 1 {
            return Err(LibraryError::CorruptRegistry);
        }
        for source in connected_sources {
            validate_connected_catdb_source(source)?;
        }
        let mut ids = HashSet::new();
        if sources.iter().any(|source| !ids.insert(source.id())) {
            return Err(LibraryError::CorruptRegistry);
        }
        Ok(())
    }

    fn validate_pending_cleanup(
        pending: &[PendingCleanup],
        sources: &[LibrarySource],
    ) -> Result<(), LibraryError> {
        if pending.len() > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry);
        }
        let mut source_ids = HashSet::new();
        let mut tombstones = HashSet::new();
        for entry in pending {
            let valid_source_id = entry.source_id.len() == 64
                && entry
                    .source_id
                    .as_bytes()
                    .iter()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
            let valid_tombstone = entry.tombstone.len() == 64
                && entry
                    .tombstone
                    .as_bytes()
                    .iter()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
            if !valid_source_id
                || !valid_tombstone
                || !source_ids.insert(entry.source_id.as_str())
                || !tombstones.insert(entry.tombstone.as_str())
                || sources.iter().any(|source| source.id() == entry.source_id)
            {
                return Err(LibraryError::CorruptRegistry);
            }
        }
        Ok(())
    }

    fn validate_restore_suppression(
        active_source_id: Option<&str>,
        suppressed: &[String],
        sources: &[LibrarySource],
    ) -> Result<(), LibraryError> {
        if suppressed.len() > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry);
        }
        let local_ids = sources
            .iter()
            .filter_map(LibrarySource::local)
            .map(|source| source.id.as_str())
            .collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        if suppressed.iter().any(|source_id| {
            !seen.insert(source_id.as_str())
                || !local_ids.contains(source_id.as_str())
                || active_source_id == Some(source_id.as_str())
        }) {
            return Err(LibraryError::CorruptRegistry);
        }
        Ok(())
    }

    fn validate_pending_local_cleanup(
        pending: &[PendingCleanup],
        sources: &[LocalFolderSource],
    ) -> Result<(), LibraryError> {
        let sources = sources
            .iter()
            .cloned()
            .map(LibrarySource::LocalFolder)
            .collect::<Vec<_>>();
        Self::validate_pending_cleanup(pending, &sources)
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), LibraryError> {
        Self::validate_membership(self.active_source_id.as_deref(), &self.sources)?;
        Self::validate_restore_suppression(
            self.active_source_id.as_deref(),
            &self.restore_suppressed_source_ids,
            &self.sources,
        )?;
        Self::validate_pending_cleanup(&self.cleanup_pending, &self.sources)?;
        let parent = path.parent().ok_or(LibraryError::PersistFailed)?;
        fs::create_dir_all(parent).map_err(|_| LibraryError::PersistFailed)?;
        let body = serde_json::to_vec_pretty(&StoredLibrary {
            version: LIBRARY_VERSION,
            active_source_id: self.active_source_id.clone(),
            restore_suppressed_source_ids: self.restore_suppressed_source_ids.clone(),
            sources: self.sources.clone(),
            connected_catdb_rediscovery_required: self.connected_catdb_rediscovery_required,
            cleanup_pending: self.cleanup_pending.clone(),
        })
        .map_err(|_| LibraryError::PersistFailed)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| LibraryError::PersistFailed)?;
        temporary
            .write_all(&body)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| LibraryError::PersistFailed)?;
        temporary
            .persist(path)
            .map(|_| ())
            .map_err(|_| LibraryError::PersistFailed)
    }
}

fn new_engine_source_id(sources: &[LocalFolderSource]) -> uuid::Uuid {
    loop {
        let candidate = uuid::Uuid::new_v4();
        if sources
            .iter()
            .all(|source| source.engine_source_id != candidate)
        {
            return candidate;
        }
    }
}

fn valid_library_id(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn recover_v2_engine_source_id(
    app_data: &Path,
    source: &LibrarySourceV2,
) -> Result<Option<uuid::Uuid>, LibraryError> {
    let identity = source
        .identity
        .as_ref()
        .ok_or(LibraryError::CorruptRegistry)?;
    let source_path = source.path.restore()?;
    let old = mdreader_core::source_root::legacy_workspace_index_path_for_path_identity(
        app_data,
        &source_path,
        identity,
    );
    let current =
        mdreader_core::source_root::workspace_index_path_for_stable_identity(app_data, identity);
    for path in [old, current] {
        if managed_directory_present(path.parent().ok_or(LibraryError::PersistFailed)?)? {
            return Err(LibraryError::OfflineMigrationRequired);
        }
    }
    Ok(None)
}

fn managed_directory_present(path: &Path) -> Result<bool, LibraryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            })
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(LibraryError::CleanupFailed {
            reason: LibraryCleanupReason::ManagedStateUnavailable,
        }),
    }
}

/// Historical V1 path-only workspace location. This is intentionally kept
/// local to migration code: new indexes must always use the identity-bound
/// key in `mdreader-ingest`.
fn v1_workspace_dir(app_data: &Path, root: &Path) -> PathBuf {
    let hash = if let Some(root) = root.to_str() {
        blake3::hash(root.as_bytes())
    } else {
        let mut hasher = blake3::Hasher::new();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            hasher.update(b"mdreader-unix-native-root-v1\0");
            hasher.update(root.as_os_str().as_bytes());
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            hasher.update(b"mdreader-windows-native-root-v1\0");
            for unit in root.as_os_str().encode_wide() {
                hasher.update(&unit.to_le_bytes());
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            hasher.update(b"mdreader-platform-native-root-v1\0");
            hasher.update(root.to_string_lossy().as_bytes());
        }
        hasher.finalize()
    }
    .to_hex()
    .to_string();
    app_data.join("workspaces").join(hash)
}

/// Move every path-keyed V1 index directory, including its sibling
/// `proposals.db`, into a recoverable quarantine before publishing V2. A crash
/// after the rename is idempotent: retry observes the destination and the
/// missing old location. Existing source and destination together are refused
/// so migration never overwrites either copy.
fn quarantine_v1_workspaces(
    registry_path: &Path,
    sources: &[LibrarySourceV1],
) -> Result<(), LibraryError> {
    let app_data = registry_path.parent().ok_or(LibraryError::PersistFailed)?;
    let quarantine_root = app_data.join("legacy-v1-path-workspaces");

    for source in sources {
        let source_path = source.path.restore()?;
        let legacy = v1_workspace_dir(app_data, &source_path);
        let metadata = match fs::symlink_metadata(&legacy) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(LibraryError::PersistFailed),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(LibraryError::CorruptRegistry);
        }

        fs::create_dir_all(&quarantine_root).map_err(|_| LibraryError::PersistFailed)?;
        let quarantine_metadata =
            fs::symlink_metadata(&quarantine_root).map_err(|_| LibraryError::PersistFailed)?;
        if quarantine_metadata.file_type().is_symlink() || !quarantine_metadata.is_dir() {
            return Err(LibraryError::PersistFailed);
        }

        let key = legacy.file_name().ok_or(LibraryError::CorruptRegistry)?;
        let destination = quarantine_root.join(key);
        match fs::symlink_metadata(&destination) {
            Ok(_) => return Err(LibraryError::PersistFailed),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(LibraryError::PersistFailed),
        }
        fs::rename(&legacy, &destination).map_err(|_| LibraryError::PersistFailed)?;
    }
    Ok(())
}

fn migrate_v2_managed_workspaces(
    app_data: &Path,
    sources: &[LibrarySourceV2],
) -> Result<Vec<(PathBuf, PathBuf)>, LibraryError> {
    let mut moves = Vec::new();
    for source in sources {
        let identity = source
            .identity
            .as_ref()
            .ok_or(LibraryError::CorruptRegistry)?;
        let path = source.path.restore()?;
        let old = mdreader_core::source_root::legacy_workspace_index_path_for_path_identity(
            app_data, &path, identity,
        )
        .parent()
        .ok_or(LibraryError::PersistFailed)?
        .to_path_buf();
        let new = mdreader_core::source_root::workspace_index_path_for_stable_identity(
            app_data, identity,
        )
        .parent()
        .ok_or(LibraryError::PersistFailed)?
        .to_path_buf();
        if old == new {
            continue;
        }
        let old_metadata = match fs::symlink_metadata(&old) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                rollback_managed_moves(&moves).ok();
                return Err(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                });
            }
        };
        if old_metadata.file_type().is_symlink() || !old_metadata.is_dir() || new.exists() {
            rollback_managed_moves(&moves).ok();
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }
        if fs::rename(&old, &new).is_err() {
            rollback_managed_moves(&moves).ok();
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }
        moves.push((old, new));
    }
    Ok(moves)
}

fn rollback_managed_moves(moves: &[(PathBuf, PathBuf)]) -> Result<(), ()> {
    for (old, new) in moves.iter().rev() {
        fs::rename(new, old).map_err(|_| ())?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct LibraryStore {
    registry_path: PathBuf,
}

impl LibraryStore {
    pub(crate) fn new(registry_path: PathBuf) -> Self {
        Self { registry_path }
    }

    #[cfg(any(test, feature = "e2e-build"))]
    pub(crate) fn replace_with_e2e_local_membership(
        &self,
        corpus_root: &Path,
        engine_source_id: uuid::Uuid,
    ) -> Result<LocalFolderSource, LibraryError> {
        let _lease = self.acquire()?;
        let mut registry = LibraryRegistry::empty();
        if engine_source_id.is_nil() {
            return Err(LibraryError::InvalidRoot);
        }
        // This path comes from the isolated runner process, but it becomes
        // authority only after the backend canonicalizes it and captures the
        // native filesystem identity through `add_root`.
        let mut source = registry.add_root(corpus_root)?;
        source.engine_source_id = engine_source_id;
        registry
            .sources
            .iter_mut()
            .find_map(LibrarySource::local_mut)
            .ok_or(LibraryError::PersistFailed)?
            .engine_source_id = engine_source_id;
        registry.active_source_id = Some(source.id.clone());
        registry.save_to(&self.registry_path)?;
        Ok(source)
    }

    fn lock_path(&self) -> Result<PathBuf, LibraryError> {
        self.registry_path
            .parent()
            .map(|parent| parent.join("library.lock"))
            .ok_or(LibraryError::PersistFailed)
    }

    fn acquire(&self) -> Result<AdvisoryFileLock, LibraryError> {
        AdvisoryFileLock::acquire(&self.lock_path()?).map_err(|_| LibraryError::PersistFailed)
    }

    fn read<T>(
        &self,
        operation: impl FnOnce(&LibraryRegistry) -> Result<T, LibraryError>,
    ) -> Result<T, LibraryError> {
        let _lease = self.acquire()?;
        let mut registry = LibraryRegistry::load_from(&self.registry_path)?;
        let app_data = self.app_data()?;
        let mut cleanup = |path: &Path| fs::remove_dir_all(path);
        self.retry_pending_cleanup_locked(&mut registry, &app_data, &mut cleanup)?;
        operation(&registry)
    }

    /// Capture existing local membership under one registry lease.
    /// Provider scopes are resolved by the separate MDReader provider owner.
    pub(crate) fn read_query_snapshot(&self) -> Result<LibraryQuerySnapshot, LibraryError> {
        self.read(|registry| {
            Ok(LibraryQuerySnapshot {
                local_sources: registry.local_sources().cloned().collect(),
            })
        })
    }

    fn update<T>(
        &self,
        operation: impl FnOnce(&mut LibraryRegistry) -> Result<T, LibraryError>,
    ) -> Result<T, LibraryError> {
        let _lease = self.acquire()?;
        let mut registry = LibraryRegistry::load_from(&self.registry_path)?;
        let app_data = self.app_data()?;
        let mut cleanup = |path: &Path| fs::remove_dir_all(path);
        self.retry_pending_cleanup_locked(&mut registry, &app_data, &mut cleanup)?;
        let result = operation(&mut registry)?;
        registry.save_to(&self.registry_path)?;
        Ok(result)
    }

    #[cfg(test)]
    pub(crate) fn update_cloud<T>(
        &self,
        operation: impl FnOnce(&mut Vec<CloudSourceRecord>) -> Result<T, CloudSourceError>,
    ) -> Result<T, CloudSourceError> {
        let _lease = self
            .acquire()
            .map_err(|_| CloudSourceError::PersistFailed)?;
        let mut registry =
            LibraryRegistry::load_from(&self.registry_path).map_err(map_library_cloud_error)?;
        let app_data = self.app_data().map_err(map_library_cloud_error)?;
        let mut cleanup = |path: &Path| fs::remove_dir_all(path);
        self.retry_pending_cleanup_locked(&mut registry, &app_data, &mut cleanup)
            .map_err(map_library_cloud_error)?;
        let mut cloud_sources = registry
            .sources
            .iter()
            .filter_map(LibrarySource::cloud)
            .cloned()
            .collect::<Vec<_>>();
        let result = operation(&mut cloud_sources)?;
        validate_cloud_source_records(&cloud_sources)?;
        registry.sources.retain(|source| {
            matches!(
                source,
                LibrarySource::LocalFolder(_) | LibrarySource::ConnectedCatdb(_)
            )
        });
        registry
            .sources
            .extend(cloud_sources.into_iter().map(LibrarySource::from_cloud));
        registry
            .save_to(&self.registry_path)
            .map_err(map_library_cloud_error)?;
        Ok(result)
    }

    fn app_data(&self) -> Result<PathBuf, LibraryError> {
        self.registry_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or(LibraryError::PersistFailed)
    }

    fn forget_source(
        &self,
        source_id: &str,
        active_root: Option<&SourceRoot>,
        app_data: &Path,
    ) -> Result<LibraryCleanupStatus, LibraryError> {
        self.forget_source_with_cleanup_ops(
            source_id,
            active_root,
            app_data,
            |from, to| fs::rename(from, to),
            |path| fs::remove_dir_all(path),
        )
    }

    fn forget_source_with_cleanup_ops(
        &self,
        source_id: &str,
        active_root: Option<&SourceRoot>,
        app_data: &Path,
        rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
        mut cleanup: impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<LibraryCleanupStatus, LibraryError> {
        let _lease = self.acquire()?;
        if self.app_data()?.as_path() != app_data {
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }
        let mut registry = LibraryRegistry::load_from(&self.registry_path)?;
        self.retry_pending_cleanup_locked(&mut registry, app_data, &mut cleanup)?;
        let source = registry
            .source_for(source_id)
            .ok_or(LibraryError::SourceNotFound)?;
        let identity = source
            .identity
            .as_ref()
            .ok_or(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            })?;
        let managed = mdreader_core::source_root::workspace_index_path_for_stable_identity(
            app_data, identity,
        )
        .parent()
        .ok_or(LibraryError::CleanupFailed {
            reason: LibraryCleanupReason::ManagedStateUnavailable,
        })?
        .to_path_buf();
        let mut updated = registry.clone();
        updated.forget(source_id, active_root)?;

        let metadata = match fs::symlink_metadata(&managed) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => {
                return Err(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                });
            }
        };
        let Some(metadata) = metadata else {
            updated.save_to(&self.registry_path)?;
            return Ok(updated.cleanup_status());
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }

        let staging_root = app_data.join("managed-delete-staging");
        fs::create_dir_all(&staging_root).map_err(|_| LibraryError::CleanupFailed {
            reason: LibraryCleanupReason::ManagedStateUnavailable,
        })?;
        let staging_metadata =
            fs::symlink_metadata(&staging_root).map_err(|_| LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            })?;
        if staging_metadata.file_type().is_symlink() || !staging_metadata.is_dir() {
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }
        let staged = staging_root.join(source_id);
        match fs::symlink_metadata(&staged) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => {
                return Err(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                });
            }
        }
        rename(&managed, &staged).map_err(|_| LibraryError::CleanupFailed {
            reason: LibraryCleanupReason::ManagedStateUnavailable,
        })?;
        let tombstone = staged
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            })?
            .to_string();
        updated.cleanup_pending.push(PendingCleanup {
            source_id: source_id.to_string(),
            tombstone,
        });

        if let Err(error) = updated.save_to(&self.registry_path) {
            if fs::rename(&staged, &managed).is_err() {
                return Err(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::RollbackFailed,
                });
            }
            return Err(error);
        }

        if cleanup(&staged).is_err() {
            return Ok(updated.cleanup_status());
        }
        let persisted_pending_count = updated.cleanup_pending.len();
        updated
            .cleanup_pending
            .retain(|entry| entry.source_id != source_id);
        if updated.save_to(&self.registry_path).is_err() {
            return Ok(LibraryCleanupStatus::Pending {
                source_count: u32::try_from(persisted_pending_count).unwrap_or(u32::MAX),
            });
        }
        Ok(updated.cleanup_status())
    }

    /// Preflight the exact active membership, then revoke its native authority
    /// before the first quarantine/delete effect. Once `deactivate` succeeds,
    /// every later failure is returned as a typed inactive outcome rather than
    /// an error that could leave the renderer presenting stale authority.
    #[allow(clippy::too_many_arguments)]
    fn close_and_forget_source_with_cleanup_ops(
        &self,
        source_id: &str,
        active_root: &SourceRoot,
        active_engine_source_id: Option<uuid::Uuid>,
        app_data: &Path,
        deactivate: impl FnOnce() -> Result<u32, LibraryError>,
        rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
        cleanup: impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<CloseAndForgetStoreOutcome, LibraryError> {
        self.close_and_forget_source_with_ops(
            source_id,
            active_root,
            active_engine_source_id,
            app_data,
            deactivate,
            rename,
            cleanup,
            |registry, path| registry.save_to(path),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn close_and_forget_source_with_ops(
        &self,
        source_id: &str,
        active_root: &SourceRoot,
        active_engine_source_id: Option<uuid::Uuid>,
        app_data: &Path,
        deactivate: impl FnOnce() -> Result<u32, LibraryError>,
        rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
        mut cleanup: impl FnMut(&Path) -> io::Result<()>,
        mut persist: impl FnMut(&LibraryRegistry, &Path) -> Result<(), LibraryError>,
    ) -> Result<CloseAndForgetStoreOutcome, LibraryError> {
        let _lease = self.acquire()?;
        if self.app_data()?.as_path() != app_data {
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }
        let registry = LibraryRegistry::load_from(&self.registry_path)?;
        let source = registry
            .source_for(source_id)
            .ok_or(LibraryError::SourceNotFound)?;
        let source_root = source.root()?;
        if &source_root != active_root {
            return Err(LibraryError::GrantMismatch);
        }
        if active_engine_source_id != Some(source.engine_source_id()) {
            return Err(LibraryError::GrantMismatch);
        }
        if registry
            .active_source_id
            .as_deref()
            .is_some_and(|active_source_id| active_source_id != source_id)
        {
            return Err(LibraryError::GrantMismatch);
        }
        let identity = source
            .identity
            .as_ref()
            .ok_or(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            })?;
        let managed = mdreader_core::source_root::workspace_index_path_for_stable_identity(
            app_data, identity,
        )
        .parent()
        .ok_or(LibraryError::CleanupFailed {
            reason: LibraryCleanupReason::ManagedStateUnavailable,
        })?
        .to_path_buf();
        let mut inactive_registry = registry.clone();
        inactive_registry.active_source_id = None;
        if !inactive_registry
            .restore_suppressed_source_ids
            .iter()
            .any(|suppressed| suppressed == source_id)
        {
            inactive_registry
                .restore_suppressed_source_ids
                .push(source_id.to_string());
        }
        let mut updated = inactive_registry.clone();
        // The direct registry API remains strict. This distinct app-owned path
        // has already proven the requested membership is the exact active root,
        // and performs native revocation below before touching managed state.
        updated.forget(source_id, None)?;

        let managed_metadata = match fs::symlink_metadata(&managed) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => {
                return Err(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                });
            }
        };
        if managed_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        {
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }

        let staged = if managed_metadata.is_some() {
            let staging_root = app_data.join("managed-delete-staging");
            fs::create_dir_all(&staging_root).map_err(|_| LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            })?;
            let staging_metadata =
                fs::symlink_metadata(&staging_root).map_err(|_| LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                })?;
            if staging_metadata.file_type().is_symlink() || !staging_metadata.is_dir() {
                return Err(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                });
            }
            let staged = staging_root.join(source_id);
            match fs::symlink_metadata(&staged) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                _ => {
                    return Err(LibraryError::CleanupFailed {
                        reason: LibraryCleanupReason::ManagedStateUnavailable,
                    });
                }
            }
            Some(staged)
        } else {
            None
        };

        // Make the close intent durable before revoking native authority. A
        // cleanup failure or process restart after revocation can therefore
        // never restore this retained membership from a stale session root.
        persist(&inactive_registry, &self.registry_path)?;
        let workspace_generation = match deactivate() {
            Ok(generation) => generation,
            Err(error) => {
                if persist(&registry, &self.registry_path).is_err() {
                    return Err(LibraryError::CleanupFailed {
                        reason: LibraryCleanupReason::RollbackFailed,
                    });
                }
                return Err(error);
            }
        };
        let inactive = |error| CloseAndForgetStoreOutcome {
            status: CloseAndForgetStatus::Inactive { error },
            workspace_generation,
        };

        let Some(staged) = staged else {
            return match persist(&updated, &self.registry_path) {
                Ok(()) => Ok(CloseAndForgetStoreOutcome {
                    status: CloseAndForgetStatus::Complete,
                    workspace_generation,
                }),
                Err(error) => Ok(inactive(error)),
            };
        };

        if rename(&managed, &staged).is_err() {
            return Ok(inactive(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            }));
        }
        updated.cleanup_pending.push(PendingCleanup {
            source_id: source_id.to_string(),
            tombstone: source_id.to_string(),
        });
        if let Err(error) = persist(&updated, &self.registry_path) {
            let retained_error = if fs::rename(&staged, &managed).is_ok() {
                error
            } else {
                LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::RollbackFailed,
                }
            };
            return Ok(inactive(retained_error));
        }

        if cleanup(&staged).is_err() {
            return Ok(CloseAndForgetStoreOutcome {
                status: CloseAndForgetStatus::CleanupPending,
                workspace_generation,
            });
        }
        let mut completed = updated.clone();
        completed
            .cleanup_pending
            .retain(|entry| entry.source_id != source_id);
        if persist(&completed, &self.registry_path).is_err() {
            return Ok(CloseAndForgetStoreOutcome {
                status: CloseAndForgetStatus::CleanupPending,
                workspace_generation,
            });
        }
        Ok(CloseAndForgetStoreOutcome {
            status: CloseAndForgetStatus::Complete,
            workspace_generation,
        })
    }

    #[cfg(test)]
    fn retry_pending_cleanup_with(
        &self,
        app_data: &Path,
        mut cleanup: impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<LibraryCleanupStatus, LibraryError> {
        let _lease = self.acquire()?;
        if self.app_data()?.as_path() != app_data {
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            });
        }
        let mut registry = LibraryRegistry::load_from(&self.registry_path)?;
        self.retry_pending_cleanup_locked(&mut registry, app_data, &mut cleanup)
    }

    fn retry_pending_cleanup_locked(
        &self,
        registry: &mut LibraryRegistry,
        app_data: &Path,
        cleanup: &mut impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<LibraryCleanupStatus, LibraryError> {
        self.recover_interrupted_quarantines_locked(registry, app_data)?;
        let mut retained = Vec::new();
        for pending in &registry.cleanup_pending {
            let tombstone = app_data
                .join("managed-delete-staging")
                .join(&pending.tombstone);
            let completed = match fs::symlink_metadata(&tombstone) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => false,
                Ok(_) => cleanup(&tombstone).is_ok(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                Err(_) => false,
            };
            if !completed {
                retained.push(pending.clone());
            }
        }
        if retained != registry.cleanup_pending {
            registry.cleanup_pending = retained;
            registry.save_to(&self.registry_path)?;
        }
        Ok(registry.cleanup_status())
    }

    fn recover_interrupted_quarantines_locked(
        &self,
        registry: &mut LibraryRegistry,
        app_data: &Path,
    ) -> Result<(), LibraryError> {
        let mut recovered = Vec::new();
        for source in registry.local_sources() {
            let staged = app_data.join("managed-delete-staging").join(&source.id);
            let staged_metadata = match fs::symlink_metadata(&staged) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(LibraryError::CleanupFailed {
                        reason: LibraryCleanupReason::ManagedStateUnavailable,
                    });
                }
            };
            if staged_metadata.file_type().is_symlink() || !staged_metadata.is_dir() {
                continue;
            }
            let identity = source
                .identity
                .as_ref()
                .ok_or(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                })?;
            let managed_database =
                mdreader_core::source_root::workspace_index_path_for_stable_identity(
                    app_data, identity,
                );
            let managed = managed_database
                .parent()
                .ok_or(LibraryError::CleanupFailed {
                    reason: LibraryCleanupReason::ManagedStateUnavailable,
                })?;
            match fs::symlink_metadata(managed) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    recovered.push(source.id.clone());
                }
                Ok(_) => {}
                Err(_) => {
                    return Err(LibraryError::CleanupFailed {
                        reason: LibraryCleanupReason::ManagedStateUnavailable,
                    });
                }
            }
        }
        if recovered.is_empty() {
            return Ok(());
        }
        for source_id in recovered {
            registry.forget(&source_id, None)?;
            registry.cleanup_pending.push(PendingCleanup {
                tombstone: source_id.clone(),
                source_id,
            });
        }
        registry.save_to(&self.registry_path)
    }

    fn reconcile_restore_or_migrate(
        &self,
        requested_root: Option<&Path>,
        app_data: &Path,
    ) -> Result<Option<LibrarySourceDto>, LibraryError> {
        let _lease = self.acquire()?;
        let registry_exists = match fs::symlink_metadata(&self.registry_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(LibraryError::CorruptRegistry);
            }
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(_) => return Err(LibraryError::CorruptRegistry),
        };

        if registry_exists {
            let mut registry = LibraryRegistry::load_from(&self.registry_path)?;
            let mut cleanup = |path: &Path| fs::remove_dir_all(path);
            self.retry_pending_cleanup_locked(&mut registry, app_data, &mut cleanup)?;
            let source_id = registry.reconcile_restore(requested_root)?;
            return source_id
                .map(|source_id| {
                    registry
                        .snapshot(app_data, None)?
                        .sources
                        .into_iter()
                        .find(|source| source.id == source_id)
                        .ok_or(LibraryError::CorruptRegistry)
                })
                .transpose();
        }

        // A renderer session string is never filesystem authority. With no
        // backend-owned registry there is nothing to reconcile, regardless of
        // what SQLite-shaped files happen to exist in app data. Only the native
        // picker may create the first membership.
        Err(LibraryError::NeedsNativeRepick)
    }
}

#[cfg(test)]
pub(crate) fn map_library_cloud_error(error: LibraryError) -> CloudSourceError {
    match error {
        LibraryError::CorruptRegistry => CloudSourceError::CorruptRegistry,
        LibraryError::UnsupportedVersion { found, expected } => {
            CloudSourceError::UnsupportedVersion { found, expected }
        }
        _ => CloudSourceError::PersistFailed,
    }
}

pub(crate) fn registry_path(app: &AppHandle) -> Result<(PathBuf, PathBuf), LibraryError> {
    let app_data =
        crate::reader_profile::directory(app).map_err(|_| LibraryError::PersistFailed)?;
    Ok((app_data.join("library.json"), app_data))
}

#[cfg(all(
    feature = "e2e-build",
    not(any(target_os = "ios", target_os = "android"))
))]
pub(crate) fn seed_e2e_local_membership(app: &AppHandle) -> Result<(), LibraryError> {
    const E2E_IDENTIFIER: &str = "dev.mdreader.app.e2e";
    const E2E_CORPUS_ROOT_ENV: &str = "MDREADER_E2E_CORPUS_ROOT";
    const E2E_SOURCE_ID_ENV: &str = "MDREADER_E2E_SOURCE_ID";

    if app.config().identifier != E2E_IDENTIFIER {
        return Err(LibraryError::PersistFailed);
    }
    let corpus_root = std::env::var_os(E2E_CORPUS_ROOT_ENV).ok_or(LibraryError::InvalidRoot)?;
    let source_id = std::env::var(E2E_SOURCE_ID_ENV)
        .ok()
        .and_then(|value| uuid::Uuid::parse_str(&value).ok())
        .filter(|source_id| !source_id.is_nil())
        .ok_or(LibraryError::InvalidRoot)?;
    let (registry_path, _) = registry_path(app)?;
    LibraryStore::new(registry_path)
        .replace_with_e2e_local_membership(Path::new(&corpus_root), source_id)
        .map(|_| ())
}

pub(crate) fn active_root(workspace: &crate::commands::WorkspaceState) -> Option<SourceRoot> {
    workspace.current().map(|epoch| epoch.source_root())
}

fn lock_library(state: &LibraryState) -> Result<std::sync::MutexGuard<'_, ()>, LibraryError> {
    state.0.lock().map_err(|_| LibraryError::PersistFailed)
}

/// Return every persisted local Markdown root and its live filesystem/index
/// state. A corrupt registry is a typed error, never an empty library.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn list_library_sources(
    app: AppHandle,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<LibrarySnapshot, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let active = active_root(&workspace);
    LibraryStore::new(path).read(|registry| {
        let mut snapshot = registry.snapshot(&app_data, active.as_ref())?;
        refresh_reader_index_metadata(
            registry,
            &mut snapshot,
            app.state::<crate::reader::ReaderBackend>().inner(),
        );
        Ok(snapshot)
    })
}

fn refresh_reader_index_metadata(
    registry: &LibraryRegistry,
    snapshot: &mut LibrarySnapshot,
    reader: &crate::reader::ReaderBackend,
) {
    for row in &mut snapshot.sources {
        let status = (|| -> Result<LibraryIndexState, String> {
            let binding = registry
                .source_for(&row.id)
                .ok_or("Source binding missing")?;
            let identity = binding
                .identity
                .clone()
                .ok_or("Source identity unavailable")?;
            let source: Option<mdreader_core::Source> =
                reader.typed(mdreader_service::Request::FindSource {
                    path: binding.path().map_err(|e| e.to_string())?,
                    identity,
                })?;
            let Some(source) = source else {
                return Ok(LibraryIndexState::NotCreated);
            };
            if !source.enabled {
                return Ok(LibraryIndexState::Unknown);
            }
            let indexed: mdreader_core::IndexStatus =
                reader.typed(mdreader_service::Request::Status {
                    sources: vec![source.id.clone()],
                })?;
            let job: Option<mdreader_core::IndexJob> =
                reader.typed(mdreader_service::Request::JobStatus {
                    source_id: source.id,
                })?;
            Ok(
                if indexed.documents > 0
                    || job.is_some_and(|job| {
                        matches!(
                            job.phase.as_str(),
                            "loading" | "embedding" | "done" | "model-unavailable"
                        )
                    })
                {
                    LibraryIndexState::Present
                } else {
                    LibraryIndexState::NotCreated
                },
            )
        })();
        row.index_state = status.unwrap_or(LibraryIndexState::Unknown);
    }
}

/// Open the real native folder picker and persist its canonical native path.
/// Re-selecting an existing member returns that member with `added = false`;
/// core admission still rejects duplicates, while the product can simply
/// activate the already-known source after a repeat pick.
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_and_add_library_source(
    app: AppHandle,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<PickedLibrarySource, LibraryError> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        // tauri-plugin-dialog deliberately has no mobile directory picker.
        // Refuse instead of treating a file pick or display path as a durable
        // directory grant.
        let _ = (app, workspace, state);
        return Err(LibraryError::NeedsNativeRepick);
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let Some(selected) = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(PickedLibrarySource {
            source: None,
            added: false,
        });
    };

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _effect = crate::profile_transition::workspace::effect(&app)
            .map_err(|_| LibraryError::PersistFailed)?;
        let _guard = lock_library(&state)?;
        let (path, app_data) = registry_path(&app)?;
        let active = active_root(&workspace);
        LibraryStore::new(path).update(|registry| {
            let canonical = fs::canonicalize(&selected).map_err(|_| LibraryError::InvalidRoot)?;
            let picked_identity =
                SourceRoot::capture(canonical).map_err(|_| LibraryError::InvalidRoot)?;
            let (source_id, added) = match registry.add_root(&selected) {
                Ok(source) => (source.id, true),
                Err(LibraryError::DuplicateRoot) => {
                    let source_id = registry
                        .sources
                        .iter()
                        .filter_map(LibrarySource::local)
                        .find(|source| source.identity.as_ref() == Some(&picked_identity.identity))
                        .map(|source| source.id.clone())
                        .ok_or(LibraryError::CorruptRegistry)?;
                    (source_id, false)
                }
                Err(error) => return Err(error),
            };
            let snapshot = registry.snapshot(&app_data, active.as_ref())?;
            Ok(PickedLibrarySource {
                source: snapshot
                    .sources
                    .into_iter()
                    .find(|source| source.id == source_id),
                added,
            })
        })
    }
}

/// Reconcile a persisted session root against available membership. Returning
/// `None` means no backend-owned source identity may be granted or watched.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn reconcile_library_restore(
    app: AppHandle,
    session_root: Option<String>,
    state: tauri::State<'_, LibraryState>,
) -> Result<Option<LibrarySourceDto>, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    reconcile_library_restore_at(path, app_data, session_root)
}

fn reconcile_library_restore_at(
    registry_path: PathBuf,
    app_data: PathBuf,
    session_root: Option<String>,
) -> Result<Option<LibrarySourceDto>, LibraryError> {
    LibraryStore::new(registry_path)
        .reconcile_restore_or_migrate(session_root.as_deref().map(Path::new), &app_data)
}

/// Arm and commit a persisted member by source ID. The registry supplies the
/// native `PathBuf`; the webview's display string never participates in the
/// grant transaction.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn watch_library_source(
    app: AppHandle,
    source_id: String,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    watcher: tauri::State<'_, crate::commands::WatcherState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<crate::commands::WorkspaceGrant, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, _) = registry_path(&app)?;
    LibraryStore::new(path).read(|registry| {
        let native_root = registry.activation_root(&source_id)?;
        app.state::<crate::reader::ReaderBackend>()
            .grant(&native_root)
            .map_err(|_| LibraryError::OpenFailed)?;
        let engine_source_id = registry
            .source_for(&source_id)
            .ok_or(LibraryError::SourceNotFound)?
            .engine_source_id();
        crate::commands::watch_workspace_path_exact_with_source(
            &app,
            &native_root,
            engine_source_id,
            &workspace,
            &watcher,
        )
        .map_err(|_| LibraryError::OpenFailed)
    })
}

/// Degraded first-open path for a known member when its filesystem watcher
/// cannot be armed. It establishes only the epoch grant, still from the same
/// lossless registry path and never from UI text.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn grant_library_source(
    app: AppHandle,
    source_id: String,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<String, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, _) = registry_path(&app)?;
    LibraryStore::new(path).read(|registry| {
        let native_root = registry.activation_root(&source_id)?;
        app.state::<crate::reader::ReaderBackend>()
            .grant(&native_root)
            .map_err(|_| LibraryError::OpenFailed)?;
        let engine_source_id = registry
            .source_for(&source_id)
            .ok_or(LibraryError::SourceNotFound)?
            .engine_source_id();
        crate::commands::grant_workspace_path_exact_with_source(
            &native_root,
            engine_source_id,
            &workspace,
        )
        .map(|root| root.to_string_lossy().into_owned())
        .map_err(|_| LibraryError::OpenFailed)
    })
}

/// Resolve a semantic source-relative document path only after the exact
/// persisted membership has become the active, identity-validated workspace.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn resolve_library_document_edit(
    app: AppHandle,
    source_id: String,
    relative_path: String,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<String, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, _) = registry_path(&app)?;
    let active = active_root(&workspace);
    LibraryStore::new(path).read(|registry| {
        registry
            .resolve_document_edit_path(&source_id, Path::new(&relative_path), active.as_ref())?
            .into_os_string()
            .into_string()
            .map_err(|_| LibraryError::InvalidDocumentPath)
    })
}

/// Persist the restore candidate only after the existing watch/grant path has
/// committed it as the current epoch. A caller cannot mint authority by setting
/// an arbitrary source ID.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn mark_library_source_active(
    app: AppHandle,
    source_id: String,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<LibrarySnapshot, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let active = active_root(&workspace);
    LibraryStore::new(path).update(|registry| {
        registry.mark_active(&source_id, active.as_ref())?;
        registry.snapshot(&app_data, active.as_ref())
    })
}

/// Forget one inactive membership and exactly its private managed state.
/// The user's source directory is never addressed by cleanup.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn forget_library_source(
    app: AppHandle,
    source_id: String,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<LibrarySnapshot, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let active = active_root(&workspace);
    let store = LibraryStore::new(path);
    let source = store.read(|registry| {
        let mut preflight = registry.clone();
        preflight.forget(&source_id, active.as_ref())?;
        registry
            .source_for(&source_id)
            .cloned()
            .ok_or(LibraryError::SourceNotFound)
    })?;
    revoke_reader_membership(app.state::<crate::reader::ReaderBackend>().inner(), &source)?;
    store.forget_source(&source_id, active.as_ref(), &app_data)?;
    store.read(|registry| registry.snapshot(&app_data, active.as_ref()))
}

/// Close the exact active local source, revoke its epoch and watcher, then
/// forget its membership and private managed state. User source bytes are
/// never cleanup targets.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn close_and_forget_library_source(
    app: AppHandle,
    source_id: String,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    watcher: tauri::State<'_, crate::commands::WatcherState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<CloseAndForgetLibrarySourceOutcome, LibraryError> {
    let _effect = crate::profile_transition::workspace::effect(&app)
        .map_err(|_| LibraryError::PersistFailed)?;
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let epoch = workspace.current().ok_or(LibraryError::GrantMismatch)?;
    let source = LibraryStore::new(path.clone()).read(|registry| {
        let source = registry
            .source_for(&source_id)
            .ok_or(LibraryError::SourceNotFound)?;
        if source.root()? != epoch.source_root()
            || Some(source.engine_source_id()) != epoch.source_id()
        {
            return Err(LibraryError::GrantMismatch);
        }
        Ok(source.clone())
    })?;
    revoke_reader_membership(app.state::<crate::reader::ReaderBackend>().inner(), &source)?;
    close_and_forget_library_source_at(path, app_data, &source_id, &workspace, &watcher)
}

fn revoke_reader_membership(
    reader: &crate::reader::ReaderBackend,
    source: &LocalFolderSource,
) -> Result<(), LibraryError> {
    let path = source.path()?;
    let identity = source
        .identity
        .clone()
        .ok_or(LibraryError::SourceUnavailable)?;
    reader
        .revoke_binding(path.clone(), identity)
        .map_err(|_| LibraryError::PersistFailed)?;
    #[cfg(not(mobile))]
    reader
        .pause_capture(&path)
        .map_err(|_| LibraryError::PersistFailed)?;
    Ok(())
}

fn close_and_forget_library_source_at(
    registry_path: PathBuf,
    app_data: PathBuf,
    source_id: &str,
    workspace: &crate::commands::WorkspaceState,
    watcher: &crate::commands::WatcherState,
) -> Result<CloseAndForgetLibrarySourceOutcome, LibraryError> {
    close_and_forget_library_source_at_with_ops(
        registry_path,
        app_data,
        source_id,
        workspace,
        watcher,
        |from, to| fs::rename(from, to),
        |path| fs::remove_dir_all(path),
    )
}

fn close_and_forget_library_source_at_with_ops(
    registry_path: PathBuf,
    app_data: PathBuf,
    source_id: &str,
    workspace: &crate::commands::WorkspaceState,
    watcher: &crate::commands::WatcherState,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
    cleanup: impl FnMut(&Path) -> io::Result<()>,
) -> Result<CloseAndForgetLibrarySourceOutcome, LibraryError> {
    let epoch = workspace.current().ok_or(LibraryError::GrantMismatch)?;
    let active = epoch.source_root();
    let active_engine_source_id = epoch.source_id();
    let outcome = LibraryStore::new(registry_path).close_and_forget_source_with_cleanup_ops(
        source_id,
        &active,
        active_engine_source_id,
        &app_data,
        move || {
            let revoked = crate::commands::revoke_workspace(&epoch, workspace, watcher)
                .map_err(|_| LibraryError::OpenFailed);
            drop(epoch);
            revoked
        },
        rename,
        cleanup,
    )?;
    Ok(CloseAndForgetLibrarySourceOutcome {
        status: outcome.status,
        workspace_generation: outcome.workspace_generation,
    })
}

pub(crate) fn classify_probe_error(error: io::Error) -> LibrarySourceState {
    match error.kind() {
        io::ErrorKind::NotFound => LibrarySourceState::Missing,
        io::ErrorKind::PermissionDenied => LibrarySourceState::Unreadable,
        _ => LibrarySourceState::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{
        classify_probe_error, close_and_forget_library_source_at,
        close_and_forget_library_source_at_with_ops, CloseAndForgetStatus, LibraryError,
        LibraryIndexState, LibraryRegistry, LibrarySourceState, LibraryStore, NativePath,
        LIBRARY_VERSION,
    };
    use mdreader_core::SourceRoot;

    fn mkdir(parent: &Path, name: &str) -> PathBuf {
        let path = parent.join(name);
        fs::create_dir(&path).expect("create source directory");
        path
    }

    fn bound_root(path: &Path) -> SourceRoot {
        SourceRoot::capture(fs::canonicalize(path).expect("canonical source root"))
            .expect("capture source root identity")
    }

    fn legacy_v1_workspace_path(app_data: &Path, root: &Path) -> PathBuf {
        let hash = if let Some(root) = root.to_str() {
            blake3::hash(root.as_bytes())
        } else {
            let mut hasher = blake3::Hasher::new();
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                hasher.update(b"mdreader-unix-native-root-v1\0");
                hasher.update(root.as_os_str().as_bytes());
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                hasher.update(b"mdreader-windows-native-root-v1\0");
                for unit in root.as_os_str().encode_wide() {
                    hasher.update(&unit.to_le_bytes());
                }
            }
            #[cfg(not(any(unix, windows)))]
            {
                hasher.update(b"mdreader-platform-native-root-v1\0");
                hasher.update(root.to_string_lossy().as_bytes());
            }
            hasher.finalize()
        }
        .to_hex()
        .to_string();
        app_data.join("workspaces").join(hash)
    }

    fn create_legacy_catdb_index(path: &Path) {
        fs::create_dir_all(path.parent().expect("legacy index parent"))
            .expect("create legacy workspace");
        let connection = rusqlite::Connection::open(path).expect("open legacy CatDB index");
        connection
            .execute_batch(
                "CREATE TABLE objects (id TEXT PRIMARY KEY);
                 CREATE TABLE documents (id TEXT PRIMARY KEY);
                 CREATE TABLE operation_log (seq INTEGER PRIMARY KEY);",
            )
            .expect("create exact legacy evidence tables");
    }

    fn v2_managed_paths(app_data: &Path, root: &SourceRoot) -> (PathBuf, PathBuf) {
        let old = mdreader_core::source_root::legacy_workspace_index_path_for_path_identity(
            app_data,
            &root.path,
            &root.identity,
        );
        let new = mdreader_core::source_root::workspace_index_path_for_stable_identity(
            app_data,
            &root.identity,
        );
        (old, new)
    }

    fn assert_v2_refusal_preserves_managed_state(
        registry_path: &Path,
        registry_before: &[u8],
        old_db: &Path,
        new_db: &Path,
    ) {
        assert!(matches!(
            LibraryRegistry::load_from(registry_path),
            Err(LibraryError::CorruptRegistry)
                | Err(LibraryError::CleanupFailed { .. })
                | Err(LibraryError::OfflineMigrationRequired)
        ));
        assert_eq!(
            fs::read(registry_path).expect("read unchanged V2 registry"),
            registry_before
        );
        assert!(old_db.exists(), "refusal preserves the old managed state");
        assert!(
            !new_db.exists(),
            "refusal never publishes partially adopted managed state"
        );
    }

    #[test]
    fn duplicate_and_non_directory_roots_are_rejected_without_mutating_membership() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let file = temp.path().join("not-a-folder.md");
        fs::write(&file, "# no").expect("write file");
        let mut registry = LibraryRegistry::empty();

        let first = registry.add_root(&root).expect("add root");
        let duplicate = registry.add_root(&root).expect_err("duplicate must fail");
        assert_eq!(duplicate, LibraryError::DuplicateRoot);
        assert!(matches!(
            registry.add_root(&file),
            Err(LibraryError::InvalidRoot)
        ));
        assert_eq!(registry.sources().len(), 1);
        assert_eq!(registry.sources()[0].id, first.id);
    }

    #[test]
    fn repicking_a_renamed_filesystem_object_preserves_its_opaque_source_identity() {
        let temp = tempdir().expect("tempdir");
        let original = mkdir(temp.path(), "original");
        let mut registry = LibraryRegistry::empty();
        let admitted = registry.add_root(&original).expect("add original root");
        let path_derived =
            NativePath::capture(&fs::canonicalize(&original).expect("canonical original root"))
                .source_id();

        assert_ne!(
            admitted.id, path_derived,
            "a renderer-visible path must not determine membership identity"
        );

        let renamed = temp.path().join("renamed");
        fs::rename(&original, &renamed).expect("rename exact admitted filesystem object");
        let repicked = registry
            .add_root(&renamed)
            .expect("same retained object updates its address");

        assert_eq!(repicked.id, admitted.id);
        assert_eq!(registry.sources().len(), 1);
        assert_eq!(
            registry
                .path_for(&admitted.id)
                .expect("updated source address"),
            fs::canonicalize(&renamed).expect("canonical renamed root")
        );
    }

    #[test]
    fn same_path_object_replacement_gets_a_new_source_identity() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let mut registry = LibraryRegistry::empty();
        let original = registry.add_root(&root).expect("add original object");

        let retired = temp.path().join("retired");
        fs::rename(&root, &retired).expect("retire original object");
        fs::create_dir(&root).expect("create replacement at same path");
        let replacement = registry.add_root(&root).expect("add replacement object");

        assert_ne!(replacement.id, original.id);
        assert_eq!(registry.sources().len(), 2);
        assert!(matches!(
            registry.source_state(&original.id),
            Ok(LibrarySourceState::Unreadable)
        ));
        assert_eq!(
            registry
                .activation_path(&replacement.id)
                .expect("replacement activates"),
            fs::canonicalize(&root).expect("canonical replacement")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_alias_of_a_member_never_creates_duplicate_membership() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let alias = temp.path().join("alias");
        symlink(&root, &alias).expect("create source alias");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&root).expect("add real path");

        assert_eq!(registry.add_root(&alias), Err(LibraryError::DuplicateRoot));
        assert_eq!(registry.sources().len(), 1);
    }

    #[test]
    fn native_path_units_round_trip_without_loss() {
        #[cfg(unix)]
        let native_name = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![b'n', b'o', b't', b'e', b's', b'-', 0xFF])
        };
        #[cfg(not(unix))]
        let native_name = OsString::from("notes-native");

        // Some filesystems (notably a UTF-8-normalising macOS volume) refuse to
        // create a deliberately non-UTF-8 name. The persistence contract is the
        // native encoding itself, so exercise it directly without making the
        // filesystem decide whether this particular sequence is admissible.
        let root = PathBuf::from("/").join(native_name);
        let stored = NativePath::capture(&root);
        assert_eq!(stored.restore().expect("restore native path"), root);
    }

    #[test]
    fn missing_and_unreadable_are_typed_states_never_empty_membership() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add root");

        fs::remove_dir(&root).expect("remove source only");
        assert!(matches!(
            registry.source_state(&source.id),
            Ok(LibrarySourceState::Missing)
        ));
        assert_eq!(registry.sources().len(), 1, "missing source remains known");

        let denied = classify_probe_error(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(matches!(denied, LibrarySourceState::Unreadable));
    }

    #[test]
    fn a_different_directory_at_the_same_canonical_path_is_not_the_admitted_source() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        fs::write(root.join("original.md"), "# original").expect("write admitted document");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add root");
        let registry_path = temp.path().join("library.json");
        registry
            .save_to(&registry_path)
            .expect("persist admitted identity");
        let registry =
            LibraryRegistry::load_from(&registry_path).expect("restore admitted identity");

        let moved = temp.path().join("moved-original");
        fs::rename(&root, &moved).expect("move admitted filesystem object");
        fs::create_dir(&root).expect("create replacement at the stored path");
        fs::write(root.join("replacement.md"), "# replacement").expect("write replacement");

        let state = registry
            .source_state(&source.id)
            .expect("identity mismatch is a source state");
        assert_eq!(state, LibrarySourceState::Unreadable);
        assert!(matches!(
            registry.activation_path(&source.id),
            Err(LibraryError::SourceUnavailable)
        ));
        assert_eq!(
            fs::read_to_string(moved.join("original.md")).expect("original object remains"),
            "# original"
        );
    }

    #[test]
    fn edit_path_resolution_uses_the_active_identity_bound_membership_root() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        fs::create_dir(root.join("nested")).expect("nested directory");
        let document = root.join("nested").join("document.md");
        fs::write(&document, "# document").expect("document");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add root");
        let active = bound_root(&fs::canonicalize(&root).expect("canonical root"));
        let expected = active.path.join("nested/document.md");

        assert_eq!(
            registry.resolve_document_edit_path(
                &source.id,
                Path::new("nested/document.md"),
                Some(&active),
            ),
            Ok(expected.clone())
        );
        assert_eq!(
            registry.resolve_document_edit_path(
                &source.id,
                Path::new("../outside.md"),
                Some(&active),
            ),
            Err(LibraryError::InvalidDocumentPath)
        );
        assert_eq!(
            registry.resolve_document_edit_path(&source.id, &expected, Some(&active)),
            Err(LibraryError::InvalidDocumentPath)
        );
    }

    #[test]
    fn edit_path_resolution_refuses_same_path_source_replacement() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        fs::write(root.join("original.md"), "# original").expect("original");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add root");
        let active = bound_root(&fs::canonicalize(&root).expect("canonical root"));

        fs::rename(&root, temp.path().join("retired")).expect("retire original");
        fs::create_dir(&root).expect("replacement root");
        fs::write(root.join("replacement.md"), "# replacement").expect("replacement");

        assert_eq!(
            registry.resolve_document_edit_path(
                &source.id,
                Path::new("replacement.md"),
                Some(&active),
            ),
            Err(LibraryError::SourceUnavailable)
        );
    }

    #[test]
    fn refusing_to_forget_an_active_source_preserves_user_and_managed_state() {
        let temp = tempdir().expect("tempdir");
        let source_root = mkdir(temp.path(), "notes");
        let user_file = source_root.join("kept.md");
        fs::write(&user_file, "# keep").expect("write user file");
        let index_file = temp.path().join("catdb.db");
        fs::write(&index_file, b"index stays").expect("write index marker");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&source_root).expect("add root");
        let forgotten_id = source.id.clone();
        let canonical = fs::canonicalize(&source_root).expect("canonical root");
        let active = bound_root(&canonical);

        assert_eq!(
            registry
                .forget(&source.id, Some(&active))
                .expect_err("active forget must fail"),
            LibraryError::ActiveSource
        );
        assert_eq!(registry.sources().len(), 1);

        registry
            .forget(&source.id, None)
            .expect("inactive membership can be forgotten");
        let after_forget = registry
            .snapshot(temp.path(), None)
            .expect("post-forget snapshot");
        assert!(after_forget
            .sources
            .iter()
            .all(|source| source.id != forgotten_id));
        assert!(
            source_root.join("kept.md").exists(),
            "Forget must not delete user bytes"
        );
        assert_eq!(
            fs::read_to_string(user_file).expect("user file remains"),
            "# keep"
        );
        assert_eq!(fs::read(index_file).expect("index remains"), b"index stays");
    }

    #[test]
    fn restore_requires_available_membership_and_never_fabricates_a_grant() {
        let temp = tempdir().expect("tempdir");
        let member = mkdir(temp.path(), "member");
        let outsider = mkdir(temp.path(), "outsider");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&member).expect("add member");
        let canonical_member = fs::canonicalize(&member).expect("canonical member");

        assert_eq!(
            registry
                .reconcile_restore(Some(&canonical_member))
                .expect("reconcile"),
            Some(source.id.clone())
        );
        assert_eq!(
            registry
                .reconcile_restore(Some(&outsider))
                .expect("outsider is a clean miss"),
            None
        );

        fs::remove_dir(&member).expect("remove member source");
        assert_eq!(
            registry
                .reconcile_restore(Some(&canonical_member))
                .expect("missing source is a clean refusal"),
            None
        );
        assert_eq!(
            registry.reconcile_restore(None).expect("no session root"),
            None
        );
    }

    #[test]
    fn corrupt_unsupported_or_duplicate_persistence_is_rejected_not_defaulted_empty() {
        let temp = tempdir().expect("tempdir");
        let file = temp.path().join("library.json");
        fs::write(&file, b"{ definitely not json").expect("write corrupt registry");
        assert!(matches!(
            LibraryRegistry::load_from(&file),
            Err(LibraryError::CorruptRegistry)
        ));

        fs::write(
            &file,
            format!(
                r#"{{"version":{},"active_source_id":null,"sources":[]}}"#,
                LIBRARY_VERSION + 1
            ),
        )
        .expect("write unsupported registry");
        assert!(matches!(
            LibraryRegistry::load_from(&file),
            Err(LibraryError::UnsupportedVersion { .. })
        ));

        let root = mkdir(temp.path(), "notes");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&root).expect("add source");
        registry.save_to(&file).expect("save current registry");
        let mut body: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).expect("read registry")).expect("parse json");
        body["sources"][0]
            .as_object_mut()
            .expect("source object")
            .remove("identity");
        fs::write(&file, serde_json::to_vec_pretty(&body).expect("encode"))
            .expect("write identity-less registry");
        assert!(matches!(
            LibraryRegistry::load_from(&file),
            Err(LibraryError::CorruptRegistry)
        ));
    }

    #[test]
    fn v8_registry_migrates_to_v9_with_an_explicit_empty_restore_suppression_set() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("library.json");
        let root = mkdir(temp.path(), "notes");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("mark active");
        registry.save_to(&registry_path).expect("write v9 fixture");
        let mut v8: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read fixture"))
                .expect("decode fixture");
        v8["version"] = serde_json::json!(8);
        v8.as_object_mut()
            .expect("registry object")
            .remove("restore_suppressed_source_ids");
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&v8).expect("encode v8"),
        )
        .expect("write v8");

        let migrated = LibraryRegistry::load_from(&registry_path).expect("migrate v8");

        assert_eq!(migrated.restore_source_id(), Some(source.id.as_str()));
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read migrated"))
                .expect("decode migrated");
        assert_eq!(persisted["version"], serde_json::json!(LIBRARY_VERSION));
        assert_eq!(
            persisted["restore_suppressed_source_ids"],
            serde_json::json!([])
        );
    }

    #[test]
    fn corrupt_restore_suppression_ids_are_rejected_on_load() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("library.json");
        let root = mkdir(temp.path(), "notes");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .save_to(&registry_path)
            .expect("write valid registry");
        let valid: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read valid registry"))
                .expect("decode registry");

        for suppressed in [
            serde_json::json!(["f".repeat(64)]),
            serde_json::json!([source.id.clone(), source.id.clone()]),
        ] {
            let mut corrupt = valid.clone();
            corrupt["restore_suppressed_source_ids"] = suppressed;
            fs::write(
                &registry_path,
                serde_json::to_vec_pretty(&corrupt).expect("encode corrupt registry"),
            )
            .expect("write corrupt registry");
            assert!(matches!(
                LibraryRegistry::load_from(&registry_path),
                Err(LibraryError::CorruptRegistry)
            ));
        }

        let mut active_and_suppressed = valid;
        active_and_suppressed["active_source_id"] = serde_json::json!(source.id.clone());
        active_and_suppressed["restore_suppressed_source_ids"] = serde_json::json!([source.id]);
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&active_and_suppressed)
                .expect("encode active+suppress registry"),
        )
        .expect("write corrupt registry");
        assert!(matches!(
            LibraryRegistry::load_from(&registry_path),
            Err(LibraryError::CorruptRegistry)
        ));
    }

    #[test]
    fn version_one_registry_quarantines_managed_bytes_without_creating_membership() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        fs::create_dir(&app_data).expect("create app data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let native = NativePath::capture(&canonical);
        let source_id = native.source_id();
        let v1 = serde_json::json!({
            "version": 1,
            "active_source_id": source_id,
            "sources": [{
                "id": source_id,
                "path": native,
            }],
        });
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&v1).expect("encode v1 registry"),
        )
        .expect("write v1 registry");

        let legacy = legacy_v1_workspace_path(&app_data, &canonical);
        fs::create_dir_all(&legacy).expect("create legacy workspace");
        fs::write(legacy.join("catdb.db"), b"legacy index").expect("write legacy index");
        fs::write(legacy.join("proposals.db"), b"legacy proposals")
            .expect("write legacy proposals");
        let legacy_key = legacy.file_name().expect("legacy key").to_os_string();
        let quarantine = app_data.join("legacy-v1-path-workspaces").join(legacy_key);

        let registry =
            LibraryRegistry::load_from(&registry_path).expect("migrate supported v1 registry");

        assert!(
            registry.sources().is_empty(),
            "path-only V1 rows are never live membership"
        );
        assert_eq!(
            registry.restore_source_id(),
            None,
            "a path-only active marker cannot authorize a restore after migration"
        );
        assert!(!legacy.exists(), "the path-keyed workspace is never reused");
        assert_eq!(
            fs::read(quarantine.join("catdb.db")).expect("recover quarantined index"),
            b"legacy index"
        );
        assert_eq!(
            fs::read(quarantine.join("proposals.db")).expect("recover quarantined proposals"),
            b"legacy proposals"
        );
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read migrated registry"))
                .expect("parse migrated registry");
        assert_eq!(persisted["version"], serde_json::json!(LIBRARY_VERSION));
        assert_eq!(persisted["sources"], serde_json::json!([]));
    }

    #[test]
    fn version_one_path_is_only_a_candidate_until_the_original_object_is_native_repicked() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        fs::create_dir(&app_data).expect("create app data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let native = NativePath::capture(&canonical);
        let path_id = native.source_id();
        let v1 = serde_json::json!({
            "version": 1,
            "active_source_id": path_id,
            "sources": [{
                "id": path_id,
                "path": native,
            }],
        });
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&v1).expect("encode V1 registry"),
        )
        .expect("write V1 registry");

        let mut registry =
            LibraryRegistry::load_from(&registry_path).expect("record recovery candidate");
        assert!(
            registry.sources().is_empty(),
            "a path-only row must not become live membership"
        );

        let admitted = registry
            .add_root(&canonical)
            .expect("the native picker re-admits the retained object");
        assert_eq!(registry.sources().len(), 1);
        assert_ne!(
            admitted.id, path_id,
            "the path-derived V1 identifier is never reused as authority"
        );
    }

    #[test]
    fn version_one_same_path_replacement_never_inherits_live_membership_or_managed_rows() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        fs::create_dir(&app_data).expect("create app data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let native = NativePath::capture(&canonical);
        let path_id = native.source_id();
        let v1 = serde_json::json!({
            "version": 1,
            "active_source_id": path_id,
            "sources": [{
                "id": path_id,
                "path": native,
            }],
        });
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&v1).expect("encode V1 registry"),
        )
        .expect("write V1 registry");
        let legacy = legacy_v1_workspace_path(&app_data, &canonical);
        fs::create_dir_all(&legacy).expect("create V1 workspace");
        fs::write(legacy.join("catdb.db"), b"original index").expect("write original index");

        let original = temp.path().join("retained-original");
        fs::rename(&root, &original).expect("retain original object elsewhere");
        fs::create_dir(&root).expect("create replacement at historical path");

        let mut registry =
            LibraryRegistry::load_from(&registry_path).expect("record recovery candidate");
        assert!(
            registry.sources().is_empty(),
            "migration must not capture the replacement object"
        );
        let replacement = registry
            .add_root(&root)
            .expect("native picker admits replacement as a new source");
        assert_eq!(registry.sources().len(), 1);
        assert_ne!(replacement.id, path_id);
        assert!(
            !crate::engine::db_path_under(
                &app_data,
                &registry
                    .activation_root(&replacement.id)
                    .expect("replacement is admitted")
            )
            .exists(),
            "replacement starts with no inherited CatDB state"
        );
    }

    #[test]
    fn unavailable_v1_membership_fails_closed_until_native_identity_can_be_rebound() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        fs::create_dir(&app_data).expect("create app data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "offline-notes");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let native = NativePath::capture(&canonical);
        let source_id = native.source_id();
        let v1 = serde_json::json!({
            "version": 1,
            "active_source_id": source_id,
            "sources": [{
                "id": source_id,
                "path": native,
            }],
        });
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&v1).expect("encode v1 registry"),
        )
        .expect("write v1 registry");
        fs::remove_dir(&root).expect("take source offline before migration");

        let registry = LibraryRegistry::load_from(&registry_path)
            .expect("missing V1 source remains a non-authoritative recovery candidate");
        assert!(registry.sources().is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(&registry_path).expect("candidate registry persists")
            )
            .expect("parse candidate registry")["version"],
            serde_json::json!(LIBRARY_VERSION)
        );

        let first_bytes = fs::read(&registry_path).expect("read candidate registry");
        fs::create_dir(&root).expect("source returns");
        let mut registry =
            LibraryRegistry::load_from(&registry_path).expect("migration remains idempotent");
        assert!(registry.sources().is_empty());
        assert_eq!(
            fs::read(&registry_path).expect("read candidate registry again"),
            first_bytes,
            "restart does not re-run or mutate migration"
        );
        let admitted = registry
            .add_root(&root)
            .expect("native re-pick explicitly creates membership");
        assert_ne!(admitted.id, source_id);
    }

    #[test]
    fn version_two_present_workspace_without_catdb_db_refuses_fresh_identity() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
        let before = fs::read(&registry_path).expect("read V2 registry");
        let (old_db, new_db) = v2_managed_paths(&app_data, &root);
        fs::create_dir_all(old_db.parent().expect("old workspace"))
            .expect("create present managed workspace");
        fs::write(
            old_db.with_file_name("proposals.db"),
            b"managed-but-no-index",
        )
        .expect("write managed sibling");

        assert_v2_refusal_preserves_managed_state(
            &registry_path,
            &before,
            &old_db.with_file_name("proposals.db"),
            &new_db,
        );
    }

    #[test]
    fn version_two_source_shaped_non_catdb_index_is_never_adopted() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
        let before = fs::read(&registry_path).expect("read V2 registry");
        let (old_db, new_db) = v2_managed_paths(&app_data, &root);
        fs::create_dir_all(old_db.parent().expect("old workspace")).expect("create old workspace");
        let impostor = rusqlite::Connection::open(&old_db).expect("open SQLite impostor");
        impostor
            .execute_batch(
                "CREATE TABLE sources (
                   source_id TEXT PRIMARY KEY,
                   root_path TEXT NOT NULL,
                   enabled INTEGER NOT NULL,
                   watch INTEGER NOT NULL,
                   include_globs TEXT NOT NULL,
                   exclude_globs TEXT NOT NULL,
                   max_file_size INTEGER NOT NULL,
                   respect_gitignore INTEGER NOT NULL,
                   embed_policy TEXT NOT NULL
                 );
                 PRAGMA user_version = 7;",
            )
            .expect("create source-shaped impostor");
        impostor
            .execute(
                "INSERT INTO sources VALUES (?1, ?2, 1, 0, '[]', '[]', 1, 1, 'auto')",
                (
                    uuid::Uuid::new_v4().to_string(),
                    root.path.display().to_string(),
                ),
            )
            .expect("insert plausible source");
        drop(impostor);

        assert_v2_refusal_preserves_managed_state(&registry_path, &before, &old_db, &new_db);
    }

    // APFS rejects arbitrary non-UTF-8 directory names at creation time, so
    // this authentic filesystem/engine migration runs on Unix platforms that
    // can actually manufacture the historical path.

    #[cfg(unix)]
    #[test]
    fn version_two_refuses_symlinked_and_non_regular_catdb_db() {
        use std::os::unix::fs::symlink;

        for kind in ["symlink", "directory"] {
            let temp = tempdir().expect("tempdir");
            let app_data = temp.path().join("app-data");
            let registry_path = app_data.join("library.json");
            let root = bound_root(&mkdir(temp.path(), "notes"));
            super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
            let before = fs::read(&registry_path).expect("read V2 registry");
            let (old_db, new_db) = v2_managed_paths(&app_data, &root);
            fs::create_dir_all(old_db.parent().expect("old workspace"))
                .expect("create old workspace");
            if kind == "symlink" {
                let outside = temp.path().join("outside.db");
                fs::write(&outside, b"outside bytes").expect("write outside target");
                symlink(&outside, &old_db).expect("substitute symlinked DB");
            } else {
                fs::create_dir(&old_db).expect("substitute directory DB");
            }

            assert_v2_refusal_preserves_managed_state(&registry_path, &before, &old_db, &new_db);
        }
    }

    #[test]
    fn version_four_requires_non_nil_unique_engine_source_ids() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("app-data/library.json");
        let mut registry = LibraryRegistry::empty();
        registry
            .add_root(&mkdir(temp.path(), "one"))
            .expect("add first source");
        registry
            .add_root(&mkdir(temp.path(), "two"))
            .expect("add second source");
        registry.save_to(&registry_path).expect("save valid V4");
        let valid: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read valid V4 registry"))
                .expect("parse valid V4 registry");

        let mut duplicate = valid.clone();
        let sources = duplicate["sources"].as_array_mut().expect("source array");
        let first_id = sources[0]["engine_source_id"].clone();
        sources[1]["engine_source_id"] = first_id;
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&duplicate).expect("encode duplicate V4"),
        )
        .expect("write duplicate V4");
        assert!(matches!(
            LibraryRegistry::load_from(&registry_path),
            Err(LibraryError::CorruptRegistry)
        ));

        let mut nil = valid;
        nil["sources"][0]["engine_source_id"] =
            serde_json::json!("00000000-0000-0000-0000-000000000000");
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&nil).expect("encode nil V4"),
        )
        .expect("write nil V4");
        assert!(matches!(
            LibraryRegistry::load_from(&registry_path),
            Err(LibraryError::CorruptRegistry)
        ));
    }

    #[test]
    fn version_three_migrates_once_into_the_unified_cloud_library_registry() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("app-data/library.json");
        let mut registry = LibraryRegistry::empty();
        registry
            .add_root(&mkdir(temp.path(), "notes"))
            .expect("add local source");
        registry.save_to(&registry_path).expect("save V4 registry");
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read registry"))
                .expect("parse registry");
        legacy["version"] = serde_json::json!(3);
        let local = legacy["sources"][0].as_object_mut().expect("local source");
        local.remove("kind");
        local.remove("contract");
        legacy
            .as_object_mut()
            .expect("registry object")
            .remove("cloud_sources");
        legacy
            .as_object_mut()
            .expect("registry object")
            .remove("connected_catdb_rediscovery_required");
        legacy
            .as_object_mut()
            .expect("registry object")
            .remove("restore_suppressed_source_ids");
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&legacy).expect("encode V3"),
        )
        .expect("write V3");

        let migrated = LibraryRegistry::load_from(&registry_path).expect("migrate V3");
        assert_eq!(migrated.sources().len(), 1);
        assert_eq!(migrated.cloud_source_count(), 0);
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read migrated registry"))
                .expect("parse migrated registry");
        assert_eq!(persisted["version"], serde_json::json!(LIBRARY_VERSION));
        assert!(persisted.get("cloud_sources").is_none());
        assert_eq!(persisted["sources"][0]["kind"], "local-folder");
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("migration is idempotent")
                .sources()
                .len(),
            1
        );
    }

    #[test]
    fn version_two_migration_without_identity_fails_closed_and_preserves_bytes() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("app-data/library.json");
        fs::create_dir_all(registry_path.parent().expect("registry parent"))
            .expect("create app data");
        let root = mkdir(temp.path(), "notes");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let native = NativePath::capture(&canonical);
        let path_id = native.source_id();
        let v2 = serde_json::json!({
            "version": 2,
            "active_source_id": null,
            "sources": [{
                "id": path_id,
                "path": native,
                "identity": null,
            }],
        });
        let before = serde_json::to_vec_pretty(&v2).expect("encode V2 registry");
        fs::write(&registry_path, &before).expect("write V2 registry");

        assert!(matches!(
            LibraryRegistry::load_from(&registry_path),
            Err(LibraryError::CorruptRegistry)
        ));
        assert_eq!(fs::read(&registry_path).expect("read unchanged V2"), before);
    }

    #[test]
    fn active_marker_requires_the_matching_live_grant_and_snapshot_reports_index_truth() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let other = mkdir(temp.path(), "other");
        let app_data = temp.path().join("app-data");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add root");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let canonical_root = bound_root(&canonical);
        let other_root = bound_root(&other);

        assert!(matches!(
            registry.mark_active(&source.id, Some(&other_root)),
            Err(LibraryError::GrantMismatch)
        ));
        assert!(registry.restore_source_id().is_none());

        registry
            .mark_active(&source.id, Some(&canonical_root))
            .expect("matching grant may become restore candidate");
        let before = registry
            .snapshot(&app_data, Some(&canonical_root))
            .expect("snapshot before index");
        let row = &before.sources[0];
        assert!(row.active);
        assert!(row.restore_candidate);
        assert_eq!(row.index_state, LibraryIndexState::NotCreated);

        let db = crate::engine::db_path_under(&app_data, &canonical_root);
        fs::create_dir_all(db.parent().expect("db parent")).expect("create index directory");
        fs::write(db, b"existing index").expect("write index");
        let after = registry
            .snapshot(&app_data, Some(&canonical_root))
            .expect("snapshot after index");
        assert_eq!(after.sources[0].index_state, LibraryIndexState::Present);
    }

    #[test]
    fn reader_snapshot_reports_sqlite_index_without_admitting_unindexed_sources() {
        let temp = tempdir().unwrap();
        let path = mkdir(temp.path(), "notes");
        let unindexed = mkdir(temp.path(), "unindexed");
        fs::write(path.join("note.md"), "# Note\nSQLite metadata\n").unwrap();
        let app_data = temp.path().join("app-data");
        let backend = crate::reader::ReaderBackend::embedded(&app_data.join("reader-v1")).unwrap();
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&path).unwrap();
        registry.add_root(&unindexed).unwrap();
        let root = bound_root(&path);
        let source = backend.admit(&root).unwrap();
        backend
            .call(mdreader_service::Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        let mut snapshot = registry.snapshot(&app_data, Some(&root)).unwrap();
        super::refresh_reader_index_metadata(&registry, &mut snapshot, &backend);
        assert_eq!(
            snapshot
                .sources
                .iter()
                .find(|s| s.name == "notes")
                .unwrap()
                .index_state,
            LibraryIndexState::Present
        );
        assert_eq!(
            snapshot
                .sources
                .iter()
                .find(|s| s.name == "unindexed")
                .unwrap()
                .index_state,
            LibraryIndexState::NotCreated
        );
        let sources: Vec<mdreader_core::Source> = backend
            .typed(mdreader_service::Request::ListSources)
            .unwrap();
        assert_eq!(
            sources.len(),
            1,
            "reading metadata must not grant additional folders"
        );
        backend
            .revoke_binding(root.path.clone(), root.identity.clone())
            .unwrap();
        super::refresh_reader_index_metadata(&registry, &mut snapshot, &backend);
        assert_eq!(
            snapshot
                .sources
                .iter()
                .find(|s| s.name == "notes")
                .unwrap()
                .index_state,
            LibraryIndexState::Unknown
        );
    }

    #[test]
    fn snapshot_reports_identity_keyed_managed_index_while_source_is_offline() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let app_data = temp.path().join("app-data");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&root).expect("add root");
        let admitted = bound_root(&root);
        let db = crate::engine::db_path_under(&app_data, &admitted);
        fs::create_dir_all(db.parent().expect("managed workspace parent"))
            .expect("create managed workspace");
        fs::write(&db, b"existing index").expect("write managed index");

        fs::rename(&root, temp.path().join("renamed-offline"))
            .expect("make stored address unavailable");
        let snapshot = registry
            .snapshot(&app_data, None)
            .expect("offline membership remains listable");

        assert!(matches!(
            snapshot.sources[0].state,
            super::LibraryAvailability::Missing
        ));
        assert_eq!(
            snapshot.sources[0].index_state,
            LibraryIndexState::Present,
            "managed state remains locatable from persisted stable identity"
        );
    }

    #[test]
    fn forgetting_a_source_removes_only_its_managed_workspace_and_preserves_user_bytes() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let document = root.join("keep.md");
        fs::write(&document, b"# keep\nsentinel body").expect("write user document");
        let before = fs::metadata(&document)
            .expect("document metadata")
            .modified()
            .expect("document mtime");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add root");
        let admitted = bound_root(&root);
        let managed = crate::engine::db_path_under(&app_data, &admitted)
            .parent()
            .expect("managed workspace")
            .to_path_buf();
        fs::create_dir_all(managed.join("cache")).expect("create managed workspace");
        fs::write(managed.join("catdb.db"), b"index").expect("write index");
        fs::write(managed.join("proposals.db"), b"proposals").expect("write proposals");
        fs::write(managed.join("cache/materialized"), b"cache").expect("write cache");
        registry.save_to(&registry_path).expect("save registry");

        LibraryStore::new(registry_path.clone())
            .forget_source(&source.id, None, &app_data)
            .expect("forget source and managed state");

        assert!(!managed.exists(), "all source-managed state is removed");
        assert!(LibraryRegistry::load_from(&registry_path)
            .expect("load post-forget registry")
            .sources()
            .is_empty());
        assert_eq!(
            fs::read(&document).expect("read untouched user document"),
            b"# keep\nsentinel body"
        );
        assert_eq!(
            fs::metadata(&document)
                .expect("document metadata after forget")
                .modified()
                .expect("document mtime after forget"),
            before
        );
    }

    #[test]
    fn closing_the_sole_active_source_revokes_before_private_cleanup_and_preserves_user_bytes() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let document = root.join("keep.md");
        fs::write(&document, b"# user authority").expect("write user document");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add sole source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("mark restore candidate");
        let managed = crate::engine::db_path_under(&app_data, &active)
            .parent()
            .expect("managed workspace")
            .to_path_buf();
        fs::create_dir_all(&managed).expect("create managed workspace");
        fs::write(managed.join("catdb.db"), b"private index").expect("write private index");
        registry.save_to(&registry_path).expect("save registry");
        let deactivated = Arc::new(AtomicBool::new(false));
        let app_data_for_rename = app_data.clone();
        let managed_for_rename = managed.clone();
        let app_data_for_cleanup = app_data.clone();
        let root_for_cleanup = root.clone();

        let outcome = LibraryStore::new(registry_path.clone())
            .close_and_forget_source_with_cleanup_ops(
                &source.id,
                &active,
                Some(source.engine_source_id()),
                &app_data,
                {
                    let deactivated = Arc::clone(&deactivated);
                    move || {
                        deactivated.store(true, Ordering::Release);
                        Ok(7)
                    }
                },
                {
                    let deactivated = Arc::clone(&deactivated);
                    move |from, to| {
                        assert!(
                            deactivated.load(Ordering::Acquire),
                            "authority must be revoked before quarantine"
                        );
                        assert_eq!(from, managed_for_rename);
                        assert!(to.starts_with(app_data_for_rename.join("managed-delete-staging")));
                        fs::rename(from, to)
                    }
                },
                {
                    let deactivated = Arc::clone(&deactivated);
                    move |path| {
                        assert!(
                            deactivated.load(Ordering::Acquire),
                            "authority must be revoked before private deletion"
                        );
                        assert_ne!(path, root_for_cleanup);
                        assert!(
                            path.starts_with(app_data_for_cleanup.join("managed-delete-staging"))
                        );
                        fs::remove_dir_all(path)
                    }
                },
            )
            .expect("close and forget sole source");

        assert_eq!(outcome.status, CloseAndForgetStatus::Complete);
        assert_eq!(outcome.workspace_generation, 7);
        assert!(LibraryRegistry::load_from(&registry_path)
            .expect("reload completed registry")
            .sources()
            .is_empty());
        assert_eq!(
            fs::read(document).expect("user document survives"),
            b"# user authority"
        );
    }

    #[test]
    fn close_and_forget_command_core_revokes_the_exact_active_membership() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("persist active membership");
        registry.save_to(&registry_path).expect("save registry");
        let workspace = crate::commands::WorkspaceState::default();
        workspace
            .switch_bound_with_source(active.clone(), Some(source.engine_source_id()))
            .expect("grant exact semantic source");
        let watcher = crate::commands::WatcherState::default();
        let old_epoch = workspace.current().expect("active epoch");

        let outcome = close_and_forget_library_source_at(
            registry_path.clone(),
            app_data,
            &source.id,
            &workspace,
            &watcher,
        )
        .expect("close exact active source");

        assert_eq!(outcome.status, CloseAndForgetStatus::Complete);
        assert!(LibraryRegistry::load_from(&registry_path)
            .expect("load completed registry")
            .sources()
            .is_empty());
        assert!(workspace.current().is_none());
        assert!(!workspace.is_current(&old_epoch));
        assert!(outcome.workspace_generation > old_epoch.generation as u32);
    }

    #[test]
    fn command_drops_the_captured_epoch_before_private_quarantine_and_cleanup() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("persist active membership");
        let managed = crate::engine::db_path_under(&app_data, &active)
            .parent()
            .expect("managed workspace")
            .to_path_buf();
        fs::create_dir_all(&managed).expect("create managed workspace");
        fs::write(managed.join("catdb.db"), b"private index").expect("write private index");
        registry.save_to(&registry_path).expect("save registry");
        let workspace = crate::commands::WorkspaceState::default();
        workspace
            .switch_bound_with_source(active, Some(source.engine_source_id()))
            .expect("grant exact semantic source");
        let epoch = workspace.current().expect("active epoch");
        let epoch_weak = Arc::downgrade(&epoch);
        drop(epoch);
        let watcher = crate::commands::WatcherState::default();
        let weak_for_rename = epoch_weak.clone();

        close_and_forget_library_source_at_with_ops(
            registry_path,
            app_data,
            &source.id,
            &workspace,
            &watcher,
            move |from, to| {
                assert!(
                    weak_for_rename.upgrade().is_none(),
                    "the exact epoch and its engine handles must die before quarantine"
                );
                fs::rename(from, to)
            },
            move |path| {
                assert!(
                    epoch_weak.upgrade().is_none(),
                    "the exact epoch must stay dead before private deletion"
                );
                fs::remove_dir_all(path)
            },
        )
        .expect("close after dropping native authority");
    }

    #[test]
    fn close_and_forget_command_core_rejects_a_different_membership_without_revoking() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root_a = mkdir(temp.path(), "notes-a");
        let root_b = mkdir(temp.path(), "notes-b");
        let active_a = bound_root(&root_a);
        let mut registry = LibraryRegistry::empty();
        let source_a = registry.add_root(&root_a).expect("add active source");
        let source_b = registry.add_root(&root_b).expect("add other source");
        registry
            .mark_active(&source_a.id, Some(&active_a))
            .expect("persist active membership");
        registry.save_to(&registry_path).expect("save registry");
        let workspace = crate::commands::WorkspaceState::default();
        workspace
            .switch_bound_with_source(bound_root(&root_b), Some(source_b.engine_source_id()))
            .expect("grant source B while stale registry restore candidate still names A");
        let watcher = crate::commands::WatcherState::default();
        let old_epoch = workspace.current().expect("active epoch");
        let generation = workspace.generation();

        assert_eq!(
            close_and_forget_library_source_at(
                registry_path.clone(),
                app_data,
                &source_b.id,
                &workspace,
                &watcher,
            ),
            Err(LibraryError::GrantMismatch)
        );
        assert!(workspace.is_current(&old_epoch));
        assert_eq!(workspace.generation(), generation);
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("registry remains valid")
                .restore_source_id(),
            Some(source_a.id.as_str())
        );
    }

    #[test]
    fn close_and_forget_rejects_same_root_with_a_different_semantic_source_identity() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("persist active membership");
        registry.save_to(&registry_path).expect("save registry");
        let workspace = crate::commands::WorkspaceState::default();
        workspace
            .switch_bound_with_source(active, Some(uuid::Uuid::new_v4()))
            .expect("grant the same root under a different semantic source");
        let watcher = crate::commands::WatcherState::default();
        let epoch = workspace.current().expect("active epoch");
        let generation = workspace.generation();

        assert_eq!(
            close_and_forget_library_source_at(
                registry_path.clone(),
                app_data,
                &source.id,
                &workspace,
                &watcher,
            ),
            Err(LibraryError::GrantMismatch)
        );
        assert!(workspace.is_current(&epoch));
        assert_eq!(workspace.generation(), generation);
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("registry unchanged")
                .restore_source_id(),
            Some(source.id.as_str())
        );
    }

    #[test]
    fn post_close_quarantine_failure_returns_an_inactive_recoverable_source() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("mark restore candidate");
        let managed = crate::engine::db_path_under(&app_data, &active)
            .parent()
            .expect("managed workspace")
            .to_path_buf();
        fs::create_dir_all(&managed).expect("create managed workspace");
        fs::write(managed.join("catdb.db"), b"private index").expect("write private index");
        registry.save_to(&registry_path).expect("save registry");

        let outcome = LibraryStore::new(registry_path.clone())
            .close_and_forget_source_with_cleanup_ops(
                &source.id,
                &active,
                Some(source.engine_source_id()),
                &app_data,
                || Ok(9),
                |_, _| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                |_| panic!("delete must not run after quarantine failure"),
            )
            .expect("a closed workspace returns a typed inactive outcome");

        assert_eq!(
            outcome.status,
            CloseAndForgetStatus::Inactive {
                error: LibraryError::CleanupFailed {
                    reason: super::LibraryCleanupReason::ManagedStateUnavailable,
                },
            }
        );
        assert_eq!(outcome.workspace_generation, 9);
        let retained =
            LibraryRegistry::load_from(&registry_path).expect("reload retained membership");
        assert_eq!(retained.sources().len(), 1);
        assert_eq!(retained.sources()[0].id, source.id);
        assert_eq!(
            fs::read(managed.join("catdb.db")).expect("private state remains retryable"),
            b"private index"
        );
        let snapshot = retained
            .snapshot(&app_data, None)
            .expect("inactive snapshot");
        assert!(!snapshot.sources[0].active);
        assert!(!snapshot.sources[0].restore_candidate);
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("reload inactive registry")
                .reconcile_restore(Some(&root))
                .expect("restart reconciliation"),
            None,
            "a stale session root must not reactivate the retained membership"
        );
    }

    #[test]
    fn close_intent_persist_failure_never_calls_the_authority_revoke() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("mark active");
        registry.save_to(&registry_path).expect("save registry");
        let revoked = Arc::new(AtomicBool::new(false));

        let error = LibraryStore::new(registry_path.clone())
            .close_and_forget_source_with_ops(
                &source.id,
                &active,
                Some(source.engine_source_id()),
                &app_data,
                {
                    let revoked = Arc::clone(&revoked);
                    move || {
                        revoked.store(true, Ordering::Release);
                        Ok(11)
                    }
                },
                |_, _| panic!("no managed state to quarantine"),
                |_| panic!("no managed state to delete"),
                |_, _| Err(LibraryError::PersistFailed),
            )
            .expect_err("intent must be durable before revoke");

        assert_eq!(error, LibraryError::PersistFailed);
        assert!(!revoked.load(Ordering::Acquire));
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("original registry remains")
                .restore_source_id(),
            Some(source.id.as_str())
        );
    }

    #[test]
    fn revoke_failure_with_failed_intent_rollback_reports_rollback_failure() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let active = bound_root(&root);
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        registry
            .mark_active(&source.id, Some(&active))
            .expect("mark active");
        registry.save_to(&registry_path).expect("save registry");
        let persist_calls = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let error = LibraryStore::new(registry_path.clone())
            .close_and_forget_source_with_ops(
                &source.id,
                &active,
                Some(source.engine_source_id()),
                &app_data,
                || Err(LibraryError::OpenFailed),
                |_, _| panic!("no managed state to quarantine"),
                |_| panic!("no managed state to delete"),
                {
                    let persist_calls = Arc::clone(&persist_calls);
                    move |candidate, path| {
                        if persist_calls.fetch_add(1, Ordering::AcqRel) == 0 {
                            candidate.save_to(path)
                        } else {
                            Err(LibraryError::PersistFailed)
                        }
                    }
                },
            )
            .expect_err("rollback failure must be explicit");

        assert_eq!(
            error,
            LibraryError::CleanupFailed {
                reason: super::LibraryCleanupReason::RollbackFailed,
            }
        );
        assert_eq!(persist_calls.load(Ordering::Acquire), 2);
        let persisted =
            LibraryRegistry::load_from(&registry_path).expect("durable close intent remains");
        assert!(persisted.restore_source_id().is_none());
        assert_eq!(
            persisted
                .reconcile_restore(Some(&root))
                .expect("suppressed restart restore"),
            None
        );
    }

    #[test]
    fn managed_cleanup_failure_after_quarantine_stays_pending_and_retries_after_restart() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let document = root.join("keep.md");
        fs::write(&document, b"# unchanged").expect("write user document");
        let before = fs::metadata(&document)
            .expect("document metadata")
            .modified()
            .expect("document mtime");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        let admitted = bound_root(&root);
        let managed = crate::engine::db_path_under(&app_data, &admitted)
            .parent()
            .expect("managed workspace")
            .to_path_buf();
        fs::create_dir_all(managed.join("cache")).expect("create managed workspace");
        fs::write(managed.join("catdb.db"), b"index").expect("write managed index");
        fs::write(managed.join("cache/materialized"), b"cached rows").expect("write cache");
        registry.save_to(&registry_path).expect("save registry");

        let source_path = fs::canonicalize(&root).expect("canonical user root");
        let store = LibraryStore::new(registry_path.clone());
        let status = store
            .forget_source_with_cleanup_ops(
                &source.id,
                None,
                &app_data,
                |from, to| fs::rename(from, to),
                |tombstone| {
                    assert_ne!(tombstone, source_path);
                    assert!(tombstone.starts_with(app_data.join("managed-delete-staging")));
                    fs::remove_file(tombstone.join("catdb.db"))?;
                    fs::remove_file(tombstone.join("cache/materialized"))?;
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                },
            )
            .expect("a post-quarantine failure is a durable pending outcome");

        assert_eq!(
            status,
            super::LibraryCleanupStatus::Pending { source_count: 1 }
        );
        assert!(!managed.exists(), "quarantined state is never reactivated");
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("load cleanup ledger")
                .sources()
                .len(),
            0,
            "membership stays removed while cleanup is pending"
        );
        assert!(matches!(
            LibraryRegistry::load_from(&registry_path)
                .expect("reload removed membership")
                .activation_root(&source.id),
            Err(LibraryError::SourceNotFound)
        ));
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("load durable cleanup ledger")
                .snapshot(&app_data, None)
                .expect("snapshot reports pending")
                .cleanup,
            super::LibraryCleanupStatus::Pending { source_count: 1 }
        );
        assert_eq!(
            fs::read(&document).expect("user document untouched"),
            b"# unchanged"
        );

        let restarted = LibraryStore::new(registry_path);
        let snapshot = restarted
            .read(|registry| registry.snapshot(&app_data, None))
            .expect("the normal next read deterministically finishes the tombstone");
        assert_eq!(snapshot.cleanup, super::LibraryCleanupStatus::Complete);
        assert_eq!(
            fs::read(&document).expect("user document remains untouched"),
            b"# unchanged"
        );
        assert_eq!(
            fs::metadata(&document)
                .expect("document metadata after cleanup retry")
                .modified()
                .expect("document mtime after cleanup retry"),
            before
        );
    }

    #[test]
    fn managed_cleanup_failure_before_quarantine_preserves_membership_and_state() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        let admitted = bound_root(&root);
        let managed = crate::engine::db_path_under(&app_data, &admitted)
            .parent()
            .expect("managed workspace")
            .to_path_buf();
        fs::create_dir_all(&managed).expect("create managed workspace");
        fs::write(managed.join("catdb.db"), b"index").expect("write managed index");
        registry.save_to(&registry_path).expect("save registry");

        let error = LibraryStore::new(registry_path.clone())
            .forget_source_with_cleanup_ops(
                &source.id,
                None,
                &app_data,
                |_, _| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                |_| panic!("delete must not run when the atomic quarantine rename fails"),
            )
            .expect_err("pre-quarantine failure remains an explicit refusal");

        assert_eq!(
            error,
            LibraryError::CleanupFailed {
                reason: super::LibraryCleanupReason::ManagedStateUnavailable
            }
        );
        let loaded = LibraryRegistry::load_from(&registry_path).expect("load unchanged registry");
        assert_eq!(loaded.sources().len(), 1);
        assert_eq!(loaded.sources()[0].id, source.id);
        assert_eq!(
            fs::read(managed.join("catdb.db")).expect("managed state stays addressable"),
            b"index"
        );
    }

    #[test]
    fn restart_recovers_a_crash_between_quarantine_rename_and_registry_publish() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add source");
        let managed = crate::engine::db_path_under(&app_data, &bound_root(&root))
            .parent()
            .expect("managed workspace")
            .to_path_buf();
        fs::create_dir_all(&managed).expect("create managed workspace");
        fs::write(managed.join("catdb.db"), b"index").expect("write index");
        registry
            .save_to(&registry_path)
            .expect("save pre-forget registry");
        let staged = app_data.join("managed-delete-staging").join(&source.id);
        fs::create_dir_all(staged.parent().expect("staging parent")).expect("create staging");
        fs::rename(&managed, &staged).expect("simulate the completed atomic quarantine rename");

        let store = LibraryStore::new(registry_path.clone());
        let status = store
            .retry_pending_cleanup_with(&app_data, |_| {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            })
            .expect("startup reconstructs the pending ledger from the deterministic tombstone");

        assert_eq!(
            status,
            super::LibraryCleanupStatus::Pending { source_count: 1 }
        );
        let recovered =
            LibraryRegistry::load_from(&registry_path).expect("load recovered registry");
        assert!(recovered.sources().is_empty());
        assert!(staged.exists());
        assert!(!managed.exists());

        assert_eq!(
            store
                .retry_pending_cleanup_with(&app_data, |path| fs::remove_dir_all(path))
                .expect("later startup finishes recovered cleanup"),
            super::LibraryCleanupStatus::Complete
        );
        assert!(!staged.exists());
    }

    #[test]
    fn forgetting_one_source_never_addresses_another_sources_managed_state() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let first_root = mkdir(temp.path(), "first");
        let second_root = mkdir(temp.path(), "second");
        let mut registry = LibraryRegistry::empty();
        let first = registry.add_root(&first_root).expect("add first");
        registry.add_root(&second_root).expect("add second");
        let first_managed = crate::engine::db_path_under(&app_data, &bound_root(&first_root))
            .parent()
            .expect("first workspace")
            .to_path_buf();
        let second_managed = crate::engine::db_path_under(&app_data, &bound_root(&second_root))
            .parent()
            .expect("second workspace")
            .to_path_buf();
        fs::create_dir_all(&first_managed).expect("create first workspace");
        fs::create_dir_all(&second_managed).expect("create second workspace");
        fs::write(first_managed.join("catdb.db"), b"first").expect("write first");
        fs::write(second_managed.join("catdb.db"), b"second").expect("write second");
        registry.save_to(&registry_path).expect("save registry");

        let status = LibraryStore::new(registry_path.clone())
            .forget_source(&first.id, None, &app_data)
            .expect("forget first");

        assert_eq!(status, super::LibraryCleanupStatus::Complete);
        assert!(!first_managed.exists());
        assert_eq!(
            fs::read(second_managed.join("catdb.db")).expect("second remains exact"),
            b"second"
        );
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("load one-source registry")
                .sources()
                .len(),
            1
        );
    }

    #[test]
    fn offline_source_without_managed_index_reports_not_created() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let app_data = temp.path().join("app-data");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&root).expect("add source");
        fs::rename(&root, temp.path().join("offline")).expect("take source offline");

        let snapshot = registry.snapshot(&app_data, None).expect("snapshot");
        assert_eq!(
            snapshot.sources[0].index_state,
            LibraryIndexState::NotCreated
        );
    }

    #[test]
    fn activation_resolves_only_an_available_member_and_persistence_replaces_atomically() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let file = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add member");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let canonical_root = bound_root(&canonical);

        assert_eq!(
            registry
                .activation_path(&source.id)
                .expect("available member activates"),
            canonical
        );
        registry.save_to(&file).expect("initial save");
        registry
            .mark_active(&source.id, Some(&canonical_root))
            .expect("mark active");
        registry.save_to(&file).expect("replace existing registry");
        assert_eq!(
            LibraryRegistry::load_from(&file)
                .expect("load replacement")
                .restore_source_id(),
            Some(source.id.as_str())
        );

        fs::remove_dir(&root).expect("make source unavailable");
        assert!(matches!(
            registry.activation_path(&source.id),
            Err(LibraryError::SourceUnavailable)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_substitution_invalidates_activation_and_the_restore_candidate() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let target = mkdir(temp.path(), "replacement");
        let app_data = temp.path().join("app-data");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add member");
        let admitted = fs::canonicalize(&root).expect("canonical admitted root");
        let admitted_root = bound_root(&admitted);
        registry
            .mark_active(&source.id, Some(&admitted_root))
            .expect("record restore candidate");

        fs::remove_dir(&root).expect("remove admitted directory");
        symlink(&target, &root).expect("replace admitted path with symlink");

        assert!(
            matches!(
                registry.source_state(&source.id),
                Ok(LibrarySourceState::Unreadable)
            ),
            "a path now resolving to a different canonical root is unavailable"
        );
        assert!(matches!(
            registry.activation_path(&source.id),
            Err(LibraryError::SourceUnavailable)
        ));
        assert_eq!(
            registry
                .reconcile_restore(Some(&admitted))
                .expect("restore refusal is not registry corruption"),
            None,
            "a saved display path cannot restore a substituted source"
        );

        let replacement = fs::canonicalize(&target).expect("canonical replacement");
        assert!(
            !crate::engine::db_path_under(&app_data, &bound_root(&replacement)).exists(),
            "refusing substitution must not create an index for the target"
        );
        assert_eq!(
            registry.restore_source_id(),
            Some(source.id.as_str()),
            "refusal does not silently rewrite the recorded restore identity"
        );
    }

    #[test]
    fn independent_registry_handles_serialize_the_full_read_modify_write_transaction() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("app-data/library.json");
        let root_a = mkdir(temp.path(), "source-a");
        let root_b = mkdir(temp.path(), "source-b");
        let first = LibraryStore::new(registry_path.clone());
        let second = LibraryStore::new(registry_path.clone());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let first_thread = thread::spawn(move || {
            first.update(|registry| {
                registry.add_root(&root_a)?;
                entered_tx.send(()).expect("signal first mutation");
                release_rx.recv().expect("release first mutation");
                Ok(())
            })
        });
        entered_rx.recv().expect("first mutation holds the lease");

        let (second_done_tx, second_done_rx) = mpsc::channel();
        let second_thread = thread::spawn(move || {
            let result = second.update(|registry| {
                registry.add_root(&root_b)?;
                Ok(())
            });
            second_done_tx.send(()).expect("signal second mutation");
            result
        });

        assert!(
            second_done_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "a process-equivalent handle must block before loading stale state"
        );
        release_tx.send(()).expect("release first mutation");
        first_thread
            .join()
            .expect("join first mutation")
            .expect("first mutation succeeds");
        second_thread
            .join()
            .expect("join second mutation")
            .expect("second mutation succeeds");

        let source_count = LibraryStore::new(registry_path)
            .read(|registry| Ok(registry.sources().len()))
            .expect("read serialized result");
        assert_eq!(source_count, 2, "neither membership update is lost");
    }

    #[test]
    fn registry_lock_failure_is_closed_before_reading_or_mutating_state() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("app-data/library.json");
        let lock_path = temp.path().join("app-data/library.lock");
        fs::create_dir_all(&lock_path).expect("occupy sidecar path with a directory");

        assert!(matches!(
            LibraryStore::new(registry_path).read(|_| Ok(())),
            Err(LibraryError::PersistFailed)
        ));
    }

    #[test]
    fn absent_registry_always_requires_native_repick_even_with_legacy_index_shape() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "legacy-notes");
        let canonical = fs::canonicalize(&root).expect("canonical legacy root");
        let store = LibraryStore::new(registry_path.clone());
        let legacy_workspace = legacy_v1_workspace_path(&app_data, &canonical);
        create_legacy_catdb_index(&legacy_workspace.join("catdb.db"));

        assert_eq!(
            store.reconcile_restore_or_migrate(Some(&canonical), &app_data),
            Err(LibraryError::NeedsNativeRepick)
        );
        assert!(
            !registry_path.exists(),
            "session text and a lookalike index create no registry"
        );
        assert!(legacy_workspace.join("catdb.db").is_file());
    }

    #[test]
    fn existing_registry_has_precedence_over_a_legacy_session_string() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let admitted = mkdir(temp.path(), "admitted");
        let outsider = mkdir(temp.path(), "legacy-outsider");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&admitted).expect("add admitted source");
        registry.save_to(&registry_path).expect("persist registry");
        let before = fs::read(&registry_path).expect("read registry before restore");

        let restored = LibraryStore::new(registry_path.clone())
            .reconcile_restore_or_migrate(Some(&outsider), &app_data)
            .expect("outsider is a clean refusal");

        assert!(restored.is_none());
        assert_eq!(
            fs::read(&registry_path).expect("read registry after refusal"),
            before,
            "an existing registry is never overwritten from a session string"
        );
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("load unchanged registry")
                .sources()
                .len(),
            1
        );
    }

    #[test]
    fn unavailable_or_corrupt_legacy_restore_fails_without_publishing_membership() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let missing = temp.path().join("missing");
        let store = LibraryStore::new(registry_path.clone());

        assert_eq!(
            store.reconcile_restore_or_migrate(Some(&missing), &app_data),
            Err(LibraryError::NeedsNativeRepick)
        );
        assert!(
            !registry_path.exists(),
            "a refused migration does not consume or fabricate registry state"
        );

        fs::create_dir_all(&app_data).expect("create app data");
        fs::write(&registry_path, b"{corrupt").expect("write corrupt registry");
        let available = mkdir(temp.path(), "available");
        assert!(matches!(
            store.reconcile_restore_or_migrate(Some(&available), &app_data),
            Err(LibraryError::CorruptRegistry)
        ));
        assert_eq!(
            fs::read(&registry_path).expect("corrupt bytes remain"),
            b"{corrupt",
            "corruption is never replaced by a session-derived registry"
        );
    }

    #[test]
    fn command_boundary_refuses_existing_outsider_without_backend_legacy_evidence() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let outsider = mkdir(temp.path(), "existing-outsider");

        assert_eq!(
            super::reconcile_library_restore_at(
                registry_path.clone(),
                app_data,
                Some(outsider.to_string_lossy().into_owned())
            ),
            Err(LibraryError::NeedsNativeRepick)
        );
        assert!(
            !registry_path.exists(),
            "the command boundary creates no membership or registry authority"
        );
    }

    #[test]
    fn canonical_outsider_with_renderer_reproducible_legacy_tables_needs_native_repick() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        fs::create_dir(&app_data).expect("create app data");
        let registry_path = app_data.join("library.json");
        let outsider = mkdir(temp.path(), "existing-outsider");
        let canonical = fs::canonicalize(&outsider).expect("canonical outsider");
        create_legacy_catdb_index(&super::v1_workspace_dir(&app_data, &canonical).join("catdb.db"));

        assert_eq!(
            super::reconcile_library_restore_at(
                registry_path.clone(),
                app_data.clone(),
                Some(canonical.to_string_lossy().into_owned())
            ),
            Err(LibraryError::NeedsNativeRepick)
        );
        assert!(
            !registry_path.exists(),
            "renderer-reproducible SQLite names never publish membership"
        );
        assert!(
            !crate::engine::db_path_under(
                &app_data,
                &SourceRoot::capture(canonical).expect("capture outsider only for assertion")
            )
            .exists(),
            "refusal creates no identity-keyed managed workspace"
        );
    }

    #[test]
    fn isolated_e2e_setup_admits_only_the_backend_captured_corpus_before_remote_members() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let corpus = mkdir(temp.path(), "fixture-corpus");
        let outsider = mkdir(temp.path(), "outsider");
        let store = LibraryStore::new(registry_path.clone());
        let expected_engine_source_id = uuid::Uuid::from_u128(0xe2e);

        let admitted = store
            .replace_with_e2e_local_membership(&corpus, expected_engine_source_id)
            .expect("backend captures the runner-owned corpus");
        assert_eq!(
            admitted.engine_source_id(),
            expected_engine_source_id,
            "the pre-reviewed workspace and the E2E Library must share one source identity"
        );
        let drive: crate::cloud_accounts::CloudSourceRecord =
            serde_json::from_value(serde_json::json!({
                "id": "d".repeat(64),
                "provider": "google-drive",
                "label": "Google Drive",
                "enabled": true,
                "credential_reference": "mdreader-e2e-google-drive",
                "config": {
                    "kind": "google-drive",
                    "selections": [{
                        "id": "drive-fixture-note",
                        "resource_key": null,
                        "folder": false
                    }]
                },
                "contract": {
                    "connector_version": "google-drive/v1",
                    "capabilities": {
                        "read": true,
                        "write": false,
                        "search": true,
                        "watch": false,
                        "open_original": true,
                        "download": true
                    },
                    "mapping_version": crate::cloud_accounts::CLOUD_DOCUMENT_MAPPING_VERSION,
                    "index_policy": "read-through"
                }
            }))
            .expect("valid E2E Drive record");
        store
            .update_cloud(|sources| {
                sources.push(drive);
                Ok(())
            })
            .expect("add E2E Drive member after local admission");

        let registry =
            LibraryRegistry::load_from(&registry_path).expect("load composed E2E Library");
        assert_eq!(registry.sources().len(), 1);
        assert_eq!(registry.cloud_source_count(), 1);
        assert_eq!(registry.restore_source_id(), Some(admitted.id.as_str()));

        let restored = store
            .reconcile_restore_or_migrate(Some(&corpus), &app_data)
            .expect("reconcile exact admitted corpus")
            .expect("exact admitted corpus restores");
        assert_eq!(restored.id, admitted.id);
        assert_eq!(
            store
                .reconcile_restore_or_migrate(Some(&outsider), &app_data)
                .expect("outsider is a typed refusal"),
            None
        );
    }

    #[test]
    fn library_transport_never_serializes_free_form_native_or_secret_context() {
        let sentinel =
            "/Users/private-user/secret.md token=blue-lantern postgres://name:pass@host/db $1";
        let error = LibraryRegistry::empty()
            .activation_root(sentinel)
            .expect_err("renderer-supplied source id is not a member");
        let availability: super::LibraryAvailability =
            classify_probe_error(io::Error::new(io::ErrorKind::PermissionDenied, sentinel)).into();

        for rendered in [
            format!("{error:?}"),
            error.to_string(),
            serde_json::to_string(&error).expect("serialize library error"),
            format!("{availability:?}"),
            serde_json::to_string(&availability).expect("serialize availability"),
        ] {
            assert!(
                !rendered.contains(sentinel),
                "transport representation exposed sentinel: {rendered}"
            );
            assert!(!rendered.contains("postgres://"));
            assert!(!rendered.contains("blue-lantern"));
            assert!(!rendered.contains("secret.md"));
        }
    }

    #[test]
    fn retired_connection_metadata_round_trips_without_runtime_exposure() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("library.json");
        let source_id = "a".repeat(64);
        let remote_source_id = "00000000-0000-0000-0000-000000000011";
        let body = serde_json::json!({
            "version": LIBRARY_VERSION,
            "active_source_id": null,
            "sources": [{
                "kind": "connected-catdb",
                "id": source_id,
                "label": "Research CatDB",
                "server_origin": "https://catdb.example.com/",
                "credential_reference": "credential-reference-1",
                "deployment_id": "00000000-0000-0000-0000-000000000002",
                "workspace_id": "00000000-0000-0000-0000-000000000003",
                "catalog_revision_id": "00000000-0000-0000-0000-000000000004",
                "default_schema_version_id": "00000000-0000-0000-0000-000000000005",
                "permitted_sources": [{
                    "source_id": remote_source_id,
                    "source_version_id": "00000000-0000-0000-0000-000000000012",
                    "runtime_bound": true,
                    "connector_id": "postgres-primary"
                }],
                "selected_source_ids": [remote_source_id],
                "contract": {
                    "connector_version": "connected-catdb/v1",
                    "capabilities": {
                        "read": true,
                        "write": false,
                        "search": true,
                        "watch": false,
                        "open_original": false,
                        "download": false
                    },
                    "mapping_version": "00000000-0000-0000-0000-000000000004",
                    "index_policy": "read-through"
                }
            }],
            "cleanup_pending": []
        });
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&body).expect("serialize fixture"),
        )
        .expect("write fixture");

        let registry =
            LibraryRegistry::load_from(&registry_path).expect("load Connected CatDB membership");
        let snapshot = registry
            .snapshot(temp.path(), None)
            .expect("snapshot Connected CatDB membership");
        assert!(snapshot.sources.is_empty());
        let rendered = serde_json::to_value(&snapshot).expect("serialize local-only snapshot");
        assert!(rendered.get("connected_catdb").is_none());
        assert!(rendered.get("connected_catdb_rediscovery_required").is_none());
        registry
            .save_to(&registry_path)
            .expect("persist membership");
        let persisted = fs::read_to_string(&registry_path).expect("read persisted registry");

        let retained: serde_json::Value =
            serde_json::from_str(&persisted).expect("decode retained compatibility metadata");
        assert_eq!(retained["sources"], body["sources"]);
        assert!(persisted.contains("\"kind\": \"connected-catdb\""));
        assert!(persisted.contains("\"selected_source_ids\""));
        assert!(!persisted.contains("bearer"));
        assert!(!persisted.contains("native-secret"));
    }

    #[test]
    fn persisted_connected_catdb_rejects_distinct_sources_with_one_version_authority() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("library.json");
        let first_source_id = "00000000-0000-0000-0000-000000000011";
        let second_source_id = "00000000-0000-0000-0000-000000000013";
        let shared_version_id = "00000000-0000-0000-0000-000000000012";
        let body = serde_json::json!({
            "version": LIBRARY_VERSION,
            "active_source_id": null,
            "sources": [{
                "kind": "connected-catdb",
                "id": "a".repeat(64),
                "label": "Research CatDB",
                "server_origin": "https://catdb.example.com/",
                "credential_reference": "credential-reference-1",
                "deployment_id": "00000000-0000-0000-0000-000000000002",
                "workspace_id": "00000000-0000-0000-0000-000000000003",
                "catalog_revision_id": "00000000-0000-0000-0000-000000000004",
                "default_schema_version_id": "00000000-0000-0000-0000-000000000005",
                "permitted_sources": [{
                    "source_id": first_source_id,
                    "source_version_id": shared_version_id,
                    "runtime_bound": true,
                    "connector_id": "postgres-primary"
                }, {
                    "source_id": second_source_id,
                    "source_version_id": shared_version_id,
                    "runtime_bound": true,
                    "connector_id": "snowflake-secondary"
                }],
                "selected_source_ids": [first_source_id, second_source_id],
                "contract": {
                    "connector_version": "connected-catdb/v1",
                    "capabilities": {
                        "read": true,
                        "write": false,
                        "search": true,
                        "watch": false,
                        "open_original": false,
                        "download": false
                    },
                    "mapping_version": "00000000-0000-0000-0000-000000000004",
                    "index_policy": "read-through"
                }
            }],
            "connected_catdb_rediscovery_required": false,
            "cleanup_pending": []
        });
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&body).expect("serialize fixture"),
        )
        .expect("write fixture");

        assert!(matches!(
            LibraryRegistry::load_from(&registry_path),
            Err(LibraryError::CorruptRegistry)
        ));
    }

    #[test]
    fn version_seven_quarantines_only_unpinned_connected_catdb_membership() {
        let temp = tempdir().expect("tempdir");
        let registry_path = temp.path().join("library.json");
        let root = mkdir(temp.path(), "local-notes");
        let mut current = LibraryRegistry::empty();
        let local = current.add_root(&root).expect("admit local source");
        current
            .save_to(&registry_path)
            .expect("persist current local registry");
        let mut body: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read current registry"))
                .expect("parse current registry");
        body["version"] = serde_json::json!(7);
        body.as_object_mut()
            .expect("registry object")
            .remove("connected_catdb_rediscovery_required");
        body.as_object_mut()
            .expect("registry object")
            .remove("restore_suppressed_source_ids");
        body["sources"]
            .as_array_mut()
            .expect("source array")
            .push(serde_json::json!({
                "kind": "connected-catdb",
                "id": "a".repeat(64),
                "label": "Legacy CatDB",
                "server_origin": "https://catdb.example.com/",
                "credential_reference": "credential-reference-1",
                "deployment_id": "00000000-0000-0000-0000-000000000002",
                "workspace_id": "00000000-0000-0000-0000-000000000003",
                "catalog_revision_id": "00000000-0000-0000-0000-000000000004",
                "permitted_sources": [],
                "selected_source_ids": [],
                "contract": {
                    "connector_version": "connected-catdb/v1",
                    "capabilities": {
                        "read": true,
                        "write": false,
                        "search": true,
                        "watch": false,
                        "open_original": false,
                        "download": false
                    },
                    "mapping_version": "00000000-0000-0000-0000-000000000004",
                    "index_policy": "read-through"
                }
            }));
        body["active_source_id"] = serde_json::json!("a".repeat(64));
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&body).expect("serialize legacy fixture"),
        )
        .expect("write fixture");

        let migrated = LibraryRegistry::load_from(&registry_path)
            .expect("quarantine only the unverifiable membership");
        assert_eq!(migrated.sources().len(), 1);
        assert_eq!(migrated.sources()[0].id, local.id);
        assert!(migrated
            .sources
            .iter()
            .all(|source| source.connected_catdb().is_none()));
        assert_eq!(migrated.restore_source_id(), None);
        let snapshot = migrated
            .snapshot(temp.path(), None)
            .expect("local Library remains readable");
        assert_eq!(snapshot.sources.len(), 1);
        assert!(serde_json::to_value(&snapshot)
            .expect("snapshot JSON")
            .get("connected_catdb")
            .is_none());

        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(registry_path).expect("read migrated registry"))
                .expect("parse migrated registry");
        assert_eq!(persisted["version"], serde_json::json!(LIBRARY_VERSION));
        assert_eq!(
            persisted["connected_catdb_rediscovery_required"],
            serde_json::json!(true)
        );
        assert_eq!(
            persisted["sources"].as_array().expect("source array").len(),
            1
        );
    }
}
