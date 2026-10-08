use super::*;
// --------------------------------------------------------------------------

/// A snapshot of index-build progress: the payload streamed on the import
/// [`Channel`] *and* the value [`import_status`] returns. Deliberately coarse —
/// see the module docs on progress granularity.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportProgress {
    /// `"idle"` (no import yet), `"loading"` (constructing/downloading the
    /// embedding model), `"importing"` (in flight), `"reembedding"` (in
    /// flight, and re-embedding the whole workspace because the embedding
    /// model changed), `"done"`, `"model-unavailable"` (the embedding model
    /// could not be loaded — the workspace is open and readable, but nothing
    /// could be indexed and the semantic arm is off), or `"error"` (the index
    /// build itself failed).
    pub phase: String,
    /// Files processed so far (equals `files_total` on `done`; the atomic import
    /// gives no intermediate count).
    pub files_done: u32,
    /// Files the pre-scan found (the denominator the spinner shows).
    pub files_total: u32,
    /// Chunks embedded in this pass (0 when a re-import finds nothing changed).
    pub chunks_embedded: u32,
    /// The failure message when `phase == "error"`.
    pub error: Option<String>,
}

impl Default for ImportProgress {
    fn default() -> Self {
        Self {
            phase: "idle".to_string(),
            files_done: 0,
            files_total: 0,
            chunks_embedded: 0,
            error: None,
        }
    }
}

impl ImportProgress {
    pub(super) fn failed(files_total: u32, message: String) -> Self {
        Self {
            phase: "error".to_string(),
            files_done: 0,
            files_total,
            chunks_embedded: 0,
            error: Some(message),
        }
    }
}

/// A file listing plus which side produced it: `true` = engine index (post
/// import), `false` = the live filesystem fallback (before the first import
/// finishes). The StatusBar surfaces the distinction.
#[derive(Debug, Clone, Serialize, Type)]
pub struct WorkspaceListing {
    /// The rows the sidebar tree renders.
    pub entries: Vec<FileEntry>,
    /// Whether `entries` came from the engine's index.
    pub from_index: bool,
}

/// Why a workspace listing could not be produced.
///
/// Typed on purpose. `Unreadable` is the operating system **refusing to read**
/// the folder — on macOS that is the privacy (TCC) layer withholding
/// `~/Documents`, `~/Desktop` or `~/Downloads` from an app that holds no grant;
/// elsewhere it is an ordinary mode denial. That is a categorically different
/// fact from a folder we read perfectly well which simply holds no markdown,
/// and the two must never render the same. Collapsing them is precisely what
/// let a permission denial reach the user as "No markdown files in this folder
/// yet." — an answer that is not merely unhelpful but false.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ListingError {
    /// The folder could not be READ. Carries the folder, the OS's own words,
    /// and what the user has to do about it.
    Unreadable {
        /// The folder the walk was refused on.
        path: String,
        /// The underlying OS error, verbatim.
        reason: String,
        /// The concrete grant the user must make, and where.
        remedy: String,
    },
    /// Every other listing failure (no workspace open, a superseded epoch, an
    /// index read that failed).
    Failed {
        /// The failure, as text.
        message: String,
    },
}

impl ListingError {
    /// What the user must actually do to grant the access. Platform-specific
    /// because the remedy is: on macOS the folder permission is a privacy
    /// grant that only System Settings can give, and no amount of retrying
    /// inside the app will produce it.
    #[cfg(target_os = "macos")]
    pub(crate) const REMEDY: &'static str =
        "macOS is withholding this folder from MDReader. Open System Settings › Privacy & Security › Files and Folders, enable the folder for MDReader (or grant Full Disk Access), then choose the folder again.";
    /// The non-macOS remedy: an ordinary filesystem permission.
    #[cfg(not(target_os = "macos"))]
    pub(crate) const REMEDY: &'static str =
        "Your account does not have permission to read this folder. Adjust the folder's permissions, then choose the folder again.";

    /// Classify an I/O failure that occurred while walking `path`. A
    /// [`PermissionDenied`](std::io::ErrorKind::PermissionDenied) — `EPERM`
    /// from macOS's privacy layer, `EACCES` from a unix mode — becomes
    /// [`Unreadable`](Self::Unreadable); anything else stays generic.
    pub(super) fn from_walk(path: &Path, err: &std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            Self::Unreadable {
                path: path.display().to_string(),
                reason: err.to_string(),
                remedy: Self::REMEDY.to_string(),
            }
        } else {
            Self::Failed {
                message: format!("scan: {err}"),
            }
        }
    }

    pub(super) fn from_source(path: &Path, error: &mdreader_core::SourceRootError) -> Self {
        if matches!(
            error,
            mdreader_core::SourceRootError::PermissionDenied { .. }
        ) {
            Self::Unreadable {
                path: path.display().to_string(),
                reason: error.to_string(),
                remedy: Self::REMEDY.to_string(),
            }
        } else {
            Self::Failed {
                message: error.to_string(),
            }
        }
    }
}

impl From<String> for ListingError {
    fn from(message: String) -> Self {
        Self::Failed { message }
    }
}

/// One candidate when a wikilink target is ambiguous.
#[derive(Debug, Clone, Serialize, Type)]
pub struct WikiCandidate {
    /// Absolute path to the candidate document.
    pub path: String,
    /// The candidate's file name.
    pub name: String,
}

/// The outcome of resolving a `[[wikilink]]` against the index.
#[derive(Debug, Clone, Serialize, Type)]
pub struct WikiResolution {
    /// `"resolved"` (exactly one match), `"ambiguous"` (several — see
    /// `candidates`), or `"unresolved"` (none).
    pub status: String,
    /// Absolute path of the resolved document (`resolved` only).
    pub path: Option<String>,
    /// File name of the resolved document (`resolved` only).
    pub name: Option<String>,
    /// 1-based source line of the requested heading within the resolved
    /// document, when a heading was given and matched.
    pub line: Option<u32>,
    /// The matches when `status == "ambiguous"`.
    pub candidates: Vec<WikiCandidate>,
}

impl WikiResolution {
    pub(super) fn unresolved() -> Self {
        Self {
            status: "unresolved".to_string(),
            path: None,
            name: None,
            line: None,
            candidates: Vec::new(),
        }
    }

    pub(super) fn resolved(path: String, name: String, line: Option<u32>) -> Self {
        Self {
            status: "resolved".to_string(),
            path: Some(path),
            name: Some(name),
            line,
            candidates: Vec::new(),
        }
    }
}

// --------------------------------------------------------------------------
// Engine lifecycle helpers
