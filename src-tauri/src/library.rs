//! Backend-owned Library membership.
//!
//! The active editing engine remains deliberately single-root. Query execution
//! takes one immutable snapshot of this registry and may read every enabled
//! local or cloud member together. Paths are canonicalized before admission and
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

use crate::cloud_accounts::{validate_cloud_source_records, CloudSourceError, CloudSourceRecord};
use crate::file_lock::AdvisoryFileLock;
use mdreader_ingest::{SourceRoot, SourceRootIdentity};

pub(crate) const LIBRARY_VERSION: u32 = 6;
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

/// Immutable source membership captured under the Library registry lease.
#[derive(Debug, Clone)]
pub(crate) struct LibraryQuerySnapshot {
    pub local_sources: Vec<LocalFolderSource>,
    pub cloud_sources: Vec<CloudSourceRecord>,
    pub active_source_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct PickedLibrarySource {
    pub source: Option<LibrarySourceDto>,
    pub added: bool,
}

#[derive(Default)]
pub(crate) struct LibraryState(Mutex<()>);

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

    pub(crate) fn connector_version(&self) -> &str {
        &self.connector_version
    }

    pub(crate) fn permits_read_only(&self) -> bool {
        self.capabilities.read
            && !self.capabilities.write
            && self.index_policy == LibraryIndexPolicy::ReadThrough
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
    /// Stable CatDB source identity owned and persisted by this Library
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
pub(crate) enum LibrarySource {
    LocalFolder(LocalFolderSource),
    GoogleDrive(CloudSourceRecord),
    Dropbox(CloudSourceRecord),
}

impl LibrarySource {
    fn id(&self) -> &str {
        match self {
            Self::LocalFolder(source) => &source.id,
            Self::GoogleDrive(source) | Self::Dropbox(source) => source.id(),
        }
    }

    fn local(&self) -> Option<&LocalFolderSource> {
        match self {
            Self::LocalFolder(source) => Some(source),
            Self::GoogleDrive(_) | Self::Dropbox(_) => None,
        }
    }

    fn local_mut(&mut self) -> Option<&mut LocalFolderSource> {
        match self {
            Self::LocalFolder(source) => Some(source),
            Self::GoogleDrive(_) | Self::Dropbox(_) => None,
        }
    }

    fn cloud(&self) -> Option<&CloudSourceRecord> {
        match self {
            Self::GoogleDrive(source) | Self::Dropbox(source) => Some(source),
            Self::LocalFolder(_) => None,
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibrary {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LibrarySource>,
    #[serde(default)]
    cleanup_pending: Vec<PendingCleanup>,
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
    sources: Vec<LibrarySource>,
    cleanup_pending: Vec<PendingCleanup>,
}

impl LibraryRegistry {
    pub(crate) fn empty() -> Self {
        Self {
            active_source_id: None,
            sources: Vec::new(),
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
                        let db = mdreader_ingest::workspace_index_path_for_stable_identity(
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
        if version.version != LIBRARY_VERSION {
            return Err(LibraryError::UnsupportedVersion {
                found: version.version,
                expected: LIBRARY_VERSION,
            });
        }
        let stored: StoredLibrary =
            serde_json::from_slice(&bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        Self::validate_membership(stored.active_source_id.as_deref(), &stored.sources)?;
        Self::validate_pending_cleanup(&stored.cleanup_pending, &stored.sources)?;
        Ok(Self {
            active_source_id: stored.active_source_id,
            sources: stored.sources,
            cleanup_pending: stored.cleanup_pending,
        })
    }

    fn migrate_v5(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let stored: StoredLibrary =
            serde_json::from_slice(bytes).map_err(|_| LibraryError::CorruptRegistry)?;
        let mut migrated = Self {
            active_source_id: stored.active_source_id,
            sources: stored.sources,
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
            // V1 stores only historical paths. They are recovery hints, not
            // live membership; the native picker must explicitly re-admit the
            // filesystem object. Legacy managed bytes were quarantined above
            // and are never inherited by the object currently at that path.
            sources: Vec::new(),
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
            sources,
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
            sources: stored
                .sources
                .into_iter()
                .map(LibrarySource::LocalFolder)
                .collect(),
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
            sources,
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
        Self::validate_pending_cleanup(&self.cleanup_pending, &self.sources)?;
        let parent = path.parent().ok_or(LibraryError::PersistFailed)?;
        fs::create_dir_all(parent).map_err(|_| LibraryError::PersistFailed)?;
        let body = serde_json::to_vec_pretty(&StoredLibrary {
            version: LIBRARY_VERSION,
            active_source_id: self.active_source_id.clone(),
            sources: self.sources.clone(),
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

fn recover_v2_engine_source_id(
    app_data: &Path,
    source: &LibrarySourceV2,
) -> Result<Option<uuid::Uuid>, LibraryError> {
    recover_v2_engine_source_id_with_hooks(app_data, source, |_| {}, |_| {})
}

fn recover_v2_engine_source_id_with_hooks(
    app_data: &Path,
    source: &LibrarySourceV2,
    before_open: impl FnOnce(&Path),
    after_open: impl FnOnce(&Path),
) -> Result<Option<uuid::Uuid>, LibraryError> {
    let identity = source
        .identity
        .as_ref()
        .ok_or(LibraryError::CorruptRegistry)?;
    let source_path = source.path.restore()?;
    let old_workspace = mdreader_ingest::legacy_workspace_index_path_for_path_identity(
        app_data,
        &source_path,
        identity,
    )
    .parent()
    .ok_or(LibraryError::PersistFailed)?
    .to_path_buf();
    let new_workspace =
        mdreader_ingest::workspace_index_path_for_stable_identity(app_data, identity)
            .parent()
            .ok_or(LibraryError::PersistFailed)?
            .to_path_buf();

    let old_present = managed_directory_present(&old_workspace)?;
    let new_present = managed_directory_present(&new_workspace)?;
    let workspace = match (old_present, new_present) {
        (true, false) => old_workspace,
        (false, true) => new_workspace,
        (false, false) => return Ok(None),
        (true, true) => {
            return Err(LibraryError::CleanupFailed {
                reason: LibraryCleanupReason::ManagedStateUnavailable,
            })
        }
    };
    let db_path = workspace.join("catdb.db");
    let db_metadata = match fs::symlink_metadata(&db_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(LibraryError::CorruptRegistry)
        }
        Err(_) => return Err(LibraryError::CorruptRegistry),
    };
    if db_metadata.file_type().is_symlink() || !db_metadata.is_file() {
        return Err(LibraryError::CorruptRegistry);
    }
    ensure_no_sqlite_recovery_sidecars(&db_path)?;

    // Hold the exact checked file open while SQLite opens a platform-specific
    // binding to it. A final name-to-handle comparison also rejects a
    // one-directional replacement, while the bound connection prevents an ABA
    // replacement from changing which database is read.
    let open_path = fs::canonicalize(&db_path).map_err(|_| LibraryError::CorruptRegistry)?;
    let checked_file =
        same_file::Handle::from_path(&open_path).map_err(|_| LibraryError::CorruptRegistry)?;
    before_open(&db_path);
    let (connection, _connection_guard) = open_checked_v2_connection(&open_path, &checked_file)?;
    after_open(&db_path);
    let opened_file =
        same_file::Handle::from_path(&open_path).map_err(|_| LibraryError::CorruptRegistry)?;
    if checked_file != opened_file {
        return Err(LibraryError::CorruptRegistry);
    }
    ensure_no_sqlite_recovery_sidecars(&db_path)?;
    let inspected = catdb_storage_sqlite::inspect_single_source_catalog_for_adoption(&connection)
        .map_err(|_| LibraryError::CorruptRegistry)?;
    // Registry V2 was introduced against CatDB v7. Earlier schemas were never
    // V2 producers; later supported schemas remain migration-owned and are
    // accepted only after storage validates their exact contract.
    const EARLIEST_V2_CATDB_SCHEMA_VERSION: i64 = 7;
    if inspected.schema_version() < EARLIEST_V2_CATDB_SCHEMA_VERSION {
        return Err(LibraryError::CorruptRegistry);
    }
    let expected_root = source_path.display().to_string();
    if inspected.root_path() != expected_root {
        return Err(LibraryError::CorruptRegistry);
    }
    Ok(Some(inspected.source_id()))
}

fn ensure_no_sqlite_recovery_sidecars(db_path: &Path) -> Result<(), LibraryError> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = db_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match fs::symlink_metadata(PathBuf::from(sidecar)) {
            Ok(_) => return Err(LibraryError::CorruptRegistry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(LibraryError::CorruptRegistry),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn open_checked_v2_connection(
    _open_path: &Path,
    checked_file: &same_file::Handle,
) -> Result<(rusqlite::Connection, ()), LibraryError> {
    use std::os::fd::AsRawFd;

    // SQLite opens the descriptor link, so the connection is bound to the
    // already-checked inode even if the original filename is replaced.
    let descriptor_path = format!(
        "file:/dev/fd/{}?mode=ro&immutable=1",
        checked_file.as_raw_fd()
    );
    let connection = rusqlite::Connection::open_with_flags(
        descriptor_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|_| LibraryError::CorruptRegistry)?;
    Ok((connection, ()))
}

#[cfg(windows)]
fn open_checked_v2_connection(
    open_path: &Path,
    checked_file: &same_file::Handle,
) -> Result<(rusqlite::Connection, fs::File), LibraryError> {
    use std::os::windows::fs::OpenOptionsExt;

    // Excluding FILE_SHARE_DELETE keeps the name attached to this file from
    // the identity comparison through SQLite's open. SQLite remains free to
    // acquire its ordinary read/write sharing and locking handles.
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    let path_lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(open_path)
        .map_err(|_| LibraryError::CorruptRegistry)?;
    let locked_file = path_lock
        .try_clone()
        .and_then(same_file::Handle::from_file)
        .map_err(|_| LibraryError::CorruptRegistry)?;
    if *checked_file != locked_file {
        return Err(LibraryError::CorruptRegistry);
    }
    let connection = rusqlite::Connection::open_with_flags(
        open_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| LibraryError::CorruptRegistry)?;
    Ok((connection, path_lock))
}

#[cfg(not(any(unix, windows)))]
fn open_checked_v2_connection(
    _open_path: &Path,
    _checked_file: &same_file::Handle,
) -> Result<(rusqlite::Connection, ()), LibraryError> {
    Err(LibraryError::CorruptRegistry)
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
        let old = mdreader_ingest::legacy_workspace_index_path_for_path_identity(
            app_data, &path, identity,
        )
        .parent()
        .ok_or(LibraryError::PersistFailed)?
        .to_path_buf();
        let new = mdreader_ingest::workspace_index_path_for_stable_identity(app_data, identity)
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
    #[cfg(test)]
    fail_next_cloud_persist: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl LibraryStore {
    pub(crate) fn new(registry_path: PathBuf) -> Self {
        Self {
            registry_path,
            #[cfg(test)]
            fail_next_cloud_persist: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
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

    /// Capture local and enabled cloud membership under one registry lease.
    ///
    /// Credential material is deliberately absent. The caller first validates
    /// `EntityQueryRequest.source_ids` against this snapshot and only then
    /// resolves credentials for the selected cloud members.
    pub(crate) fn read_query_snapshot(&self) -> Result<LibraryQuerySnapshot, LibraryError> {
        self.read(|registry| {
            Ok(LibraryQuerySnapshot {
                local_sources: registry.local_sources().cloned().collect(),
                cloud_sources: registry
                    .sources
                    .iter()
                    .filter_map(LibrarySource::cloud)
                    .filter(|source| source.enabled())
                    .cloned()
                    .collect(),
                active_source_id: registry.active_source_id.clone(),
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

    pub(crate) fn read_cloud<T>(
        &self,
        operation: impl FnOnce(&[CloudSourceRecord]) -> Result<T, CloudSourceError>,
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
        let sources = registry
            .sources
            .iter()
            .filter_map(LibrarySource::cloud)
            .cloned()
            .collect::<Vec<_>>();
        operation(&sources)
    }

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
        registry
            .sources
            .retain(|source| matches!(source, LibrarySource::LocalFolder(_)));
        registry
            .sources
            .extend(cloud_sources.into_iter().map(LibrarySource::from_cloud));
        #[cfg(test)]
        if self
            .fail_next_cloud_persist
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(CloudSourceError::PersistFailed);
        }
        registry
            .save_to(&self.registry_path)
            .map_err(map_library_cloud_error)?;
        Ok(result)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_cloud_persist(&self) {
        self.fail_next_cloud_persist
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
        let managed = mdreader_ingest::workspace_index_path_for_stable_identity(app_data, identity)
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
                })
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
                })
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
                    })
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
                mdreader_ingest::workspace_index_path_for_stable_identity(app_data, identity);
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
                    })
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

pub(crate) fn map_library_cloud_error(error: LibraryError) -> CloudSourceError {
    match error {
        LibraryError::CorruptRegistry => CloudSourceError::CorruptRegistry,
        LibraryError::UnsupportedVersion { found, expected } => {
            CloudSourceError::UnsupportedVersion { found, expected }
        }
        _ => CloudSourceError::PersistFailed,
    }
}

fn registry_path(app: &AppHandle) -> Result<(PathBuf, PathBuf), LibraryError> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|_| LibraryError::PersistFailed)?;
    Ok((app_data.join("library.json"), app_data))
}

fn active_root(workspace: &crate::commands::WorkspaceState) -> Option<SourceRoot> {
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
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let active = active_root(&workspace);
    LibraryStore::new(path).read(|registry| registry.snapshot(&app_data, active.as_ref()))
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
    let _guard = lock_library(&state)?;
    let (path, _) = registry_path(&app)?;
    LibraryStore::new(path).read(|registry| {
        let native_root = registry.activation_root(&source_id)?;
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
    let _guard = lock_library(&state)?;
    let (path, _) = registry_path(&app)?;
    LibraryStore::new(path).read(|registry| {
        let native_root = registry.activation_root(&source_id)?;
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
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let active = active_root(&workspace);
    LibraryStore::new(path).update(|registry| {
        registry.mark_active(&source_id, active.as_ref())?;
        registry.snapshot(&app_data, active.as_ref())
    })
}

/// Forget one inactive membership and exactly its private CatDB-managed state.
/// The user's source directory is never addressed by cleanup.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn forget_library_source(
    app: AppHandle,
    source_id: String,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
    state: tauri::State<'_, LibraryState>,
) -> Result<LibrarySnapshot, LibraryError> {
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let active = active_root(&workspace);
    let store = LibraryStore::new(path);
    store.forget_source(&source_id, active.as_ref(), &app_data)?;
    store.read(|registry| registry.snapshot(&app_data, active.as_ref()))
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
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use catdb_storage::CatalogStorage;
    use tempfile::tempdir;

    use super::{
        classify_probe_error, LibraryError, LibraryIndexState, LibraryRegistry, LibrarySourceState,
        LibraryStore, NativePath, LIBRARY_VERSION,
    };
    use mdreader_ingest::SourceRoot;

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
        let old = mdreader_ingest::legacy_workspace_index_path_for_path_identity(
            app_data,
            &root.path,
            &root.identity,
        );
        let new =
            mdreader_ingest::workspace_index_path_for_stable_identity(app_data, &root.identity);
        (old, new)
    }

    fn create_authentic_v2_index(path: &Path, root: &SourceRoot) -> uuid::Uuid {
        create_authentic_v2_index_with_schema(path, root).0
    }

    fn create_authentic_v2_index_with_schema(path: &Path, root: &SourceRoot) -> (uuid::Uuid, i64) {
        let engine = catdb_engine::EmbeddedCatDb::open(
            catdb_engine::EngineConfig::new(
                path,
                std::sync::Arc::new(catdb_embed::HashEmbedder::new()),
            )
            .with_domain(mdreader_ingest::registration()),
        )
        .expect("open authentic V2 managed index");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(engine.import(&root.path, catdb_engine::ImportOptions::default()))
            .expect("import authentic V2 managed index");
        let sources = runtime
            .block_on(engine.store().list_sources())
            .expect("list authentic V2 source");
        assert_eq!(sources.len(), 1);
        let schema_version = engine
            .store()
            .schema_version()
            .expect("read authentic schema version");
        (sources[0].source_id, schema_version)
    }

    fn create_genuine_v7_index(path: &Path, root: &SourceRoot) -> uuid::Uuid {
        fs::create_dir_all(path.parent().expect("legacy index parent"))
            .expect("create legacy workspace");
        catdb_storage_sqlite::migrate_database_to_version_for_tests(path, 7)
            .expect("migrate through genuine v7 DDL");
        let source_id = uuid::Uuid::new_v4();
        let connection = rusqlite::Connection::open(path).expect("open genuine v7 catalog");
        let schema_version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .expect("read genuine v7 schema version");
        assert_eq!(schema_version, 7);
        connection
            .execute(
                "INSERT INTO sources(
                   source_id, root_path, enabled, watch, include_globs,
                   exclude_globs, max_file_size, respect_gitignore, embed_policy
                 ) VALUES (?1, ?2, 1, 0, '[]', '[]', 1, 1, 'auto')",
                (source_id.to_string(), root.path.display().to_string()),
            )
            .expect("insert genuine v7 source");
        drop(connection);
        assert_no_sqlite_sidecars(path);
        source_id
    }

    fn mutate_catalog_without_leaving_sidecars(path: &Path, sql: &str) {
        let connection = rusqlite::Connection::open(path).expect("open catalog for mutation");
        connection
            .execute_batch(sql)
            .expect("apply controlled catalog mutation");
        let _: (i64, i64, i64) = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("checkpoint controlled mutation");
        let journal_mode = connection
            .query_row("PRAGMA journal_mode=DELETE", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("leave controlled mutation in the main database");
        assert_eq!(journal_mode, "delete");
        drop(connection);
        assert_no_sqlite_sidecars(path);
    }

    fn assert_no_sqlite_sidecars(path: &Path) {
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            assert!(
                !PathBuf::from(sidecar).exists(),
                "fixture must not leave a {suffix} sidecar"
            );
        }
    }

    #[test]
    fn version_two_recovers_an_authentic_v10_catalog_without_mutating_it() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
        let (old_db, _new_db) = v2_managed_paths(&app_data, &root);
        let (source_id, schema_version) = create_authentic_v2_index_with_schema(&old_db, &root);
        assert_eq!(schema_version, 10, "fixture must traverse the real v10 DDL");
        let before = fs::read(&old_db).expect("read index before legacy inspection");
        assert_no_sqlite_sidecars(&old_db);

        let recovered = super::recover_v2_engine_source_id(&app_data, &v2_source(&root));
        let after = fs::read(&old_db).expect("read index after legacy inspection");

        assert_eq!(
            after, before,
            "legacy inspection must never migrate or otherwise write the catalog"
        );
        assert_eq!(
            recovered,
            Ok(Some(source_id)),
            "a complete current CatDB catalog remains a valid V2 recovery authority"
        );
    }

    #[test]
    fn version_two_preserves_genuine_v7_catalog_compatibility_without_mutation() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        let (old_db, _new_db) = v2_managed_paths(&app_data, &root);
        let source_id = create_genuine_v7_index(&old_db, &root);
        let before = fs::read(&old_db).expect("read v7 index before legacy inspection");

        let recovered = super::recover_v2_engine_source_id(&app_data, &v2_source(&root));
        let after = fs::read(&old_db).expect("read v7 index after legacy inspection");

        assert_eq!(
            after, before,
            "v7 compatibility inspection must never migrate or write the catalog"
        );
        assert_eq!(recovered, Ok(Some(source_id)));
    }

    fn v2_source(root: &SourceRoot) -> super::LibrarySourceV2 {
        let path = NativePath::capture(&root.path);
        super::LibrarySourceV2 {
            id: path.source_id(),
            path,
            identity: Some(root.identity.clone()),
        }
    }

    fn assert_v2_refusal_preserves_managed_state(
        registry_path: &Path,
        registry_before: &[u8],
        old_db: &Path,
        new_db: &Path,
    ) {
        assert!(matches!(
            LibraryRegistry::load_from(registry_path),
            Err(LibraryError::CorruptRegistry) | Err(LibraryError::CleanupFailed { .. })
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
        let root = mkdir(temp.path(), "notes");
        let user_file = root.join("keep.md");
        fs::write(&user_file, "# keep").expect("write user file");
        let index_file = temp.path().join("catdb.db");
        fs::write(&index_file, b"index stays").expect("write index marker");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("add root");
        let canonical = fs::canonicalize(&root).expect("canonical root");
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
        assert!(registry.sources().is_empty());
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
    fn version_two_path_ids_and_managed_state_migrate_once_to_stable_identity_keys() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        fs::create_dir(&app_data).expect("create app data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "notes");
        let admitted = bound_root(&root);
        let native = NativePath::capture(&admitted.path);
        let path_id = native.source_id();
        let v2 = serde_json::json!({
            "version": 2,
            "active_source_id": path_id.clone(),
            "sources": [{
                "id": path_id.clone(),
                "path": native,
                "identity": admitted.identity.clone(),
            }],
        });
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&v2).expect("encode V2 registry"),
        )
        .expect("write V2 registry");
        let old_db = mdreader_ingest::legacy_workspace_index_path_for_path_identity(
            &app_data,
            &admitted.path,
            &admitted.identity,
        );
        let legacy_engine = catdb_engine::EmbeddedCatDb::open(
            catdb_engine::EngineConfig::new(
                &old_db,
                std::sync::Arc::new(catdb_embed::HashEmbedder::new()),
            )
            .with_domain(mdreader_ingest::registration()),
        )
        .expect("open authentic old managed index");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(legacy_engine.import(&admitted.path, catdb_engine::ImportOptions::default()))
            .expect("import authentic old managed index");
        let legacy_sources = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("source runtime")
            .block_on(legacy_engine.store().list_sources())
            .expect("list authentic old sources");
        assert_eq!(legacy_sources.len(), 1);
        let legacy_source_id = legacy_sources[0].source_id;
        drop(legacy_engine);
        fs::write(old_db.with_file_name("proposals.db"), b"proposals")
            .expect("write old proposals");

        let migrated = LibraryRegistry::load_from(&registry_path).expect("migrate V2");
        let opaque_id = migrated.sources()[0].id.clone();
        assert_ne!(opaque_id, path_id);
        assert_eq!(migrated.restore_source_id(), Some(opaque_id.as_str()));
        assert_eq!(migrated.sources()[0].engine_source_id(), legacy_source_id);
        let new_db = mdreader_ingest::workspace_index_path_for_stable_identity(
            &app_data,
            &admitted.identity,
        );
        assert!(!old_db.exists());
        assert!(new_db.is_file());
        assert_eq!(
            fs::read(new_db.with_file_name("proposals.db")).expect("migrated proposals"),
            b"proposals"
        );

        let second = LibraryRegistry::load_from(&registry_path).expect("idempotent reload");
        assert_eq!(second.sources()[0].id, opaque_id);
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

    #[test]
    fn version_two_refuses_newer_and_partial_catalog_mutations_without_writing_them() {
        for (label, mutation) in [
            ("newer", "PRAGMA user_version = 11;"),
            (
                "partial",
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE links;",
            ),
        ] {
            let temp = tempdir().expect("tempdir");
            let app_data = temp.path().join("app-data");
            let registry_path = app_data.join("library.json");
            let root = bound_root(&mkdir(temp.path(), "notes"));
            fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
            super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
            let registry_before = fs::read(&registry_path).expect("read V2 registry");
            let (old_db, new_db) = v2_managed_paths(&app_data, &root);
            create_authentic_v2_index(&old_db, &root);
            mutate_catalog_without_leaving_sidecars(&old_db, mutation);
            let catalog_before = fs::read(&old_db).expect("read mutated catalog");

            assert_v2_refusal_preserves_managed_state(
                &registry_path,
                &registry_before,
                &old_db,
                &new_db,
            );
            assert_eq!(
                fs::read(&old_db).expect("read refused catalog"),
                catalog_before,
                "refusing the {label} catalog must not mutate it"
            );
        }
    }

    // APFS rejects arbitrary non-UTF-8 directory names at creation time, so
    // this authentic filesystem/engine migration runs on Unix platforms that
    // can actually manufacture the historical path.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn version_two_non_utf8_root_recovers_the_engine_persisted_path_representation() {
        use std::os::unix::ffi::OsStringExt;

        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root_path = temp.path().join(OsString::from_vec(b"notes-\xff".to_vec()));
        fs::create_dir(&root_path).expect("create non-UTF-8 root");
        fs::write(root_path.join("note.md"), b"# alpha\n").expect("write Markdown");
        let root = bound_root(&root_path);
        super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
        let (old_db, new_db) = v2_managed_paths(&app_data, &root);
        let source_id = create_authentic_v2_index(&old_db, &root);

        let migrated = LibraryRegistry::load_from(&registry_path)
            .expect("the exact engine-persisted display representation is comparable");

        assert_eq!(migrated.sources().len(), 1);
        assert_eq!(migrated.sources()[0].engine_source_id(), source_id);
        assert!(!old_db.exists());
        assert!(new_db.is_file());
    }

    #[test]
    fn version_two_recovers_after_crash_moved_workspace_before_registry_publish() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
        super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
        let (old_db, new_db) = v2_managed_paths(&app_data, &root);
        let source_id = create_authentic_v2_index(&old_db, &root);
        fs::rename(
            old_db.parent().expect("old workspace"),
            new_db.parent().expect("new workspace"),
        )
        .expect("simulate crash after managed move and before registry publish");

        let migrated =
            LibraryRegistry::load_from(&registry_path).expect("restart completes migration");

        assert_eq!(migrated.sources().len(), 1);
        assert_eq!(migrated.sources()[0].engine_source_id(), source_id);
        assert!(!old_db.exists());
        assert!(new_db.is_file());
    }

    #[test]
    fn version_two_refuses_ambiguous_old_and_new_managed_locations() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
        super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
        let before = fs::read(&registry_path).expect("read V2 registry");
        let (old_db, new_db) = v2_managed_paths(&app_data, &root);
        create_authentic_v2_index(&old_db, &root);
        fs::create_dir_all(new_db.parent().expect("new workspace"))
            .expect("create ambiguous new workspace");
        fs::write(&new_db, b"second managed candidate").expect("write ambiguous candidate");

        assert!(matches!(
            LibraryRegistry::load_from(&registry_path),
            Err(LibraryError::CleanupFailed { .. })
        ));
        assert_eq!(
            fs::read(&registry_path).expect("unchanged registry"),
            before
        );
        assert!(old_db.is_file());
        assert!(new_db.is_file());
    }

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
    fn version_two_refuses_malformed_or_ambiguous_source_rows_and_path_mismatch() {
        for kind in [
            "malformed-db",
            "zero",
            "multiple",
            "non-uuid",
            "path-mismatch",
        ] {
            let temp = tempdir().expect("tempdir");
            let app_data = temp.path().join("app-data");
            let registry_path = app_data.join("library.json");
            let root = bound_root(&mkdir(temp.path(), "notes"));
            fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
            super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
            let before = fs::read(&registry_path).expect("read V2 registry");
            let (old_db, new_db) = v2_managed_paths(&app_data, &root);

            if kind == "malformed-db" {
                fs::create_dir_all(old_db.parent().expect("old workspace"))
                    .expect("create old workspace");
                fs::write(&old_db, b"not sqlite").expect("write malformed DB");
            } else {
                create_authentic_v2_index(&old_db, &root);
                let connection =
                    rusqlite::Connection::open(&old_db).expect("open authentic index for mutation");
                match kind {
                    "zero" => {
                        connection
                            .execute("DELETE FROM sources", [])
                            .expect("remove source");
                    }
                    "multiple" => {
                        connection
                            .execute(
                                "INSERT INTO sources
                                 SELECT ?1, root_path, enabled, watch, include_globs,
                                        exclude_globs, max_file_size, respect_gitignore,
                                        embed_policy
                                   FROM sources LIMIT 1",
                                [uuid::Uuid::new_v4().to_string()],
                            )
                            .expect("insert second source");
                    }
                    "non-uuid" => {
                        connection
                            .execute("UPDATE sources SET source_id = 'not-a-uuid'", [])
                            .expect("malform source UUID");
                    }
                    "path-mismatch" => {
                        connection
                            .execute("UPDATE sources SET root_path = '/renderer/claim'", [])
                            .expect("mismatch source root");
                    }
                    _ => unreachable!("covered cases"),
                }
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
    fn version_two_save_failure_rolls_managed_move_back_before_returning() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
        super::write_v2_registry_for_test(&registry_path, &root).expect("write V2 registry");
        let before = fs::read(&registry_path).expect("read V2 registry");
        let (old_db, new_db) = v2_managed_paths(&app_data, &root);
        create_authentic_v2_index(&old_db, &root);

        let result = LibraryRegistry::migrate_v2_with_persist(&registry_path, &before, |_, _| {
            Err(LibraryError::PersistFailed)
        });

        assert!(matches!(result, Err(LibraryError::PersistFailed)));
        assert_eq!(
            fs::read(&registry_path).expect("unchanged registry"),
            before
        );
        assert!(old_db.is_file(), "managed move was rolled back");
        assert!(!new_db.exists(), "failed publish leaves no V4 managed path");
    }

    #[test]
    fn version_two_recovery_refuses_regular_file_swap_between_check_and_open() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
        let (old_db, _new_db) = v2_managed_paths(&app_data, &root);
        create_authentic_v2_index(&old_db, &root);

        let replacement_dir = temp.path().join("replacement");
        let replacement_db = replacement_dir.join("catdb.db");
        let replacement_source_id = create_authentic_v2_index(&replacement_db, &root);
        let original_parked = old_db.with_file_name("checked-original.db");
        let source = v2_source(&root);
        let recovered = super::recover_v2_engine_source_id_with_hooks(
            &app_data,
            &source,
            |_| {
                fs::rename(&old_db, &original_parked).expect("park checked file");
                fs::rename(&replacement_db, &old_db).expect("swap regular replacement");
            },
            |_| {},
        );

        assert_eq!(
            recovered,
            Err(LibraryError::CorruptRegistry),
            "the UUID from the path-substituted DB ({replacement_source_id}) must never be adopted"
        );
        assert!(original_parked.is_file());
        assert!(old_db.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn version_two_recovery_reads_checked_file_across_regular_file_aba_swap() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let root = bound_root(&mkdir(temp.path(), "notes"));
        fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
        let (old_db, _new_db) = v2_managed_paths(&app_data, &root);
        let original_source_id = create_authentic_v2_index(&old_db, &root);

        let replacement_db = temp.path().join("replacement/catdb.db");
        let replacement_source_id = create_authentic_v2_index(&replacement_db, &root);
        let original_parked = old_db.with_file_name("checked-original.db");
        let replacement_parked = replacement_db.with_file_name("opened-replacement.db");
        let source = v2_source(&root);

        let recovered = super::recover_v2_engine_source_id_with_hooks(
            &app_data,
            &source,
            |_| {
                fs::rename(&old_db, &original_parked).expect("park checked original");
                fs::rename(&replacement_db, &old_db).expect("install replacement before open");
            },
            |_| {
                fs::rename(&old_db, &replacement_parked).expect("park opened replacement");
                fs::rename(&original_parked, &old_db).expect("restore checked original");
            },
        );

        assert_eq!(
            recovered,
            Ok(Some(original_source_id)),
            "the UUID from the transient replacement DB ({replacement_source_id}) must never be adopted"
        );
        assert!(old_db.is_file());
        assert!(replacement_parked.is_file());
    }

    #[test]
    fn version_two_recovery_refuses_sqlite_recovery_sidecars() {
        for suffix in ["-wal", "-shm", "-journal"] {
            let temp = tempdir().expect("tempdir");
            let app_data = temp.path().join("app-data");
            let root = bound_root(&mkdir(temp.path(), "notes"));
            fs::write(root.path.join("note.md"), b"# alpha\n").expect("write Markdown");
            let (old_db, _new_db) = v2_managed_paths(&app_data, &root);
            let source_id = create_authentic_v2_index(&old_db, &root);
            let mut sidecar = old_db.as_os_str().to_os_string();
            sidecar.push(suffix);
            fs::write(PathBuf::from(sidecar), b"unresolved SQLite state")
                .expect("write recovery sidecar");

            assert_eq!(
                super::recover_v2_engine_source_id(&app_data, &v2_source(&root)),
                Err(LibraryError::CorruptRegistry),
                "recovery must not adopt {source_id} while {suffix} state is unresolved"
            );
        }
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
}
