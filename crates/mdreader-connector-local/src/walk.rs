//! A read-only directory walk that honours a pushed-down path prefix.
//!
//! The prefix is not a filter applied to a full listing — it *prunes*, so a
//! directory that cannot contain a matching path is never descended into. That
//! is what makes `path` pushdown a real capability rather than a label: the
//! caller can see it in [`Answer::scanned`](crate::Answer::scanned).
//!
//! # Declared scope
//!
//! Hidden entries (a name starting with `.`) are not walked. This is a
//! *declared scope* — workspace metadata such as `.git` is not a document —
//! and it is stated here rather than left as a surprise. It is not a silent
//! filter on values: a file that is walked always produces a row.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{ReadContext, SourceCursor, SourceError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum WalkError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("path is not valid UTF-8: {0:?}")]
    NonUtf8(PathBuf),
    #[error("fixed walk bound exceeded: {0}")]
    Limit(&'static str),
    #[error("contained open refused `{path}`: {detail}")]
    Containment { path: PathBuf, detail: String },
}

pub(crate) fn map_error(error: WalkError, source_id: &crate::SourceId, root: &Path) -> SourceError {
    match error {
        WalkError::Io(error) => SourceError::Read {
            source_id: source_id.clone(),
            path: root.display().to_string(),
            detail: error.to_string(),
        },
        WalkError::Source(error) => error,
        WalkError::NonUtf8(path) => SourceError::UnrepresentablePath {
            source_id: source_id.clone(),
            path: path.display().to_string(),
        },
        WalkError::Limit(resource) => SourceError::ResourceLimit {
            source_id: source_id.clone(),
            resource,
        },
        WalkError::Containment { path, detail } => SourceError::Containment {
            source_id: source_id.clone(),
            path: path.display().to_string(),
            detail,
        },
    }
}

/// One walked file, identified only by its root-relative path (with `/`
/// separators). The root descriptor supplies authority when it is opened.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Walked {
    pub(crate) rel: String,
}

/// Walk `root`, descending only into directories that could contain a path
/// starting with `prefix`, and yielding only files whose relative path starts
/// with it.
///
/// `extensions`, when given, restricts the walk to files with one of those
/// extensions (the frontmatter source describes markdown documents, not every
/// byte on disk). Results are in relative-path order, so a read is
/// reproducible.
#[cfg(test)]
pub(crate) fn walk(
    root: &Path,
    prefix: &str,
    extensions: Option<&[&str]>,
) -> Result<Vec<Walked>, WalkError> {
    let mut found = Vec::new();
    descend(root, root, prefix, extensions, &mut found)?;
    found.sort();
    Ok(found)
}

const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_WALK_DEPTH: usize = 64;
const MAX_RELATIVE_PATH_BYTES: usize = 4 * 1024;

