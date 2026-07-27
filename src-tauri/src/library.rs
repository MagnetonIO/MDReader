//! Backend-owned local-library membership.
//!
//! The active CatDB engine remains deliberately single-root. This registry says
//! which local Markdown roots MDReader knows about and which successfully
//! granted root is the restore candidate; it does not claim that their indexes
//! are queried together. Paths are canonicalized before admission and persisted
//! as native OS units so a lossy webview display string is never the authority.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::file_lock::AdvisoryFileLock;
use mdreader_ingest::{SourceRoot, SourceRootIdentity};

pub(crate) const LIBRARY_VERSION: u32 = 2;
const MAX_LIBRARY_SOURCES: usize = 128;

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum LibraryError {
    InvalidRoot { message: String },
    DuplicateRoot { source_id: String },
    SourceNotFound { source_id: String },
    SourceUnavailable { source_id: String, reason: String },
    ActiveSource { source_id: String },
    GrantMismatch { source_id: String },
    OpenFailed { source_id: String, message: String },
    CorruptRegistry { message: String },
    UnsupportedVersion { found: u32, expected: u32 },
    PersistFailed { message: String },
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot { message }
            | Self::CorruptRegistry { message }
            | Self::PersistFailed { message } => formatter.write_str(message),
            Self::DuplicateRoot { source_id } => {
                write!(formatter, "source is already in the library: {source_id}")
            }
            Self::SourceNotFound { source_id } => {
                write!(formatter, "library source was not found: {source_id}")
            }
            Self::SourceUnavailable { source_id, reason } => {
                write!(
                    formatter,
                    "library source {source_id} is unavailable: {reason}"
                )
            }
            Self::ActiveSource { source_id } => {
                write!(
                    formatter,
                    "the active source cannot be forgotten: {source_id}"
                )
            }
            Self::GrantMismatch { source_id } => write!(
                formatter,
                "source {source_id} is not the current granted workspace"
            ),
            Self::OpenFailed { source_id, message } => {
                write!(
                    formatter,
                    "library source {source_id} could not be opened: {message}"
                )
            }
            Self::UnsupportedVersion { found, expected } => write!(
                formatter,
                "unsupported library registry version {found}; expected {expected}"
            ),
        }
    }
}

