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
    identity: SourceRootIdentity,
}

impl LibrarySource {
    pub(crate) fn path(&self) -> Result<PathBuf, LibraryError> {
        self.path.restore()
    }

    fn root(&self) -> Result<SourceRoot, LibraryError> {
        Ok(SourceRoot {
            path: self.path()?,
            identity: self.identity.clone(),
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
        if self.sources.iter().any(|source| source.id == source_id) {
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
            identity: admitted.identity,
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
        if let Err(error) = source.root()?.verify() {
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
        let source_root = self.sources[index].root()?;
        if active_root.is_some_and(|root| root == &source_root) {
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
                index_state: if crate::engine::db_path_under(app_data, &source.root()?).is_file() {
                    LibraryIndexState::Present
                } else {
                    LibraryIndexState::NotCreated
                },
                active: active_root.is_some_and(|active| {
                    active.path == path && active.identity == source.identity
                }),
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
        if stored.sources.len() > MAX_LIBRARY_SOURCES {
            return Err(LibraryError::CorruptRegistry {
                message: "library registry exceeds its source bound".to_string(),
            });
        }
        let mut ids = HashSet::new();
        for source in &stored.sources {
            let path = source.path()?;
            if !path.is_absolute()
                || source.path.source_id() != source.id
                || !ids.insert(&source.id)
            {
                return Err(LibraryError::CorruptRegistry {
                    message: "library registry contains a malformed or duplicate source"
                        .to_string(),
                });
            }
        }
        if stored
            .active_source_id
            .as_ref()
            .is_some_and(|active| !ids.contains(active))
        {
            return Err(LibraryError::CorruptRegistry {
                message: "library registry active source is not a member".to_string(),
            });
        }
        Ok(Self {
            active_source_id: stored.active_source_id,
            sources: stored.sources,
        })
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
/// `None` means the caller must not call `set_workspace`/`watch_workspace`.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn reconcile_library_restore(
    app: AppHandle,
    session_root: Option<String>,
    state: tauri::State<'_, LibraryState>,
) -> Result<Option<LibrarySourceDto>, LibraryError> {
    let _guard = lock_library(&state)?;
    let (path, app_data) = registry_path(&app)?;
    LibraryStore::new(path).read(|registry| {
        let source_id = registry.reconcile_restore(session_root.as_deref().map(Path::new))?;
        let Some(source_id) = source_id else {
            return Ok(None);
        };
        Ok(registry
            .snapshot(&app_data, None)?
            .sources
            .into_iter()
            .find(|source| source.id == source_id))
    })
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

        body["version"] = serde_json::json!(LIBRARY_VERSION - 1);
        fs::write(
            &file,
            serde_json::to_vec_pretty(&body).expect("encode old version"),
        )
        .expect("write old registry");
        assert_eq!(
            LibraryRegistry::load_from(&file).expect_err("old version must fail"),
            LibraryError::UnsupportedVersion {
                found: LIBRARY_VERSION - 1,
                expected: LIBRARY_VERSION,
            }
        );
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
}