#[derive(Debug)]
pub(crate) struct WalkPage {
    pub(crate) files: Vec<Walked>,
    pub(crate) has_more: bool,
    pub(crate) cursor: WalkCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LocalCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) walk: Option<WalkCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WalkCursor {
    frames: Vec<WalkFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalkFrame {
    dir: String,
    after: Option<String>,
}

impl Default for WalkCursor {
    fn default() -> Self {
        Self {
            frames: vec![WalkFrame {
                dir: String::new(),
                after: None,
            }],
        }
    }
}

pub(crate) fn decode_cursor(
    cursor: Option<&SourceCursor>,
    key: &[u8; 32],
) -> Result<Option<LocalCursor>, SourceError> {
    cursor
        .map(|cursor| {
            let bytes = cursor.as_bytes();
            let payload_len = bytes
                .len()
                .checked_sub(blake3::OUT_LEN)
                .ok_or(SourceError::InvalidCursor)?;
            let (payload, supplied_tag) = bytes.split_at(payload_len);
            let expected_tag = blake3::keyed_hash(key, payload);
            if supplied_tag != expected_tag.as_bytes() {
                return Err(SourceError::InvalidCursor);
            }
            let decoded: LocalCursor =
                serde_json::from_slice(payload).map_err(|_| SourceError::InvalidCursor)?;
            validate_cursor(&decoded)?;
            Ok(decoded)
        })
        .transpose()
}

pub(crate) fn encode_cursor(
    cursor: &LocalCursor,
    key: &[u8; 32],
) -> Result<SourceCursor, SourceError> {
    let mut payload = serde_json::to_vec(cursor).map_err(|_| SourceError::InvalidCursor)?;
    let tag = blake3::keyed_hash(key, &payload);
    payload.extend_from_slice(tag.as_bytes());
    SourceCursor::new(payload)
}

pub(crate) fn new_cursor_key() -> [u8; 32] {
    *blake3::hash(UuidBytes::new().as_slice()).as_bytes()
}

struct UuidBytes([u8; 32]);

impl UuidBytes {
    fn new() -> Self {
        let mut bytes = [0; 32];
        bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        Self(bytes)
    }

    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

fn validate_cursor(cursor: &LocalCursor) -> Result<(), SourceError> {
    let Some(walk) = &cursor.walk else {
        return Ok(());
    };
    if walk.frames.is_empty() || walk.frames.len() > MAX_WALK_DEPTH {
        return Err(SourceError::InvalidCursor);
    }
    for (index, frame) in walk.frames.iter().enumerate() {
        if index == 0 {
            if !frame.dir.is_empty() {
                return Err(SourceError::InvalidCursor);
            }
        } else if frame.dir.is_empty()
            || std::path::Path::new(&frame.dir)
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(SourceError::InvalidCursor);
        }
        if index > 0 {
            let parent = &walk.frames[index - 1];
            let branch = parent
                .after
                .as_deref()
                .and_then(|after| after.strip_suffix('/'))
                .ok_or(SourceError::InvalidCursor)?;
            let expected = Path::new(&parent.dir).join(branch);
            if Path::new(&frame.dir) != expected {
                return Err(SourceError::InvalidCursor);
            }
        }
        if let Some(after) = &frame.after {
            let trimmed = after.strip_suffix('/').unwrap_or(after);
            let mut components = Path::new(trimmed).components();
            if trimmed.is_empty()
                || !matches!(components.next(), Some(std::path::Component::Normal(_)))
                || components.next().is_some()
            {
                return Err(SourceError::InvalidCursor);
            }
        }
    }
    Ok(())
}

/// Return at most `limit` candidates in a stable total relative-path order.
///
/// The cursor retains the lexical position independently in every directory.
/// A directory is scanned to select only its single next branch; entries are
/// never collected or sorted as a whole. Fixed directory, depth, and path
/// bounds keep that scan finite.
pub(crate) fn walk_page(
    root: &ContainedRoot,
    prefix: &str,
    extensions: Option<&[&str]>,
    cursor: Option<WalkCursor>,
    limit: usize,
    context: &ReadContext,
) -> Result<WalkPage, WalkError> {
    let mut cursor = cursor.unwrap_or_default();
    let mut found = Vec::with_capacity(limit);
    while found.len() < limit {
        let Some(file) = next_file(root, prefix, extensions, &mut cursor, context)? else {
            break;
        };
        found.push(file);
    }
    let mut lookahead = cursor.clone();
    let has_more = next_file(root, prefix, extensions, &mut lookahead, context)?.is_some();
    Ok(WalkPage {
        files: found,
        has_more,
        cursor,
    })
}

fn next_file(
    root: &ContainedRoot,
    prefix: &str,
    extensions: Option<&[&str]>,
    cursor: &mut WalkCursor,
    context: &ReadContext,
) -> Result<Option<Walked>, WalkError> {
    loop {
        context.check()?;
        let Some(frame) = cursor.frames.last_mut() else {
            return Ok(None);
        };
        let selected = next_branch(root, frame, prefix, extensions, context)?;
        let Some(selected) = selected else {
            cursor.frames.pop();
            continue;
        };
        frame.after = Some(selected.branch_key);
        if selected.is_dir {
            if cursor.frames.len() >= MAX_WALK_DEPTH {
                return Err(WalkError::Limit("directory depth"));
            }
            cursor.frames.push(WalkFrame {
                dir: selected.rel,
                after: None,
            });
            continue;
        }
        return Ok(Some(Walked { rel: selected.rel }));
    }
}

struct SelectedBranch {
    branch_key: String,
    rel: String,
    is_dir: bool,
}

#[derive(Debug)]
struct ContainedDirEntry {
    name: OsString,
    kind: ContainedEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainedEntryKind {
    File,
    Directory,
    Other,
}

fn next_branch(
    root: &ContainedRoot,
    frame: &WalkFrame,
    prefix: &str,
    extensions: Option<&[&str]>,
    context: &ReadContext,
) -> Result<Option<SelectedBranch>, WalkError> {
    let mut selected: Option<SelectedBranch> = None;
    root.visit_dir(
        Path::new(&frame.dir),
        context,
        MAX_DIRECTORY_ENTRIES,
        |entry| consider_branch(root, frame, prefix, extensions, entry, &mut selected),
    )?;
    Ok(selected)
}

fn consider_branch(
    root: &ContainedRoot,
    frame: &WalkFrame,
    prefix: &str,
    extensions: Option<&[&str]>,
    entry: ContainedDirEntry,
    selected: &mut Option<SelectedBranch>,
) -> Result<(), WalkError> {
    let path = root.canonical().join(&frame.dir).join(&entry.name);
    let name = entry
        .name
        .to_str()
        .ok_or_else(|| WalkError::NonUtf8(path.clone()))?;
    if name.starts_with('.')
        || !matches!(
            entry.kind,
            ContainedEntryKind::Directory | ContainedEntryKind::File
        )
    {
        return Ok(());
    }
    let rel = if frame.dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{name}", frame.dir)
    };
    if rel.len() > MAX_RELATIVE_PATH_BYTES {
        return Err(WalkError::Limit("relative path length"));
    }
    let is_dir = entry.kind == ContainedEntryKind::Directory;
    let branch_key = if is_dir {
        format!("{name}/")
    } else {
        name.to_string()
    };
    if frame
        .after
        .as_ref()
        .is_some_and(|after| branch_key <= *after)
    {
        return Ok(());
    }
    if is_dir {
        let boundary = format!("{rel}/");
        if !prefix.starts_with(&boundary) && !boundary.starts_with(prefix) {
            return Ok(());
        }
    } else if !rel.starts_with(prefix)
        || extensions.is_some_and(|extensions| {
            !Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| extensions.contains(&ext))
        })
    {
        return Ok(());
    }
    if selected
        .as_ref()
        .is_none_or(|current| branch_key < current.branch_key)
    {
        *selected = Some(SelectedBranch {
            branch_key,
            rel,
            is_dir,
        });
    }
    Ok(())
}

/// An open directory descriptor that anchors every later file read to the
/// configured root object rather than to a path that can be swapped.
#[derive(Debug, Clone)]
pub(crate) struct ContainedRoot {
    canonical: PathBuf,
    identity: crate::LocalRootIdentity,
    path_bound: bool,
    platform: Arc<PlatformContainedRoot>,
}

impl ContainedRoot {
    pub(crate) fn open(root: &Path) -> Result<Self, WalkError> {
        let canonical = root.canonicalize()?;
        let platform = PlatformContainedRoot::open(&canonical)?;
        let identity = platform.identity()?;
        Ok(Self {
            canonical,
            identity,
            path_bound: true,
            platform: Arc::new(platform),
        })
    }