impl std::error::Error for LibraryError {}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LibraryIndexState {
    Present,
    NotCreated,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum LibraryAvailability {
    Available,
    Missing { reason: String },
    Unreadable { reason: String },
    NotDirectory { reason: String },
}

impl From<LibrarySourceState> for LibraryAvailability {
    fn from(state: LibrarySourceState) -> Self {
        match state {
            LibrarySourceState::Available => Self::Available,
            LibrarySourceState::Missing { reason } => Self::Missing { reason },
            LibrarySourceState::Unreadable { reason } => Self::Unreadable { reason },
            LibrarySourceState::NotDirectory { reason } => Self::NotDirectory { reason },
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
pub(crate) struct LibrarySnapshot {
    pub version: u32,
    pub sources: Vec<LibrarySourceDto>,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct PickedLibrarySource {
    pub source: Option<LibrarySourceDto>,
    pub added: bool,
}

#[derive(Default)]
pub(crate) struct LibraryState(Mutex<()>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LibrarySourceState {
    Available,
    Missing { reason: String },
    Unreadable { reason: String },
    NotDirectory { reason: String },
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
            return Err(LibraryError::CorruptRegistry {
                message: "library registry contains an empty or NUL path".to_string(),
            });
        }

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            if self.encoding != NativeEncoding::UnixBytes {
                return Err(LibraryError::CorruptRegistry {
                    message: "library registry path encoding does not match this platform"
                        .to_string(),
                });
            }
            let bytes = self
                .units
                .iter()
                .copied()
                .map(u8::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| LibraryError::CorruptRegistry {
                    message: "library registry contains an invalid native path unit".to_string(),
                })?;
            Ok(PathBuf::from(OsString::from_vec(bytes)))
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            if self.encoding != NativeEncoding::WindowsWide {
                return Err(LibraryError::CorruptRegistry {
                    message: "library registry path encoding does not match this platform"
                        .to_string(),
                });
            }
            let wide = self
                .units
                .iter()
                .copied()
                .map(u16::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| LibraryError::CorruptRegistry {
                    message: "library registry contains an invalid native path unit".to_string(),
                })?;
            Ok(PathBuf::from(OsString::from_wide(&wide)))
        }

        #[cfg(not(any(unix, windows)))]
        {
            if self.encoding != NativeEncoding::Utf8 {
                return Err(LibraryError::CorruptRegistry {
                    message: "library registry path encoding does not match this platform"
                        .to_string(),
                });
            }
            let bytes = self
                .units
                .iter()
                .copied()
                .map(u8::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| LibraryError::CorruptRegistry {
                    message: "library registry contains an invalid UTF-8 path unit".to_string(),
                })?;
            let path = String::from_utf8(bytes).map_err(|_| LibraryError::CorruptRegistry {
                message: "library registry contains invalid UTF-8".to_string(),
            })?;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LibrarySource {
    pub(crate) id: String,
    path: NativePath,
    #[serde(deserialize_with = "deserialize_identity_binding")]
    identity: Option<SourceRootIdentity>,
}

fn deserialize_identity_binding<'de, D>(
    deserializer: D,
) -> Result<Option<SourceRootIdentity>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<SourceRootIdentity>::deserialize(deserializer)
}

impl LibrarySource {
    pub(crate) fn path(&self) -> Result<PathBuf, LibraryError> {
        self.path.restore()
    }

    fn root(&self) -> Result<SourceRoot, LibraryError> {
        let identity = self
            .identity
            .clone()
            .ok_or_else(|| LibraryError::SourceUnavailable {
                source_id: self.id.clone(),
                reason: "source identity requires re-admission after registry migration"
                    .to_string(),
            })?;
        SourceRoot::restore(self.path()?, identity).map_err(|error| {
            LibraryError::SourceUnavailable {
                source_id: self.id.clone(),
                reason: error.to_string(),
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLibrary {
    version: u32,
    active_source_id: Option<String>,
    sources: Vec<LibrarySource>,
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

#[derive(Debug, Clone)]
pub(crate) struct LibraryRegistry {
    active_source_id: Option<String>,
    sources: Vec<LibrarySource>,
}

impl LibraryRegistry {
    pub(crate) fn empty() -> Self {
        Self {
            active_source_id: None,
            sources: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn sources(&self) -> &[LibrarySource] {
        &self.sources
    }

    pub(crate) fn path_for(&self, source_id: &str) -> Option<PathBuf> {
        self.source_for(source_id)
            .and_then(|source| source.path().ok())
    }

    fn source_for(&self, source_id: &str) -> Option<&LibrarySource> {
        self.sources.iter().find(|source| source.id == source_id)
    }

    #[cfg(test)]
    pub(crate) fn restore_source_id(&self) -> Option<&str> {
        self.active_source_id.as_deref()
    }

    pub(crate) fn add_root(&mut self, requested: &Path) -> Result<LibrarySource, LibraryError> {
        let canonical = fs::canonicalize(requested).map_err(|error| LibraryError::InvalidRoot {
            message: format!("cannot resolve library root: {error}"),
        })?;
        if !canonical.is_dir() {
            return Err(LibraryError::InvalidRoot {
                message: "library root is not a directory".to_string(),
            });
        }
        let admitted =
            SourceRoot::capture(canonical.clone()).map_err(|error| LibraryError::InvalidRoot {
                message: error.to_string(),
            })?;
        let native = NativePath::capture(&canonical);
        let source_id = native.source_id();
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.id == source_id)
        {
            if source.identity.is_none() {
                source.path = native;
                source.identity = Some(admitted.identity);
                return Ok(source.clone());
            }
            return Err(LibraryError::DuplicateRoot { source_id });
        }
        if self.sources.len() >= MAX_LIBRARY_SOURCES {
            return Err(LibraryError::InvalidRoot {
                message: format!("library is limited to {MAX_LIBRARY_SOURCES} local roots"),
            });
        }
        let source = LibrarySource {
            id: source_id,
            path: native,
            identity: Some(admitted.identity),
        };
        self.sources.push(source.clone());
        Ok(source)
    }

    pub(crate) fn source_state(&self, source_id: &str) -> Result<LibrarySourceState, LibraryError> {
        let source = self
            .source_for(source_id)
            .ok_or_else(|| LibraryError::SourceNotFound {
                source_id: source_id.to_string(),
            })?;
        let path = source.path()?;
        let canonical = match fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(error) => return Ok(classify_probe_error(error)),
        };
        if canonical != path {
            return Ok(LibrarySourceState::Unreadable {
                reason: "the folder identity changed since it was added".to_string(),
            });
        }
        let root = match source.root() {
            Ok(root) => root,
            Err(LibraryError::SourceUnavailable { reason, .. }) => {
                return Ok(LibrarySourceState::Unreadable { reason })
            }
            Err(error) => return Err(error),
        };
        if let Err(error) = root.verify() {
            return Ok(LibrarySourceState::Unreadable {
                reason: error.to_string(),
            });
        }
        match fs::metadata(&canonical) {
            Ok(metadata) if !metadata.is_dir() => Ok(LibrarySourceState::NotDirectory {
                reason: "the stored root is no longer a directory".to_string(),
            }),
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
            .ok_or_else(|| LibraryError::SourceNotFound {
                source_id: source_id.to_string(),
            })?;
        match self.source_state(source_id)? {
            LibrarySourceState::Available => source.root(),
            LibrarySourceState::Missing { reason } => Err(LibraryError::SourceUnavailable {
                source_id: source_id.to_string(),
                reason: format!("missing: {reason}"),
            }),
            LibrarySourceState::Unreadable { reason } => Err(LibraryError::SourceUnavailable {
                source_id: source_id.to_string(),
                reason: format!("unreadable: {reason}"),
            }),
            LibrarySourceState::NotDirectory { reason } => Err(LibraryError::SourceUnavailable {
                source_id: source_id.to_string(),
                reason: format!("not a directory: {reason}"),
            }),
        }
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
            .position(|source| source.id == source_id)
            .ok_or_else(|| LibraryError::SourceNotFound {
                source_id: source_id.to_string(),
            })?;
        let source = &self.sources[index];
        let source_path = source.path()?;
        if active_root.is_some_and(|root| {
            source.identity.as_ref() == Some(&root.identity) && source_path == root.path
        }) {
            return Err(LibraryError::ActiveSource {
                source_id: source_id.to_string(),
            });
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
            return Err(LibraryError::GrantMismatch {
                source_id: source_id.to_string(),
            });
        }
        source_root
            .verify()
            .map_err(|error| LibraryError::SourceUnavailable {
                source_id: source_id.to_string(),
                reason: error.to_string(),
            })?;
        self.active_source_id = Some(source_id.to_string());
        Ok(())
    }

    pub(crate) fn snapshot(
        &self,
        app_data: &Path,
        active_root: Option<&SourceRoot>,
    ) -> Result<LibrarySnapshot, LibraryError> {
        let mut sources = Vec::with_capacity(self.sources.len());
        for source in &self.sources {
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
                index_state: bound_root
                    .as_ref()
                    .map_or(LibraryIndexState::NotCreated, |root| {
                        if crate::engine::db_path_under(app_data, root).is_file() {
                            LibraryIndexState::Present
                        } else {
                            LibraryIndexState::NotCreated
                        }
                    }),
                active: bound_root
                    .as_ref()
                    .is_some_and(|root| active_root == Some(root)),
                restore_candidate: self.active_source_id.as_deref() == Some(source.id.as_str()),
            });
        }
        Ok(LibrarySnapshot {
            version: LIBRARY_VERSION,
            sources,
        })
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
            let active_path =
                self.path_for(active_id)
                    .ok_or_else(|| LibraryError::CorruptRegistry {
                        message: "library registry active source is not a member".to_string(),
                    })?;
            if active_path.to_string_lossy() == requested_root.to_string_lossy()
                && matches!(self.source_state(active_id)?, LibrarySourceState::Available)
            {
                return Ok(Some(active_id.to_string()));
            }
        }

        let Ok(canonical) = fs::canonicalize(requested_root) else {
            return Ok(None);
        };
        for source in &self.sources {
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
            Err(error) => {
                return Err(LibraryError::CorruptRegistry {
                    message: format!("cannot read library registry: {error}"),
                })
            }
        };
        #[derive(Deserialize)]
        struct VersionProbe {
            version: u32,
        }
        let version: VersionProbe =
            serde_json::from_slice(&bytes).map_err(|error| LibraryError::CorruptRegistry {
                message: format!("cannot parse library registry: {error}"),
            })?;
        if version.version == 1 {
            return Self::migrate_v1(path, &bytes);
        }
        if version.version != LIBRARY_VERSION {
            return Err(LibraryError::UnsupportedVersion {
                found: version.version,
                expected: LIBRARY_VERSION,
            });
        }
        let stored: StoredLibrary =
            serde_json::from_slice(&bytes).map_err(|error| LibraryError::CorruptRegistry {
                message: format!("cannot parse library registry: {error}"),
            })?;
        Self::validate_membership(
            stored.active_source_id.as_deref(),
            stored
                .sources
                .iter()
                .map(|source| (&source.id, &source.path)),
            stored.sources.len(),
        )?;
        Ok(Self {
            active_source_id: stored.active_source_id,
            sources: stored.sources,
        })
    }

    fn migrate_v1(path: &Path, bytes: &[u8]) -> Result<Self, LibraryError> {
        let stored: StoredLibraryV1 =
            serde_json::from_slice(bytes).map_err(|error| LibraryError::CorruptRegistry {
                message: format!("cannot parse V1 library registry: {error}"),
            })?;
        Self::validate_membership(
            stored.active_source_id.as_deref(),
            stored
                .sources
                .iter()
                .map(|source| (&source.id, &source.path)),
            stored.sources.len(),
        )?;

        quarantine_v1_workspaces(path, &stored.sources)?;
        let sources = stored
            .sources
            .into_iter()
            .map(|source| {
                let native_path = source.path.restore()?;
                let identity = SourceRoot::capture(native_path)
                    .ok()
                    .map(|root| root.identity);
                Ok(LibrarySource {
                    id: source.id,
                    path: source.path,
                    identity,
                })
            })
            .collect::<Result<Vec<_>, LibraryError>>()?;
        let migrated = Self {
            // V1's marker authenticated only a path. Requiring a successful
            // V2 grant before recording it again prevents automatic restore
            // from inheriting that weaker authorization.
            active_source_id: None,
            sources,
        };
        migrated.save_to(path)?;
        Ok(migrated)
    }

    fn validate_membership<'a>(
        active_source_id: Option<&str>,
        sources: impl Iterator<Item = (&'a String, &'a NativePath)>,
        source_count: usize,
    ) -> Result<(), LibraryError> {
        if source_count > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry {
                message: "library registry exceeds its source bound".to_string(),
            });
        }
        let mut ids = HashSet::new();
        for (source_id, native_path) in sources {
            let path = native_path.restore()?;
            if !path.is_absolute()
                || native_path.source_id() != *source_id
                || !ids.insert(source_id.as_str())
            {
                return Err(LibraryError::CorruptRegistry {
                    message: "library registry contains a malformed or duplicate source"
                        .to_string(),
                });
            }
        }
        if active_source_id.is_some_and(|active| !ids.contains(active)) {
            return Err(LibraryError::CorruptRegistry {
                message: "library registry active source is not a member".to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), LibraryError> {
        let parent = path.parent().ok_or_else(|| LibraryError::PersistFailed {
            message: "library registry has no parent directory".to_string(),
        })?;
        fs::create_dir_all(parent).map_err(|error| LibraryError::PersistFailed {
            message: format!("cannot create library registry directory: {error}"),
        })?;
        let body = serde_json::to_vec_pretty(&StoredLibrary {
            version: LIBRARY_VERSION,
            active_source_id: self.active_source_id.clone(),
            sources: self.sources.clone(),
        })
        .map_err(|error| LibraryError::PersistFailed {
            message: format!("cannot encode library registry: {error}"),
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
            LibraryError::PersistFailed {
                message: format!("cannot create library registry staging file: {error}"),
            }
        })?;
        temporary
            .write_all(&body)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| LibraryError::PersistFailed {
                message: format!("cannot stage library registry: {error}"),
            })?;
        temporary
            .persist(path)
            .map(|_| ())
            .map_err(|error| LibraryError::PersistFailed {
                message: format!("cannot publish library registry: {}", error.error),
            })
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
    let app_data = registry_path
        .parent()
        .ok_or_else(|| LibraryError::PersistFailed {
            message: "library registry has no parent directory".to_string(),
        })?;
    let quarantine_root = app_data.join("legacy-v1-path-workspaces");

    for source in sources {
        let source_path = source.path.restore()?;
        let legacy = v1_workspace_dir(app_data, &source_path);
        let metadata = match fs::symlink_metadata(&legacy) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(LibraryError::PersistFailed {
                    message: format!(
                        "cannot inspect legacy workspace {}: {error}",
                        legacy.display()
                    ),
                })
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(LibraryError::CorruptRegistry {
                message: format!(
                    "legacy workspace is not a real directory: {}",
                    legacy.display()
                ),
            });
        }

        fs::create_dir_all(&quarantine_root).map_err(|error| LibraryError::PersistFailed {
            message: format!("cannot create legacy workspace quarantine: {error}"),
        })?;
        let quarantine_metadata = fs::symlink_metadata(&quarantine_root).map_err(|error| {
            LibraryError::PersistFailed {
                message: format!("cannot inspect legacy workspace quarantine: {error}"),
            }
        })?;
        if quarantine_metadata.file_type().is_symlink() || !quarantine_metadata.is_dir() {
            return Err(LibraryError::PersistFailed {
                message: "legacy workspace quarantine is not a real directory".to_string(),
            });
        }

        let key = legacy
            .file_name()
            .ok_or_else(|| LibraryError::CorruptRegistry {
                message: "legacy workspace path has no identity key".to_string(),
            })?;
        let destination = quarantine_root.join(key);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                return Err(LibraryError::PersistFailed {
                    message: format!(
                        "legacy workspace and quarantine both exist: {}",
                        destination.display()
                    ),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(LibraryError::PersistFailed {
                    message: format!("cannot inspect legacy workspace quarantine target: {error}"),
                })
            }
        }
        fs::rename(&legacy, &destination).map_err(|error| LibraryError::PersistFailed {
            message: format!(
                "cannot quarantine legacy workspace {}: {error}",
                legacy.display()
            ),
        })?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct LibraryStore {
    registry_path: PathBuf,
}

impl LibraryStore {
    fn new(registry_path: PathBuf) -> Self {
        Self { registry_path }
    }

    fn lock_path(&self) -> Result<PathBuf, LibraryError> {
        self.registry_path
            .parent()
            .map(|parent| parent.join("library.lock"))
            .ok_or_else(|| LibraryError::PersistFailed {
                message: "library registry has no parent directory".to_string(),
            })
    }

    fn acquire(&self) -> Result<AdvisoryFileLock, LibraryError> {
        AdvisoryFileLock::acquire(&self.lock_path()?).map_err(|error| LibraryError::PersistFailed {
            message: format!("cannot acquire library registry lock: {error}"),
        })
    }

    fn read<T>(
        &self,
        operation: impl FnOnce(&LibraryRegistry) -> Result<T, LibraryError>,
    ) -> Result<T, LibraryError> {
        let _lease = self.acquire()?;
        let registry = LibraryRegistry::load_from(&self.registry_path)?;
        operation(&registry)
    }

    fn update<T>(
        &self,
        operation: impl FnOnce(&mut LibraryRegistry) -> Result<T, LibraryError>,
    ) -> Result<T, LibraryError> {
        let _lease = self.acquire()?;
        let mut registry = LibraryRegistry::load_from(&self.registry_path)?;
        let result = operation(&mut registry)?;
        registry.save_to(&self.registry_path)?;
        Ok(result)
    }

    fn reconcile_restore_or_migrate(
        &self,
        requested_root: Option<&Path>,
        app_data: &Path,
    ) -> Result<Option<LibrarySourceDto>, LibraryError> {
        let _lease = self.acquire()?;
        let registry_exists = match fs::symlink_metadata(&self.registry_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(LibraryError::CorruptRegistry {
                    message: "library registry is not a regular file".to_string(),
                })
            }
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(LibraryError::CorruptRegistry {
                    message: format!("cannot inspect library registry: {error}"),
                })
            }
        };

        if registry_exists {
            let registry = LibraryRegistry::load_from(&self.registry_path)?;
            let source_id = registry.reconcile_restore(requested_root)?;
            return source_id
                .map(|source_id| {
                    registry
                        .snapshot(app_data, None)?
                        .sources
                        .into_iter()
                        .find(|source| source.id == source_id)
                        .ok_or_else(|| LibraryError::CorruptRegistry {
                            message: "restored library source is not a member".to_string(),
                        })
                })
                .transpose();
        }

        let Some(requested_root) = requested_root else {
            return Ok(None);
        };
        let mut registry = LibraryRegistry::empty();
        let source = match registry.add_root(requested_root) {
            Ok(source) => source,
            Err(LibraryError::InvalidRoot { .. }) => return Ok(None),
            Err(error) => return Err(error),
        };
        registry.activation_root(&source.id)?;
        registry.save_to(&self.registry_path)?;
        Ok(registry
            .snapshot(app_data, None)?
            .sources
            .into_iter()
            .find(|candidate| candidate.id == source.id))
    }
}

fn registry_path(app: &AppHandle) -> Result<(PathBuf, PathBuf), LibraryError> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| LibraryError::PersistFailed {
            message: format!("cannot resolve application data directory: {error}"),
        })?;
    Ok((app_data.join("library.json"), app_data))
}

fn active_root(workspace: &crate::commands::WorkspaceState) -> Option<SourceRoot> {
    workspace.current().map(|epoch| epoch.source_root())
}

fn lock_library(state: &LibraryState) -> Result<std::sync::MutexGuard<'_, ()>, LibraryError> {
    state.0.lock().map_err(|_| LibraryError::PersistFailed {
        message: "library registry lock is poisoned".to_string(),
    })
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

    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    let active = active_root(&workspace);
    LibraryStore::new(path).update(|registry| {
        let (source_id, added) = match registry.add_root(&selected) {
            Ok(source) => (source.id, true),
            Err(LibraryError::DuplicateRoot { source_id }) => (source_id, false),
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
    LibraryStore::new(path)
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
        crate::commands::watch_workspace_path_exact(&app, &native_root, &workspace, &watcher)
            .map_err(|message| LibraryError::OpenFailed { source_id, message })
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
        crate::commands::grant_workspace_path_exact(&native_root, &workspace)
            .map(|root| root.to_string_lossy().into_owned())
            .map_err(|message| LibraryError::OpenFailed { source_id, message })
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

/// Forget membership only. The active epoch is protected, and this command
/// never addresses either the user's source directory or the per-root index.
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
    LibraryStore::new(path).update(|registry| {
        registry.forget(&source_id, active.as_ref())?;
        registry.snapshot(&app_data, active.as_ref())
    })
}

pub(crate) fn classify_probe_error(error: io::Error) -> LibrarySourceState {
    match error.kind() {
        io::ErrorKind::NotFound => LibrarySourceState::Missing {
            reason: error.to_string(),
        },
        io::ErrorKind::PermissionDenied => LibrarySourceState::Unreadable {
            reason: error.to_string(),
        },
        _ => LibrarySourceState::Unreadable {
            reason: error.to_string(),
        },
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

    #[test]
    fn duplicate_and_non_directory_roots_are_rejected_without_mutating_membership() {
        let temp = tempdir().expect("tempdir");
        let root = mkdir(temp.path(), "notes");
        let file = temp.path().join("not-a-folder.md");
        fs::write(&file, "# no").expect("write file");
        let mut registry = LibraryRegistry::empty();

        let first = registry.add_root(&root).expect("add root");
        let duplicate = registry.add_root(&root).expect_err("duplicate must fail");
        assert_eq!(
            duplicate,
            LibraryError::DuplicateRoot {
                source_id: first.id.clone()
            }
        );
        assert!(matches!(
            registry.add_root(&file),
            Err(LibraryError::InvalidRoot { .. })
        ));
        assert_eq!(registry.sources().len(), 1);
        assert_eq!(registry.sources()[0].id, first.id);
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
            Ok(LibrarySourceState::Missing { .. })
        ));
        assert_eq!(registry.sources().len(), 1, "missing source remains known");

        let denied = classify_probe_error(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(matches!(denied, LibrarySourceState::Unreadable { .. }));
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
        assert!(
            matches!(state, LibrarySourceState::Unreadable { ref reason }
                if !reason.is_empty() && reason.contains("identity")),
            "the replacement must be a typed, non-empty refusal: {state:?}"
        );
        assert!(matches!(
            registry.activation_path(&source.id),
            Err(LibraryError::SourceUnavailable { ref reason, .. })
                if !reason.is_empty() && reason.contains("identity")
        ));
        assert_eq!(
            fs::read_to_string(moved.join("original.md")).expect("original object remains"),
            "# original"
        );
    }

    #[test]
    fn active_source_cannot_be_forgotten_and_forget_never_deletes_user_or_index_data() {
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
            LibraryError::ActiveSource {
                source_id: source.id.clone()
            }
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
            Err(LibraryError::CorruptRegistry { .. })
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
            Err(LibraryError::CorruptRegistry { ref message })
                if message.contains("identity")
        ));
    }

    #[test]
    fn version_one_registry_migrates_membership_and_quarantines_old_index_and_proposals() {
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

        assert_eq!(registry.sources().len(), 1, "membership is preserved");
        assert_eq!(registry.sources()[0].id, source_id);
        assert!(
            registry.activation_root(&source_id).is_ok(),
            "an available source is rebound to its current filesystem identity"
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
        assert!(
            !crate::engine::db_path_under(
                &app_data,
                &registry.activation_root(&source_id).unwrap()
            )
            .exists(),
            "identity-keyed V2 starts from a clean index instead of adopting path-keyed rows"
        );
    }

    #[test]
    fn unavailable_v1_membership_survives_migration_and_can_be_forgotten_or_readmitted() {
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

        let mut registry =
            LibraryRegistry::load_from(&registry_path).expect("offline membership migrates");
        let snapshot = registry
            .snapshot(&app_data, None)
            .expect("offline source remains listable");
        assert_eq!(snapshot.sources.len(), 1);
        assert!(matches!(
            snapshot.sources[0].state,
            super::LibraryAvailability::Missing { .. }
        ));
        assert_eq!(
            snapshot.sources[0].index_state,
            LibraryIndexState::NotCreated
        );
        assert!(matches!(
            registry.activation_root(&source_id),
            Err(LibraryError::SourceUnavailable { .. })
        ));

        let mut forgettable = registry.clone();
        forgettable
            .forget(&source_id, None)
            .expect("offline migrated membership can be forgotten");
        assert!(forgettable.sources().is_empty());

        fs::create_dir(&root).expect("source returns");
        let rebound = registry
            .add_root(&root)
            .expect("re-picking a migration-pending source binds its identity");
        assert_eq!(rebound.id, source_id, "membership identity is stable");
        assert!(registry.activation_root(&source_id).is_ok());
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
            Err(LibraryError::GrantMismatch { .. })
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
            Err(LibraryError::SourceUnavailable { .. })
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
                Ok(LibrarySourceState::Unreadable { .. })
            ),
            "a path now resolving to a different canonical root is unavailable"
        );
        assert!(matches!(
            registry.activation_path(&source.id),
            Err(LibraryError::SourceUnavailable { .. })
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
            Err(LibraryError::PersistFailed { .. })
        ));
    }

    #[test]
    fn absent_registry_migrates_one_available_legacy_session_root_once() {
        let temp = tempdir().expect("tempdir");
        let app_data = temp.path().join("app-data");
        let registry_path = app_data.join("library.json");
        let root = mkdir(temp.path(), "legacy-notes");
        let canonical = fs::canonicalize(&root).expect("canonical legacy root");
        let store = LibraryStore::new(registry_path.clone());

        let first = store
            .reconcile_restore_or_migrate(Some(&canonical), &app_data)
            .expect("available legacy root migrates")
            .expect("migration returns the admitted member");
        assert_eq!(first.path, canonical.to_string_lossy());
        assert!(registry_path.is_file(), "migration is persisted atomically");

        let first_bytes = fs::read(&registry_path).expect("read first registry");
        let second = store
            .reconcile_restore_or_migrate(Some(&canonical), &app_data)
            .expect("second startup reconciles")
            .expect("the same member restores");
        assert_eq!(second.id, first.id, "filesystem identity is stable");
        assert_eq!(
            fs::read(&registry_path).expect("read second registry"),
            first_bytes,
            "restart is idempotent"
        );
        assert_eq!(
            LibraryRegistry::load_from(&registry_path)
                .expect("load migrated registry")
                .sources()
                .len(),
            1
        );
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

        assert!(store
            .reconcile_restore_or_migrate(Some(&missing), &app_data)
            .expect("missing legacy root is a typed clean refusal")
            .is_none());
        assert!(
            !registry_path.exists(),
            "a refused migration does not consume or fabricate registry state"
        );

        fs::create_dir_all(&app_data).expect("create app data");
        fs::write(&registry_path, b"{corrupt").expect("write corrupt registry");
        let available = mkdir(temp.path(), "available");
        assert!(matches!(
            store.reconcile_restore_or_migrate(Some(&available), &app_data),
            Err(LibraryError::CorruptRegistry { .. })
        ));
        assert_eq!(
            fs::read(&registry_path).expect("corrupt bytes remain"),
            b"{corrupt",
            "corruption is never replaced by a session-derived registry"
        );
    }
}