    pub(crate) fn from_open_directory(
        canonical: PathBuf,
        directory: std::fs::File,
    ) -> Result<Self, WalkError> {
        let platform = PlatformContainedRoot::from_open_directory(&canonical, directory)?;
        let identity = platform.identity()?;
        Ok(Self {
            canonical,
            identity,
            path_bound: cfg!(windows),
            platform: Arc::new(platform),
        })
    }

    pub(crate) fn canonical(&self) -> &Path {
        &self.canonical
    }

    pub(crate) fn identity(&self) -> &crate::LocalRootIdentity {
        &self.identity
    }

    pub(crate) fn verify_current(&self) -> Result<(), WalkError> {
        if !self.path_bound {
            return self.platform.inspect();
        }
        let current = Self::open(&self.canonical).map_err(|error| WalkError::Containment {
            path: self.canonical.clone(),
            detail: format!("configured root identity cannot be re-established: {error}"),
        })?;
        if current.canonical != self.canonical || current.identity != self.identity {
            return Err(WalkError::Containment {
                path: self.canonical.clone(),
                detail: "configured root physical identity changed".to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn inspect(&self) -> Result<(), WalkError> {
        self.platform.inspect()
    }

    pub(crate) fn open_file(&self, relative: &Path) -> Result<std::fs::File, WalkError> {
        self.open_file_with(relative, |_, _| {})
    }

    pub(crate) fn has_entry(&self, relative: &Path) -> Result<bool, WalkError> {
        let components = relative_components(relative)?;
        self.platform.has_entry(relative, &components)
    }

    fn visit_dir(
        &self,
        relative: &Path,
        context: &ReadContext,
        maximum_entries: usize,
        visitor: impl FnMut(ContainedDirEntry) -> Result<(), WalkError>,
    ) -> Result<(), WalkError> {
        self.visit_dir_with(
            relative,
            context,
            maximum_entries,
            |EnumerationStage::AfterAuthorityCheck, _| {},
            visitor,
        )
    }

    fn visit_dir_with(
        &self,
        relative: &Path,
        context: &ReadContext,
        maximum_entries: usize,
        mut stage: impl FnMut(EnumerationStage, &Path),
        mut visitor: impl FnMut(ContainedDirEntry) -> Result<(), WalkError>,
    ) -> Result<(), WalkError> {
        let components = relative_components_allow_empty(relative)?;
        self.verify_current()?;
        stage(EnumerationStage::AfterAuthorityCheck, relative);
        // A persistent substitution is a typed authority failure. If the name
        // is restored after this check, descriptor-relative enumeration still
        // remains bound to the originally admitted directory.
        self.verify_current()?;
        self.platform.visit_dir(
            relative,
            &components,
            context,
            maximum_entries,
            &mut visitor,
        )?;
        self.verify_current()?;
        Ok(())
    }

    pub(crate) fn configure_command(
        &self,
        command: &mut std::process::Command,
    ) -> Result<(), WalkError> {
        self.platform.configure_command(command, &self.canonical)
    }

    fn open_file_with(
        &self,
        relative: &Path,
        stage: impl FnMut(OpenStage, &Path),
    ) -> Result<std::fs::File, WalkError> {
        let components = relative_components(relative)?;
        self.platform.open_file(relative, &components, stage)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnumerationStage {
    AfterAuthorityCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenStage {
    BeforeComponent,
    AfterComponent,
}

fn relative_components(relative: &Path) -> Result<Vec<std::ffi::OsString>, WalkError> {
    let components = relative_components_allow_empty(relative)?;
    if components.is_empty() {
        return Err(WalkError::Containment {
            path: relative.to_path_buf(),
            detail: "contained opens require a non-empty normal relative path".to_string(),
        });
    }
    Ok(components)
}

fn relative_components_allow_empty(relative: &Path) -> Result<Vec<std::ffi::OsString>, WalkError> {
    let components: Vec<std::ffi::OsString> = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(component) => Ok(component.to_os_string()),
            _ => Err(WalkError::Containment {
                path: relative.to_path_buf(),
                detail: "contained opens require a non-empty normal relative path".to_string(),
            }),
        })
        .collect::<Result<_, _>>()?;
    Ok(components)
}

fn contained_component_error(
    relative: &Path,
    component: &std::ffi::OsStr,
    error: impl std::fmt::Display,
) -> WalkError {
    WalkError::Containment {
        path: relative.to_path_buf(),
        detail: format!(
            "descriptor-relative open refused component `{}`: {error}",
            component.to_string_lossy()
        ),
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct PlatformContainedRoot {
    descriptor: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl PlatformContainedRoot {
    fn open(canonical: &Path) -> Result<Self, WalkError> {
        let descriptor = rustix::fs::open(
            canonical,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| contained_component_error(canonical, canonical.as_os_str(), error))?;
        Ok(Self { descriptor })
    }

    fn from_open_directory(canonical: &Path, directory: std::fs::File) -> Result<Self, WalkError> {
        if !directory.metadata()?.is_dir() {
            return Err(WalkError::Containment {
                path: canonical.to_path_buf(),
                detail: "retained authority is not a directory".to_string(),
            });
        }
        Ok(Self {
            descriptor: directory.into(),
        })
    }

    fn inspect(&self) -> Result<(), WalkError> {
        rustix::fs::fstat(&self.descriptor)
            .map(|_| ())
            .map_err(|error| std::io::Error::from(error).into())
    }

    fn identity(&self) -> Result<crate::LocalRootIdentity, WalkError> {
        let stat = rustix::fs::fstat(&self.descriptor).map_err(std::io::Error::from)?;
        Ok(crate::LocalRootIdentity::Unix {
            device: u64::try_from(stat.st_dev).map_err(|_| WalkError::Containment {
                path: PathBuf::new(),
                detail: "root device identity is not representable".to_string(),
            })?,
            inode: stat.st_ino,
        })
    }

    fn open_directory(
        &self,
        relative: &Path,
        components: &[OsString],
    ) -> Result<std::os::fd::OwnedFd, WalkError> {
        let mut directory = rustix::fs::openat(
            &self.descriptor,
            ".",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| contained_component_error(relative, std::ffi::OsStr::new("."), error))?;
        for component in components {
            directory = rustix::fs::openat(
                &directory,
                component,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|error| contained_component_error(relative, component, error))?;
        }
        Ok(directory)
    }

    fn visit_dir(
        &self,
        relative: &Path,
        components: &[OsString],
        context: &ReadContext,
        maximum_entries: usize,
        visitor: &mut impl FnMut(ContainedDirEntry) -> Result<(), WalkError>,
    ) -> Result<(), WalkError> {
        use std::os::unix::ffi::OsStrExt as _;

        let directory = self.open_directory(relative, components)?;
        let mut stream = rustix::fs::Dir::read_from(&directory).map_err(std::io::Error::from)?;
        let mut entries = 0usize;
        for entry in &mut stream {
            context.check()?;
            if entries >= maximum_entries {
                return Err(WalkError::Limit("directory entry count"));
            }
            let entry = entry.map_err(std::io::Error::from)?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            let name = std::ffi::OsStr::from_bytes(bytes);
            let stat = rustix::fs::statat(&directory, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|error| contained_component_error(relative, name, error))?;
            let file_type = rustix::fs::FileType::from_raw_mode(stat.st_mode);
            entries = entries.saturating_add(1);
            visitor(ContainedDirEntry {
                name: name.to_os_string(),
                kind: if file_type.is_file() {
                    ContainedEntryKind::File
                } else if file_type.is_dir() {
                    ContainedEntryKind::Directory
                } else {
                    ContainedEntryKind::Other
                },
            })?;
        }
        Ok(())
    }

    fn open_file(
        &self,
        relative: &Path,
        components: &[std::ffi::OsString],
        mut stage: impl FnMut(OpenStage, &Path),
    ) -> Result<std::fs::File, WalkError> {
        let mut directory = rustix::fs::openat(
            &self.descriptor,
            ".",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let mut traversed = PathBuf::new();
        for (index, component) in components.iter().enumerate() {
            traversed.push(component);
            stage(OpenStage::BeforeComponent, &traversed);
            let last = index + 1 == components.len();
            let mut flags = rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC;
            if !last {
                flags |= rustix::fs::OFlags::DIRECTORY;
            }
            let opened =
                rustix::fs::openat(&directory, component, flags, rustix::fs::Mode::empty())
                    .map_err(|error| contained_component_error(relative, component, error))?;
            stage(OpenStage::AfterComponent, &traversed);
            if last {
                let opened = std::fs::File::from(opened);
                if !opened.metadata()?.is_file() {
                    return Err(WalkError::Containment {
                        path: relative.to_path_buf(),
                        detail: "descriptor-relative contained open did not resolve a regular file"
                            .to_string(),
                    });
                }
                return Ok(opened);
            }
            directory = opened;
        }
        Err(WalkError::Containment {
            path: relative.to_path_buf(),
            detail: "descriptor-relative contained open resolved no file".to_string(),
        })
    }

    fn has_entry(
        &self,
        relative: &Path,
        components: &[std::ffi::OsString],
    ) -> Result<bool, WalkError> {
        let (leaf, parents) = components
            .split_last()
            .ok_or_else(|| WalkError::Containment {
                path: relative.to_path_buf(),
                detail: "descriptor-relative presence check resolved no entry".to_string(),
            })?;
        let directory = self.open_directory(relative, parents)?;
        match rustix::fs::statat(&directory, leaf, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(contained_component_error(relative, leaf, error)),
        }
    }

    #[allow(unsafe_code)]
    fn configure_command(
        &self,
        command: &mut std::process::Command,
        _canonical: &Path,
    ) -> Result<(), WalkError> {
        use std::os::unix::process::CommandExt as _;

        let descriptor = rustix::io::dup(&self.descriptor).map_err(std::io::Error::from)?;
        rustix::io::fcntl_setfd(&descriptor, rustix::io::FdFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        // SAFETY: the child hook performs only the async-signal-safe fchdir
        // syscall. The duplicated descriptor is retained by the command until
        // spawn and is closed on exec.
        unsafe {
            command.pre_exec(move || {
                rustix::process::fchdir(&descriptor).map_err(std::io::Error::from)
            });
        }
        Ok(())
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct PlatformContainedRoot {
    canonical: PathBuf,
    descriptor: std::fs::File,
}

#[cfg(windows)]
impl PlatformContainedRoot {
    fn open(canonical: &Path) -> Result<Self, WalkError> {
        let descriptor = open_windows_component(canonical, true)?;
        Ok(Self {
            canonical: canonical.to_path_buf(),
            descriptor,
        })
    }

    fn from_open_directory(canonical: &Path, directory: std::fs::File) -> Result<Self, WalkError> {
        use std::os::windows::fs::MetadataExt as _;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(WalkError::Containment {
                path: canonical.to_path_buf(),
                detail: "retained authority is not a non-reparse directory".to_string(),
            });
        }
        Ok(Self {
            canonical: canonical.to_path_buf(),
            descriptor: directory,
        })
    }

    fn inspect(&self) -> Result<(), WalkError> {
        self.descriptor.metadata().map(|_| ()).map_err(Into::into)
    }

    #[allow(unsafe_code)]
    fn identity(&self) -> Result<crate::LocalRootIdentity, WalkError> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };

        // SAFETY: the retained directory file owns a valid HANDLE and the
        // output structure is live and exclusively writable for this call.
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: arguments satisfy GetFileInformationByHandle as documented.
        let succeeded = unsafe {
            GetFileInformationByHandle(self.descriptor.as_raw_handle(), &mut information)
        };
        if succeeded == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(crate::LocalRootIdentity::Windows {
            volume_serial_number: information.dwVolumeSerialNumber,
            file_index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        })
    }
    fn open_file(
        &self,
        relative: &Path,
        components: &[std::ffi::OsString],
        mut stage: impl FnMut(OpenStage, &Path),
    ) -> Result<std::fs::File, WalkError> {
        let mut anchors = Vec::with_capacity(components.len());
        anchors.push(self.descriptor.try_clone()?);
        let mut traversed = PathBuf::new();
        let mut absolute = self.canonical.clone();
        // Win32 does not expose `openat`. The retained root and intermediate
        // handles deny delete/write sharing, so their absolute names cannot be
        // renamed, replaced, or converted into reparse points while the next
        // component is opened and inspected through its own handle.
        for (index, component) in components.iter().enumerate() {
            traversed.push(component);
            absolute.push(component);
            stage(OpenStage::BeforeComponent, &traversed);
            let last = index + 1 == components.len();
            let opened = open_windows_component(&absolute, !last)
                .map_err(|error| contained_component_error(relative, component, error))?;
            stage(OpenStage::AfterComponent, &traversed);
            if last {
                return Ok(opened);
            }
            anchors.push(opened);
        }
        Err(WalkError::Containment {
            path: relative.to_path_buf(),
            detail: "descriptor-anchored contained open resolved no file".to_string(),
        })
    }

    fn has_entry(
        &self,
        relative: &Path,
        components: &[std::ffi::OsString],
    ) -> Result<bool, WalkError> {
        let mut anchors = Vec::with_capacity(components.len());
        anchors.push(self.descriptor.try_clone()?);
        let mut absolute = self.canonical.clone();
        for (index, component) in components.iter().enumerate() {
            absolute.push(component);
            match open_windows_any_component(&absolute) {
                Ok(opened) => anchors.push(opened),
                Err(WalkError::Io(error))
                    if index + 1 == components.len()
                        && error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(false);
                }
                Err(error) => {
                    return Err(contained_component_error(relative, component, error));
                }
            }
        }
        Ok(true)
    }

    #[allow(unsafe_code)]
    fn visit_dir(
        &self,
        relative: &Path,
        components: &[OsString],
        context: &ReadContext,
        maximum_entries: usize,
        visitor: &mut impl FnMut(ContainedDirEntry) -> Result<(), WalkError>,
    ) -> Result<(), WalkError> {
        use std::os::windows::ffi::OsStringExt as _;
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
        use windows_sys::Win32::Storage::FileSystem::{
            FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ID_BOTH_DIR_INFO,
        };

        let mut anchors = Vec::with_capacity(components.len().saturating_add(1));
        anchors.push(self.descriptor.try_clone()?);
        let mut absolute = self.canonical.clone();
        for component in components {
            absolute.push(component);
            let opened = open_windows_component(&absolute, true)
                .map_err(|error| contained_component_error(relative, component, error))?;
            anchors.push(opened);
        }
        let directory = anchors.last().ok_or_else(|| WalkError::Containment {
            path: relative.to_path_buf(),
            detail: "descriptor-anchored directory resolved no handle".to_string(),
        })?;
        const BUFFER_BYTES: usize = 64 * 1024;
        let words = BUFFER_BYTES.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; words];
        let mut restart = true;
        let mut entries = 0usize;
        loop {
            context.check()?;
            let class = if restart {
                FileIdBothDirectoryRestartInfo
            } else {
                FileIdBothDirectoryInfo
            };
            restart = false;
            // SAFETY: `directory` owns a live directory HANDLE and `storage`
            // is an aligned writable region of BUFFER_BYTES.
            let succeeded = unsafe {
                GetFileInformationByHandleEx(
                    directory.as_raw_handle(),
                    class,
                    storage.as_mut_ptr().cast(),
                    BUFFER_BYTES as u32,
                )
            };
            if succeeded == 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    break;
                }
                return Err(error.into());
            }
            let mut offset = 0usize;
            loop {
                context.check()?;
                let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                if offset
                    .checked_add(header)
                    .is_none_or(|end| end > BUFFER_BYTES)
                {
                    return Err(WalkError::Containment {
                        path: relative.to_path_buf(),
                        detail: "Windows returned malformed directory information".to_string(),
                    });
                }
                // SAFETY: the bounds check establishes a complete fixed
                // FILE_ID_BOTH_DIR_INFO header at this aligned record offset.
                let information = unsafe {
                    &*storage
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_ID_BOTH_DIR_INFO>()
                };
                let name_bytes = information.FileNameLength as usize;
                if name_bytes % std::mem::size_of::<u16>() != 0
                    || offset
                        .checked_add(header)
                        .and_then(|start| start.checked_add(name_bytes))
                        .is_none_or(|end| end > BUFFER_BYTES)
                {
                    return Err(WalkError::Containment {
                        path: relative.to_path_buf(),
                        detail: "Windows returned malformed directory filename data".to_string(),
                    });
                }
                // SAFETY: FileNameLength was validated against the backing
                // buffer and is an even number of bytes.
                let name = unsafe {
                    let wide = std::slice::from_raw_parts(
                        std::ptr::addr_of!(information.FileName).cast::<u16>(),
                        name_bytes / std::mem::size_of::<u16>(),
                    );
                    OsString::from_wide(wide)
                };
                if name != "." && name != ".." {
                    if entries >= maximum_entries {
                        return Err(WalkError::Limit("directory entry count"));
                    }
                    let attributes = information.FileAttributes;
                    entries = entries.saturating_add(1);
                    visitor(ContainedDirEntry {
                        name,
                        kind: if attributes & 0x0000_0400 != 0 {
                            ContainedEntryKind::Other
                        } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                            ContainedEntryKind::Directory
                        } else {
                            ContainedEntryKind::File
                        },
                    })?;
                }
                if information.NextEntryOffset == 0 {
                    break;
                }
                let next = information.NextEntryOffset as usize;
                if next < header
                    || offset
                        .checked_add(next)
                        .is_none_or(|end| end >= BUFFER_BYTES)
                {
                    return Err(WalkError::Containment {
                        path: relative.to_path_buf(),
                        detail: "Windows returned an invalid directory record offset".to_string(),
                    });
                }
                offset += next;
            }
        }
        Ok(())
    }

    fn configure_command(
        &self,
        command: &mut std::process::Command,
        canonical: &Path,
    ) -> Result<(), WalkError> {
        command.current_dir(canonical);
        Ok(())
    }
}

#[cfg(windows)]
fn open_windows_component(path: &Path, directory: bool) -> Result<std::fs::File, WalkError> {
    let opened = open_windows_any_component(path)?;
    let metadata = opened.metadata()?;
    if directory != metadata.is_dir() {
        return Err(WalkError::Containment {
            path: path.to_path_buf(),
            detail: if directory {
                "Windows contained-open path component is not a directory".to_string()
            } else {
                "Windows contained-open leaf is not a regular file".to_string()
            },
        });
    }
    Ok(opened)
}

#[cfg(windows)]
fn open_windows_any_component(path: &Path) -> Result<std::fs::File, WalkError> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;

    let opened = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = opened.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(WalkError::Containment {
            path: path.to_path_buf(),
            detail: "Windows contained opens refuse reparse points".to_string(),
        });
    }
    Ok(opened)
}

#[cfg(not(any(unix, windows)))]
#[derive(Debug)]
struct PlatformContainedRoot;

#[cfg(not(any(unix, windows)))]
impl PlatformContainedRoot {
    fn open(canonical: &Path) -> Result<Self, WalkError> {
        Err(WalkError::Containment {
            path: canonical.to_path_buf(),
            detail: "descriptor-relative contained opens are unsupported on this platform"
                .to_string(),
        })
    }

    fn from_open_directory(canonical: &Path, _directory: std::fs::File) -> Result<Self, WalkError> {
        Err(WalkError::Containment {
            path: canonical.to_path_buf(),
            detail: "descriptor-relative contained opens are unsupported on this platform"
                .to_string(),
        })
    }

    fn inspect(&self) -> Result<(), WalkError> {
        Err(WalkError::Containment {
            path: PathBuf::new(),
            detail: "descriptor-relative root inspection is unsupported on this platform"
                .to_string(),
        })
    }

    fn identity(&self) -> Result<crate::LocalRootIdentity, WalkError> {
        Err(WalkError::Containment {
            path: PathBuf::new(),
            detail: "physical root identity is unsupported on this platform".to_string(),
        })
    }

    fn open_file(
        &self,
        relative: &Path,
        _components: &[std::ffi::OsString],
        _stage: impl FnMut(OpenStage, &Path),
    ) -> Result<std::fs::File, WalkError> {
        Err(WalkError::Containment {
            path: relative.to_path_buf(),
            detail: "descriptor-relative contained opens are unsupported on this platform"
                .to_string(),
        })
    }

    fn has_entry(
        &self,
        relative: &Path,
        _components: &[std::ffi::OsString],
    ) -> Result<bool, WalkError> {
        Err(WalkError::Containment {
            path: relative.to_path_buf(),
            detail: "descriptor-relative presence checks are unsupported on this platform"
                .to_string(),
        })
    }

    fn visit_dir(
        &self,
        relative: &Path,
        _components: &[OsString],
        _context: &ReadContext,
        _maximum_entries: usize,
        _visitor: &mut impl FnMut(ContainedDirEntry) -> Result<(), WalkError>,
    ) -> Result<(), WalkError> {
        Err(WalkError::Containment {
            path: relative.to_path_buf(),
            detail: "descriptor-relative directory reads are unsupported on this platform"
                .to_string(),
        })
    }

    fn configure_command(
        &self,
        command: &mut std::process::Command,
        _canonical: &Path,
    ) -> Result<(), WalkError> {
        let _ = command;
        Err(WalkError::Containment {
            path: PathBuf::new(),
            detail: "descriptor-bound commands are unsupported on this platform".to_string(),
        })
    }
}

#[cfg(test)]
fn descend(
    root: &Path,
    dir: &Path,
    prefix: &str,
    extensions: Option<&[&str]>,
    found: &mut Vec<Walked>,
) -> Result<(), WalkError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| WalkError::NonUtf8(path.clone()))?;
        if name.starts_with('.') {
            continue;
        }
        let rel = relative(root, &path)?;
        // `file_type` does not follow symlinks, so a link is neither a file nor
        // a directory here and is skipped: following one would let a source
        // read outside its own root, and a cycle would not terminate.
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let boundary = format!("{rel}/");
            // Descend only where a match is still possible: either this
            // directory is on the way to the prefix, or it is inside it.
            if prefix.starts_with(&boundary) || boundary.starts_with(prefix) {
                descend(root, &path, prefix, extensions, found)?;
            }
        } else if kind.is_file() && rel.starts_with(prefix) {
            if let Some(extensions) = extensions {
                let matches = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| extensions.contains(&ext));
                if !matches {
                    continue;
                }
            }
            found.push(Walked { rel });
        }
    }
    Ok(())
}

/// A path relative to the root, with `/` separators on every platform, so a
/// key means the same thing wherever it was read.
#[cfg(test)]
fn relative(root: &Path, path: &Path) -> Result<String, WalkError> {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| WalkError::NonUtf8(path.to_path_buf()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        for rel in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, "x").expect("write");
        }
        dir
    }

    fn rels(root: &Path, prefix: &str, extensions: Option<&[&str]>) -> Vec<String> {
        walk(root, prefix, extensions)
            .expect("walk")
            .into_iter()
            .map(|w| w.rel)
            .collect()
    }

    #[test]
    fn a_prefix_prunes_directories_it_cannot_match() {
        let dir = tree(&["docs/a.md", "docs/deep/b.md", "notes/c.md"]);
        assert_eq!(
            rels(dir.path(), "", None),
            ["docs/a.md", "docs/deep/b.md", "notes/c.md"]
        );
        assert_eq!(
            rels(dir.path(), "docs/", None),
            ["docs/a.md", "docs/deep/b.md"]
        );
        // A partial segment still reaches the right directory.
        assert_eq!(rels(dir.path(), "docs/d", None), ["docs/deep/b.md"]);
        assert_eq!(rels(dir.path(), "nothing/", None).len(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn distinct_non_utf8_native_names_are_never_lossily_merged() {
        use std::os::unix::ffi::OsStringExt as _;

        let root = Path::new("/root");
        let first = root.join(std::ffi::OsString::from_vec(vec![b'a', 0xfe]));
        let second = root.join(std::ffi::OsString::from_vec(vec![b'a', 0xff]));
        assert_eq!(
            first.to_string_lossy(),
            second.to_string_lossy(),
            "fixture demonstrates the lossy-key collision"
        );
        assert!(matches!(
            relative(root, &first),
            Err(WalkError::NonUtf8(path)) if path == first
        ));
        assert!(matches!(
            relative(root, &second),
            Err(WalkError::NonUtf8(path)) if path == second
        ));
    }

    #[test]
    fn extensions_restrict_the_walk() {
        let dir = tree(&["a.md", "b.markdown", "c.png", "d"]);
        assert_eq!(
            rels(dir.path(), "", Some(&["md", "markdown"])),
            ["a.md", "b.markdown"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn paged_walk_enumerates_the_retained_root_after_a_path_replacement() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let replacement = parent.path().join("replacement");
        let retired = parent.path().join("retired");
        std::fs::create_dir(&root).expect("root");
        std::fs::create_dir(&replacement).expect("replacement");
        std::fs::write(root.join("original.md"), "original").expect("original");
        std::fs::write(replacement.join("replacement.md"), "replacement").expect("replacement");
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .open(&root)
            .expect("open root directory");
        let contained =
            ContainedRoot::from_open_directory(root.clone(), directory).expect("contained root");

        std::fs::rename(&root, &retired).expect("retire root");
        std::fs::rename(&replacement, &root).expect("install replacement");
        let context = ReadContext::new(
            uuid::Uuid::new_v4(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            100,
            1024 * 1024,
            100,
            crate::CancellationToken::new(),
        )
        .expect("bounded context");
        let page =
            walk_page(&contained, "", None, None, 100, &context).expect("identity-bound walk");

        assert_eq!(
            page.files,
            [Walked {
                rel: "original.md".to_string(),
            }]
        );
    }

    #[test]
    fn hidden_entries_are_a_declared_scope() {
        let dir = tree(&["a.md", ".hidden.md", ".git/objects/x"]);
        assert_eq!(rels(dir.path(), "", None), ["a.md"]);
    }

    #[test]
    fn a_signed_walk_cursor_still_has_to_describe_one_connected_frame_chain() {
        let key = new_cursor_key();
        let disconnected = LocalCursor {
            after: None,
            walk: Some(WalkCursor {
                frames: vec![
                    WalkFrame {
                        dir: String::new(),
                        after: Some("docs/".to_owned()),
                    },
                    WalkFrame {
                        dir: "other".to_owned(),
                        after: None,
                    },
                ],
            }),
            snapshot: None,
        };
        let signed = encode_cursor(&disconnected, &key).expect("sign fixture");

        assert!(matches!(
            decode_cursor(Some(&signed), &key),
            Err(SourceError::InvalidCursor)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_discovered_file_replaced_by_an_external_symlink_is_never_opened() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        let lexical = root.join("document.md");
        std::fs::write(&lexical, "inside").expect("inside");
        let contained = ContainedRoot::open(&root).expect("contained root");
        std::fs::remove_file(&lexical).expect("replace inside file");
        std::fs::write(outside.join("secret.md"), "outside").expect("outside secret");
        symlink(outside.join("secret.md"), &lexical).expect("escape symlink");

        assert!(matches!(
            contained.open_file(Path::new("document.md")),
            Err(WalkError::Containment { ref detail, .. }) if !detail.is_empty()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_intermediate_external_symlink_is_never_opened() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("document.md"), "outside").expect("outside secret");
        let contained = ContainedRoot::open(&root).expect("contained root");
        symlink(&outside, root.join("docs")).expect("escape directory symlink");

        assert!(matches!(
            contained.open_file(Path::new("docs/document.md")),
            Err(WalkError::Containment { ref detail, .. }) if !detail.is_empty()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_leaf_aba_swap_cannot_cross_the_open_containment_boundary() {
        use std::io::Read as _;
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        let lexical = root.join("document.md");
        let parked = root.join("document.parked");
        let secret = outside.join("secret.md");
        std::fs::write(&lexical, "inside").expect("inside");
        std::fs::write(&secret, "outside secret").expect("outside secret");
        let contained = ContainedRoot::open(&root).expect("contained root");

        let mut swapped = false;
        let mut result = contained.open_file_with(Path::new("document.md"), |stage, traversed| {
            if !swapped
                && stage == OpenStage::BeforeComponent
                && traversed == Path::new("document.md")
            {
                swapped = true;
                std::fs::rename(&lexical, &parked).expect("park inside leaf");
                symlink(&secret, &lexical).expect("install escape leaf");
            }
        });
        let leaked = match &mut result {
            Ok(opened) => {
                let mut text = String::new();
                opened
                    .read_to_string(&mut text)
                    .expect("read opened handle");
                Some(text)
            }
            Err(_) => None,
        };
        if std::fs::symlink_metadata(&lexical)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            std::fs::remove_file(&lexical).expect("remove test symlink");
        }
        if parked.exists() {
            std::fs::rename(&parked, &lexical).expect("restore parked leaf");
        }

        assert!(
            matches!(
                result,
                Err(WalkError::Containment { ref detail, .. }) if !detail.is_empty()
            ),
            "containment race opened bytes before proof: {leaked:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_intermediate_aba_swap_cannot_cross_the_open_containment_boundary() {
        use std::io::Read as _;
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        let docs = root.join("docs");
        let parked = root.join("docs.parked");
        std::fs::create_dir_all(&docs).expect("inside docs");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(docs.join("document.md"), "inside").expect("inside");
        std::fs::write(outside.join("document.md"), "outside secret").expect("outside secret");
        let contained = ContainedRoot::open(&root).expect("contained root");

        let mut swapped = false;
        let mut result =
            contained.open_file_with(Path::new("docs/document.md"), |stage, traversed| {
                if !swapped && stage == OpenStage::BeforeComponent && traversed == Path::new("docs")
                {
                    swapped = true;
                    std::fs::rename(&docs, &parked).expect("park inside directory");
                    symlink(&outside, &docs).expect("install escape directory");
                }
            });
        let leaked = match &mut result {
            Ok(opened) => {
                let mut text = String::new();
                opened
                    .read_to_string(&mut text)
                    .expect("read opened handle");
                Some(text)
            }
            Err(_) => None,
        };
        if std::fs::symlink_metadata(&docs).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            std::fs::remove_file(&docs).expect("remove test symlink");
        }
        if parked.exists() {
            std::fs::rename(&parked, &docs).expect("restore parked directory");
        }

        assert!(
            matches!(
                result,
                Err(WalkError::Containment { ref detail, .. }) if !detail.is_empty()
            ),
            "containment race opened bytes before proof: {leaked:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_opened_intermediate_descriptor_cannot_be_redirected_by_a_later_swap() {
        use std::io::Read as _;
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        let docs = root.join("docs");
        let parked = root.join("docs.parked");
        std::fs::create_dir_all(&docs).expect("inside docs");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(docs.join("document.md"), "inside").expect("inside");
        std::fs::write(outside.join("document.md"), "outside secret").expect("outside secret");
        let contained = ContainedRoot::open(&root).expect("contained root");

        let mut swapped = false;
        let mut opened = contained
            .open_file_with(Path::new("docs/document.md"), |stage, traversed| {
                if !swapped && stage == OpenStage::AfterComponent && traversed == Path::new("docs")
                {
                    swapped = true;
                    std::fs::rename(&docs, &parked).expect("park opened directory");
                    symlink(&outside, &docs).expect("redirect lexical directory");
                }
            })
            .expect("opened descriptor remains inside the anchored directory");
        let mut text = String::new();
        opened
            .read_to_string(&mut text)
            .expect("read anchored handle");
        std::fs::remove_file(&docs).expect("remove redirect");
        std::fs::rename(&parked, &docs).expect("restore docs");

        assert_eq!(text, "inside");
    }

    #[cfg(unix)]
    #[test]
    fn root_substitution_after_the_source_precheck_is_not_successful_emptiness() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let parked = parent.path().join("parked");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("document.md"), "inside").expect("inside");
        let contained = ContainedRoot::open(&root).expect("contained root");
        contained.verify_current().expect("source precheck");

        std::fs::rename(&root, &parked).expect("park admitted root");
        std::fs::create_dir(&root).expect("install empty replacement");
        let context = ReadContext::new(
            uuid::Uuid::new_v4(),
            std::time::Instant::now() + std::time::Duration::from_secs(2),
            100,
            1024 * 1024,
            10,
            crate::CancellationToken::new(),
        )
        .expect("context");
        let result = walk_page(&contained, "", None, None, 10, &context);
        std::fs::remove_dir(&root).expect("remove replacement");
        std::fs::rename(&parked, &root).expect("restore admitted root");

        assert!(
            matches!(result, Err(WalkError::Containment { .. })),
            "a replaced source root must be a typed authority failure, not {result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retained_root_presence_does_not_follow_a_replacement_path() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let parked = parent.path().join("parked");
        std::fs::create_dir_all(root.join(".git")).expect("repository marker");
        let contained = ContainedRoot::open(&root).expect("contained root");

        std::fs::rename(&root, &parked).expect("park admitted root");
        std::fs::create_dir(&root).expect("install replacement without marker");
        let present = contained
            .has_entry(Path::new(".git"))
            .expect("query retained root");
        std::fs::remove_dir(&root).expect("remove replacement");
        std::fs::rename(&parked, &root).expect("restore admitted root");

        assert!(
            present,
            "repository presence must be read from the retained root, not its replaceable name"
        );
    }

    #[cfg(unix)]
    #[test]
    fn enumeration_interleaving_hook_refuses_a_postcheck_root_substitution() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("root");
        let parked = parent.path().join("parked");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("document.md"), "inside").expect("inside");
        let contained = ContainedRoot::open(&root).expect("contained root");
        let context = ReadContext::new(
            uuid::Uuid::new_v4(),
            std::time::Instant::now() + std::time::Duration::from_secs(2),
            100,
            1024 * 1024,
            10,
            crate::CancellationToken::new(),
        )
        .expect("context");

        let result = contained.visit_dir_with(
            Path::new(""),
            &context,
            MAX_DIRECTORY_ENTRIES,
            |stage, relative| {
                assert_eq!(stage, EnumerationStage::AfterAuthorityCheck);
                assert_eq!(relative, Path::new(""));
                std::fs::rename(&root, &parked).expect("park admitted root");
                std::fs::create_dir(&root).expect("install empty replacement");
            },
            |_| Ok(()),
        );
        std::fs::remove_dir(&root).expect("remove replacement");
        std::fs::rename(&parked, &root).expect("restore admitted root");

        assert!(
            matches!(result, Err(WalkError::Containment { .. })),
            "a swap after the internal precheck must be a typed authority failure, not {result:?}"
        );
    }
}
