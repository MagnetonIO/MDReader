//! Stable filesystem-object identity for local library roots.
//!
//! A canonical path is only a name. The directory behind that name can be
//! renamed away and replaced without changing the path string, so every active
//! source keeps the identity captured from an opened directory handle and
//! revalidates it before using the root again.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub mod file_stamp;
#[cfg(unix)]
mod read_symlink;
mod remove_directory;
#[cfg(unix)]
pub use read_symlink::SourceSymlinkSnapshot;

#[cfg(any(windows, test))]
const WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// The stable identity of the filesystem object backing a local source root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "platform", rename_all = "kebab-case")]
pub enum SourceRootIdentity {
    /// Unix filesystem device and inode, obtained from an opened directory.
    Unix {
        /// Filesystem device number.
        device: u64,
        /// Inode number within `device`.
        inode: u64,
    },
    /// Windows volume serial number and 128-bit file ID, obtained from an opened
    /// directory handle.
    Windows {
        /// Serial number of the containing volume.
        volume_serial_number: u64,
        /// Stable 128-bit file identifier within the volume.
        file_id: [u8; 16],
    },
}

/// A native canonical path bound to the filesystem object admitted there.
#[derive(Clone)]
pub struct SourceRoot {
    /// Canonical native path used to address the root.
    pub path: PathBuf,
    /// Stable identity of the directory object at `path`.
    pub identity: SourceRootIdentity,
    handle: Arc<fs::File>,
}

impl std::fmt::Debug for SourceRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SourceRoot")
            .field("authority", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl PartialEq for SourceRoot {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.identity == other.identity
    }
}

impl Eq for SourceRoot {}

/// Bytes and handle-derived metadata read through a retained source authority.
#[derive(Debug)]
pub struct SourceRead {
    /// Exact bytes read from the opened filesystem object.
    pub bytes: Vec<u8>,
    /// Modification time reported by that same opened object.
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceEntryKind {
    File,
    Directory,
    SymlinkOrReparse,
    Other,
}

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub name: OsString,
    pub kind: SourceEntryKind,
    pub size: u64,
}

impl SourceRoot {
    /// Bind an already-canonical directory path to its opened object identity.
    pub fn capture(path: PathBuf) -> Result<Self, SourceRootError> {
        let (identity, handle) = capture_authority(&path)?;
        Ok(Self {
            path,
            identity,
            handle: Arc::new(handle),
        })
    }

    /// Reopen a persisted path and retain it only when it still has `identity`.
    pub fn restore(path: PathBuf, identity: SourceRootIdentity) -> Result<Self, SourceRootError> {
        let observed = Self::capture(path.clone())?;
        identity.verify_observed(&path, &observed.identity)?;
        Ok(Self {
            path,
            identity,
            handle: observed.handle,
        })
    }

    /// Revalidate both the path shape and the opened object identity.
    pub fn verify(&self) -> Result<(), SourceRootError> {
        self.identity.verify(&self.path)
    }

    /// Duplicate the retained directory handle without resolving `path`.
    ///
    /// The duplicate names the same admitted filesystem object and can be
    /// handed to another component without creating a pathname race.
    pub fn try_clone_directory_handle(&self) -> Result<fs::File, SourceRootError> {
        self.handle
            .try_clone()
            .map_err(|error| SourceRootError::IdentityUnavailable {
                path: self.path.clone(),
                reason: error.to_string(),
            })
    }

    /// Convert an exact native path below this root into a traversal-free
    /// source-relative path without re-resolving filesystem names.
    pub fn relative_path(&self, candidate: &Path) -> Result<PathBuf, SourceRootError> {
        let relative =
            candidate
                .strip_prefix(&self.path)
                .map_err(|_| SourceRootError::InvalidRelative {
                    path: candidate.to_path_buf(),
                })?;
        validate_relative(relative, false)?;
        Ok(relative.to_path_buf())
    }

    /// Read a source-relative regular file through the retained root handle.
    pub fn read_relative(&self, relative: &Path) -> Result<SourceRead, SourceRootError> {
        let mut file = self.open_file_relative(relative)?;
        let metadata = file
            .metadata()
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: error.to_string(),
            })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: error.to_string(),
            })?;
        Ok(SourceRead {
            bytes,
            modified: metadata.modified().ok(),
        })
    }

    /// Confirm a source-relative directory can be opened beneath the retained
    /// root without following any symlink or reparse component.
    pub fn verify_directory_relative(&self, relative: &Path) -> Result<(), SourceRootError> {
        validate_relative(relative, true)?;

        #[cfg(unix)]
        {
            open_unix_relative(self, relative, true).map(|_| ())
        }

        #[cfg(windows)]
        {
            open_windows_relative(self, relative, true).map(|_| ())
        }

        #[cfg(not(any(unix, windows)))]
        {
            self.verify()?;
            let path = self.path.join(relative);
            if !path.is_dir() {
                return Err(SourceRootError::NotDirectory { path });
            }
            self.verify()
        }
    }

    /// Create or open one source-relative directory without following a
    /// symlink/reparse component, then retain the opened child's identity.
    ///
    /// Creation and opening are performed relative to the already-retained
    /// parent handle. A competing creator may win the initially absent name,
    /// but it cannot redirect the operation outside `self`; the exact real
    /// directory opened at that name becomes the returned authority.
    pub fn create_or_capture_directory_relative(
        &self,
        relative: &Path,
    ) -> Result<Self, SourceRootError> {
        validate_relative(relative, false)?;
        self.verify()?;

        #[cfg(unix)]
        let opened = {
            use rustix::fs::{mkdirat, Mode};

            let (parent, name) = open_unix_parent(self, relative)?;
            match mkdirat(&parent, &name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
                Ok(()) => {}
                Err(error) if error == rustix::io::Errno::EXIST => {}
                Err(error) => {
                    return Err(unix_source_error(self.path.join(relative), error));
                }
            }
            open_unix_relative(self, relative, true)?
        };

        #[cfg(windows)]
        let opened = {
            use windows_sys::Wdk::Storage::FileSystem::{
                FILE_DIRECTORY_FILE, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
                FILE_SYNCHRONOUS_IO_NONALERT,
            };
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_GENERIC_READ, FILE_READ_ATTRIBUTES, SYNCHRONIZE,
            };

            let (parent, name) = open_windows_parent(self, relative)?;
            open_windows_child(
                self,
                &parent,
                &name,
                FILE_GENERIC_READ | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_OPEN_IF,
                FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                relative,
            )?
        };

        #[cfg(not(any(unix, windows)))]
        {
            let _ = relative;
            return Err(SourceRootError::UnsupportedPlatform);
        }

        self.verify()?;
        let child = source_root_from_opened_directory(self.path.join(relative), opened)?;
        child.verify()?;
        Ok(child)
    }

    /// Read metadata for a regular file through the retained source authority.
    pub fn metadata_relative(&self, relative: &Path) -> Result<fs::Metadata, SourceRootError> {
        self.open_file_relative(relative)?
            .metadata()
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: error.to_string(),
            })
    }

    /// Open an existing source-relative regular file, returning `None` only
    /// when its name is absent. Symlinks, reparse points, directories, and
    /// replacement of the retained root are typed refusals.
    pub fn open_optional_file_relative(
        &self,
        relative: &Path,
    ) -> Result<Option<fs::File>, SourceRootError> {
        validate_relative(relative, false)?;
        self.verify()?;
        let path = self.path.join(relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(SourceRootError::CannotInspect {
                    path,
                    reason: error.to_string(),
                })
            }
        };
        #[cfg(unix)]
        if metadata.file_type().is_symlink() {
            return Err(SourceRootError::SymlinkOrReparse { path });
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if is_windows_reparse_point(metadata.file_attributes()) {
                return Err(SourceRootError::SymlinkOrReparse { path });
            }
        }
        if !metadata.is_file() {
            return Err(SourceRootError::CannotInspect {
                path,
                reason: "not a regular file".to_string(),
            });
        }
        let file = self.open_file_relative(relative)?;
        self.verify()?;
        Ok(Some(file))
    }

    /// Read metadata from an existing source-relative regular file without
    /// following a symlink or Windows reparse point.
    pub fn metadata_optional_relative(
        &self,
        relative: &Path,
    ) -> Result<Option<fs::Metadata>, SourceRootError> {
        self.open_optional_file_relative(relative)?
            .map(|file| {
                file.metadata()
                    .map_err(|error| SourceRootError::CannotInspect {
                        path: self.path.join(relative),
                        reason: error.to_string(),
                    })
            })
            .transpose()
    }

    /// Open or create a source-relative regular file for append without
    /// following a symlink or Windows reparse point.
    pub fn open_append_relative(&self, relative: &Path) -> Result<fs::File, SourceRootError> {
        validate_relative(relative, false)?;
        self.verify()?;

        #[cfg(unix)]
        let file = {
            use rustix::fs::{openat, Mode, OFlags};

            let (parent, name) = open_unix_parent(self, relative)?;
            let fd = openat(
                &parent,
                &name,
                OFlags::WRONLY
                    | OFlags::APPEND
                    | OFlags::CREATE
                    | OFlags::CLOEXEC
                    | OFlags::NOFOLLOW,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: error.to_string(),
            })?;
            fs::File::from(fd)
        };

        #[cfg(windows)]
        let file = {
            use windows_sys::Wdk::Storage::FileSystem::{
                FILE_NON_DIRECTORY_FILE, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
                FILE_SYNCHRONOUS_IO_NONALERT,
            };
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_APPEND_DATA, FILE_READ_ATTRIBUTES, SYNCHRONIZE,
            };

            let (parent, name) = open_windows_parent(self, relative)?;
            open_windows_child(
                self,
                &parent,
                &name,
                FILE_APPEND_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_OPEN_IF,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                relative,
            )?
        };

        #[cfg(not(any(unix, windows)))]
        let file = {
            let path = self.path.join(relative);
            let metadata = fs::symlink_metadata(&path).ok();
            if metadata
                .as_ref()
                .is_some_and(|metadata| !metadata.is_file())
            {
                return Err(SourceRootError::CannotInspect {
                    path,
                    reason: "not a regular file".to_string(),
                });
            }
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| SourceRootError::CannotInspect {
                    path,
                    reason: error.to_string(),
                })?
        };

        let metadata = file
            .metadata()
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: error.to_string(),
            })?;
        if !metadata.is_file() {
            return Err(SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: "not a regular file".to_string(),
            });
        }
        self.verify()?;
        Ok(file)
    }

    /// Create a new source-relative regular file without following symlinks.
    pub fn create_new_relative(&self, relative: &Path) -> Result<fs::File, SourceRootError> {
        validate_relative(relative, false)?;

        #[cfg(unix)]
        {
            use rustix::fs::{openat, Mode, OFlags};

            let (parent, name) = open_unix_parent(self, relative)?;
            let fd = openat(
                &parent,
                &name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: error.to_string(),
            })?;
            Ok(fs::File::from(fd))
        }

        #[cfg(windows)]
        {
            use windows_sys::Wdk::Storage::FileSystem::{
                FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
                FILE_SYNCHRONOUS_IO_NONALERT,
            };
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, SYNCHRONIZE,
            };

            let (parent, name) = open_windows_parent(self, relative)?;
            open_windows_child(
                self,
                &parent,
                &name,
                FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_CREATE,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                relative,
            )
        }

        #[cfg(not(any(unix, windows)))]
        {
            self.verify()?;
            let path = self.path.join(relative);
            let file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| SourceRootError::CannotInspect {
                    path,
                    reason: error.to_string(),
                })?;
            self.verify()?;
            Ok(file)
        }
    }

    /// Open or create a stable source-relative sidecar for advisory locking.
    pub fn open_lock_relative(&self, relative: &Path) -> Result<fs::File, SourceRootError> {
        validate_relative(relative, false)?;

        #[cfg(unix)]
        {
            use rustix::fs::{openat, Mode, OFlags};

            let (parent, name) = open_unix_parent(self, relative)?;
            let fd = openat(
                &parent,
                &name,
                OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(relative),
                reason: error.to_string(),
            })?;
            Ok(fs::File::from(fd))
        }

        #[cfg(windows)]
        {
            use windows_sys::Wdk::Storage::FileSystem::{
                FILE_NON_DIRECTORY_FILE, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
                FILE_SYNCHRONOUS_IO_NONALERT,
            };
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_GENERIC_READ, FILE_GENERIC_WRITE, SYNCHRONIZE,
            };

            let (parent, name) = open_windows_parent(self, relative)?;
            open_windows_child(
                self,
                &parent,
                &name,
                FILE_GENERIC_READ | FILE_GENERIC_WRITE | SYNCHRONIZE,
                FILE_OPEN_IF,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                relative,
            )
        }

        #[cfg(not(any(unix, windows)))]
        {
            self.verify()?;
            let path = self.path.join(relative);
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|error| SourceRootError::CannotInspect {
                    path,
                    reason: error.to_string(),
                })?;
            self.verify()?;
            Ok(file)
        }
    }

    /// Atomically rename one source-relative entry over another.
    pub fn rename_relative(&self, from: &Path, to: &Path) -> Result<(), SourceRootError> {
        validate_relative(from, false)?;
        validate_relative(to, false)?;

        #[cfg(unix)]
        {
            let (from_parent, from_name) = open_unix_parent(self, from)?;
            let (to_parent, to_name) = open_unix_parent(self, to)?;
            rustix::fs::renameat(&from_parent, &from_name, &to_parent, &to_name).map_err(|error| {
                SourceRootError::CannotInspect {
                    path: self.path.join(to),
                    reason: error.to_string(),
                }
            })
        }

        #[cfg(windows)]
        {
            rename_windows_relative(self, from, to)
        }

        #[cfg(not(any(unix, windows)))]
        {
            self.verify()?;
            fs::rename(self.path.join(from), self.path.join(to)).map_err(|error| {
                SourceRootError::CannotInspect {
                    path: self.path.join(to),
                    reason: error.to_string(),
                }
            })?;
            self.verify()
        }
    }

    /// Remove one source-relative regular file without following it.
    pub fn remove_file_relative(&self, relative: &Path) -> Result<(), SourceRootError> {
        validate_relative(relative, false)?;

        #[cfg(unix)]
        {
            let (parent, name) = open_unix_parent(self, relative)?;
            rustix::fs::unlinkat(&parent, &name, rustix::fs::AtFlags::empty()).map_err(|error| {
                SourceRootError::CannotInspect {
                    path: self.path.join(relative),
                    reason: error.to_string(),
                }
            })
        }

        #[cfg(windows)]
        {
            remove_windows_relative(self, relative)
        }

        #[cfg(not(any(unix, windows)))]
        {
            self.verify()?;
            fs::remove_file(self.path.join(relative)).map_err(|error| {
                SourceRootError::CannotInspect {
                    path: self.path.join(relative),
                    reason: error.to_string(),
                }
            })?;
            self.verify()
        }
    }

    pub(crate) fn open_file_relative(&self, relative: &Path) -> Result<fs::File, SourceRootError> {
        validate_relative(relative, false)?;

        #[cfg(unix)]
        {
            open_unix_relative(self, relative, false)
        }

        #[cfg(windows)]
        {
            open_windows_relative(self, relative, false)
        }

        #[cfg(not(any(unix, windows)))]
        {
            self.verify()?;
            let path = self.path.join(relative);
            let file = fs::File::open(&path).map_err(|error| SourceRootError::CannotInspect {
                path: path.clone(),
                reason: error.to_string(),
            })?;
            self.verify()?;
            Ok(file)
        }
    }

    /// Unix retained-handle directory admission with a hard entry allocation bound.
    #[cfg(unix)]
    pub fn read_dir_relative_bounded(
        &self,
        relative: &Path,
        maximum: usize,
    ) -> Result<Vec<SourceEntry>, SourceRootError> {
        validate_relative(relative, true)?;
        read_unix_dir_bounded(self, relative, maximum)
    }

    /// List through the retained directory handle, refusing symlink/reparse parents.
    /// Entries are observations; callers must reopen files and revalidate after reads.
    pub fn read_dir_relative(&self, relative: &Path) -> Result<Vec<SourceEntry>, SourceRootError> {
        validate_relative(relative, true)?;

        #[cfg(unix)]
        {
            read_unix_dir(self, relative)
        }

        #[cfg(windows)]
        {
            read_windows_dir(self, relative)
        }

        #[cfg(not(any(unix, windows)))]
        {
            self.verify()?;
            let path = self.path.join(relative);
            let entries = fs::read_dir(&path)
                .map_err(|error| SourceRootError::CannotInspect {
                    path: path.clone(),
                    reason: error.to_string(),
                })?
                .map(|entry| {
                    let entry = entry.map_err(|error| SourceRootError::CannotInspect {
                        path: path.clone(),
                        reason: error.to_string(),
                    })?;
                    let metadata =
                        entry
                            .metadata()
                            .map_err(|error| SourceRootError::CannotInspect {
                                path: entry.path(),
                                reason: error.to_string(),
                            })?;
                    Ok(SourceEntry {
                        name: entry.file_name(),
                        kind: if metadata.is_file() {
                            SourceEntryKind::File
                        } else if metadata.is_dir() {
                            SourceEntryKind::Directory
                        } else {
                            SourceEntryKind::Other
                        },
                        size: metadata.len(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.verify()?;
            Ok(entries)
        }
    }
}

/// A typed refusal to treat a path as the previously admitted source root.
pub enum SourceRootError {
    /// The path or its metadata could not be opened.
    CannotInspect {
        /// Root path being inspected.
        path: PathBuf,
        /// Operating-system failure.
        reason: String,
    },
    /// The operating system denied access to the retained source object.
    PermissionDenied {
        /// Source-relative object being inspected.
        path: PathBuf,
        /// Operating-system refusal.
        reason: String,
    },
    /// The root path no longer names a directory.
    NotDirectory {
        /// Root path being inspected.
        path: PathBuf,
    },
    /// The stored root path is now a symlink or Windows reparse point.
    SymlinkOrReparse {
        /// Root path being inspected.
        path: PathBuf,
    },
    /// The platform did not provide a stable object identity.
    IdentityUnavailable {
        /// Root path being inspected.
        path: PathBuf,
        /// Platform failure or missing identity component.
        reason: String,
    },
    /// The root path names a different object than the admitted one.
    IdentityChanged {
        /// Root path being inspected.
        path: PathBuf,
    },
    /// The root changed between path inspection and opened-handle inspection.
    ChangedDuringValidation {
        /// Root path being inspected.
        path: PathBuf,
    },
    /// A caller supplied an absolute or traversing path to a relative operation.
    InvalidRelative {
        /// Rejected path.
        path: PathBuf,
    },
    /// The platform has no supported stable filesystem-object identity.
    #[cfg(not(any(unix, windows)))]
    UnsupportedPlatform,
}

/// A bounded, path-free reason code for a source-root refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRootErrorCode {
    /// The root or one of its entries could not be inspected.
    CannotInspect,
    /// The operating system refused access.
    PermissionDenied,
    /// The retained address no longer names a directory.
    NotDirectory,
    /// A symlink or reparse point was encountered.
    SymlinkOrReparse,
    /// Stable filesystem identity was unavailable.
    IdentityUnavailable,
    /// The address names a different filesystem object.
    IdentityChanged,
    /// Identity changed during validation.
    ChangedDuringValidation,
    /// A relative operation received an invalid path.
    InvalidRelative,
    /// Stable identity is unsupported on this platform.
    #[cfg(not(any(unix, windows)))]
    UnsupportedPlatform,
}

impl SourceRootErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::CannotInspect => "cannot-inspect",
            Self::PermissionDenied => "permission-denied",
            Self::NotDirectory => "not-directory",
            Self::SymlinkOrReparse => "symlink-or-reparse",
            Self::IdentityUnavailable => "identity-unavailable",
            Self::IdentityChanged => "identity-changed",
            Self::ChangedDuringValidation => "changed-during-validation",
            Self::InvalidRelative => "invalid-relative",
            #[cfg(not(any(unix, windows)))]
            Self::UnsupportedPlatform => "unsupported-platform",
        }
    }
}

impl SourceRootError {
    /// Return the bounded reason code without exposing native paths or
    /// operating-system context.
    pub fn code(&self) -> SourceRootErrorCode {
        match self {
            Self::CannotInspect { .. } => SourceRootErrorCode::CannotInspect,
            Self::PermissionDenied { .. } => SourceRootErrorCode::PermissionDenied,
            Self::NotDirectory { .. } => SourceRootErrorCode::NotDirectory,
            Self::SymlinkOrReparse { .. } => SourceRootErrorCode::SymlinkOrReparse,
            Self::IdentityUnavailable { .. } => SourceRootErrorCode::IdentityUnavailable,
            Self::IdentityChanged { .. } => SourceRootErrorCode::IdentityChanged,
            Self::ChangedDuringValidation { .. } => SourceRootErrorCode::ChangedDuringValidation,
            Self::InvalidRelative { .. } => SourceRootErrorCode::InvalidRelative,
            #[cfg(not(any(unix, windows)))]
            Self::UnsupportedPlatform => SourceRootErrorCode::UnsupportedPlatform,
        }
    }
}

impl std::fmt::Debug for SourceRootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SourceRootError")
            .field("code", &self.code().as_str())
            .finish()
    }
}

impl std::fmt::Display for SourceRootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code().as_str())
    }
}

impl std::error::Error for SourceRootError {}

impl SourceRootIdentity {
    /// Capture identity from an opened directory while rejecting a path-level
    /// symlink/reparse substitution before and after the open.
    pub fn capture(path: &Path) -> Result<Self, SourceRootError> {
        capture_authority(path).map(|(identity, _handle)| identity)
    }

    /// Confirm that `path` still names this exact filesystem object.
    pub fn verify(&self, path: &Path) -> Result<(), SourceRootError> {
        let observed = Self::capture(path)?;
        self.verify_observed(path, &observed)
    }

    fn verify_observed(
        &self,
        path: &Path,
        observed: &SourceRootIdentity,
    ) -> Result<(), SourceRootError> {
        if self == observed {
            Ok(())
        } else {
            Err(SourceRootError::IdentityChanged {
                path: path.to_path_buf(),
            })
        }
    }
}

/// Return the private per-source index path below `base`.
///
/// Stable object identity determines the key, so a replacement directory at
/// the same canonical path cannot inherit an older source object's index and a
/// rename does not orphan the existing index.
pub fn workspace_index_path(base: &Path, root: &SourceRoot) -> PathBuf {
    workspace_index_path_for_stable_identity(base, &root.identity)
}

/// Return the private per-source index path for a persisted stable identity.
pub fn workspace_index_path_for_stable_identity(
    base: &Path,
    identity: &SourceRootIdentity,
) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-source-root-v3\0");
    hash_source_identity(&mut hasher, identity);
    let hash = hasher.finalize().to_hex().to_string();
    base.join("workspaces").join(hash).join("catdb.db")
}

/// Return the historical V2 per-source index path whose key combined native
/// path units with stable object identity. This is migration-only: new managed
/// state must use [`workspace_index_path_for_stable_identity`].
pub fn legacy_workspace_index_path_for_path_identity(
    base: &Path,
    path: &Path,
    identity: &SourceRootIdentity,
) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-source-root-v2\0");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hasher.update(b"unix-path\0");
        hasher.update(path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        hasher.update(b"windows-path\0");
        for unit in path.as_os_str().encode_wide() {
            hasher.update(&unit.to_le_bytes());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        hasher.update(b"platform-path\0");
        hasher.update(path.to_string_lossy().as_bytes());
    }
    hash_source_identity(&mut hasher, identity);
    let hash = hasher.finalize().to_hex().to_string();
    base.join("workspaces").join(hash).join("catdb.db")
}

fn hash_source_identity(hasher: &mut blake3::Hasher, identity: &SourceRootIdentity) {
    match identity {
        SourceRootIdentity::Unix { device, inode } => {
            hasher.update(b"unix-identity\0");
            hasher.update(&device.to_le_bytes());
            hasher.update(&inode.to_le_bytes());
        }
        SourceRootIdentity::Windows {
            volume_serial_number,
            file_id,
        } => {
            hasher.update(b"windows-identity\0");
            hasher.update(&volume_serial_number.to_le_bytes());
            hasher.update(file_id);
        }
    }
}

fn capture_authority(path: &Path) -> Result<(SourceRootIdentity, fs::File), SourceRootError> {
    #[cfg(unix)]
    {
        capture_unix(path)
    }

    #[cfg(windows)]
    {
        capture_windows(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(SourceRootError::UnsupportedPlatform)
    }
}

fn source_root_from_opened_directory(
    path: PathBuf,
    opened: fs::File,
) -> Result<SourceRoot, SourceRootError> {
    let metadata = opened
        .metadata()
        .map_err(|error| SourceRootError::IdentityUnavailable {
            path: path.clone(),
            reason: error.to_string(),
        })?;
    if !metadata.is_dir() {
        return Err(SourceRootError::NotDirectory { path });
    }

    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        SourceRootIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    };

    #[cfg(windows)]
    let identity = {
        use std::os::windows::fs::MetadataExt;
        if is_windows_reparse_point(metadata.file_attributes()) {
            return Err(SourceRootError::SymlinkOrReparse { path });
        }
        windows_identity(&path, &opened)?
    };

    #[cfg(not(any(unix, windows)))]
    {
        let _ = opened;
        return Err(SourceRootError::UnsupportedPlatform);
    }

    Ok(SourceRoot {
        path,
        identity,
        handle: Arc::new(opened),
    })
}

fn validate_relative(path: &Path, allow_empty: bool) -> Result<(), SourceRootError> {
    if (!allow_empty && path.as_os_str().is_empty())
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SourceRootError::InvalidRelative {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn capture_unix(path: &Path) -> Result<(SourceRootIdentity, fs::File), SourceRootError> {
    use std::os::unix::fs::MetadataExt;

    let before = fs::symlink_metadata(path).map_err(|error| SourceRootError::CannotInspect {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if before.file_type().is_symlink() {
        return Err(SourceRootError::SymlinkOrReparse {
            path: path.to_path_buf(),
        });
    }
    if !before.is_dir() {
        return Err(SourceRootError::NotDirectory {
            path: path.to_path_buf(),
        });
    }

    let opened = fs::File::open(path).map_err(|error| SourceRootError::CannotInspect {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let opened_metadata =
        opened
            .metadata()
            .map_err(|error| SourceRootError::IdentityUnavailable {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
    if !opened_metadata.is_dir() {
        return Err(SourceRootError::NotDirectory {
            path: path.to_path_buf(),
        });
    }
    let opened_identity = SourceRootIdentity::Unix {
        device: opened_metadata.dev(),
        inode: opened_metadata.ino(),
    };

    let after = fs::symlink_metadata(path).map_err(|error| SourceRootError::CannotInspect {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if after.file_type().is_symlink() {
        return Err(SourceRootError::SymlinkOrReparse {
            path: path.to_path_buf(),
        });
    }
    let path_identity = SourceRootIdentity::Unix {
        device: after.dev(),
        inode: after.ino(),
    };
    if opened_identity != path_identity {
        return Err(SourceRootError::ChangedDuringValidation {
            path: path.to_path_buf(),
        });
    }
    Ok((opened_identity, opened))
}

#[cfg(windows)]
fn capture_windows(path: &Path) -> Result<(SourceRootIdentity, fs::File), SourceRootError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, SYNCHRONIZE,
    };

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const ROOT_ACCESS: u32 = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

    let before = fs::symlink_metadata(path).map_err(|error| SourceRootError::CannotInspect {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if is_windows_reparse_point(before.file_attributes()) {
        return Err(SourceRootError::SymlinkOrReparse {
            path: path.to_path_buf(),
        });
    }
    if !before.is_dir() {
        return Err(SourceRootError::NotDirectory {
            path: path.to_path_buf(),
        });
    }

    let opened = fs::OpenOptions::new()
        .access_mode(ROOT_ACCESS)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| SourceRootError::CannotInspect {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let opened_metadata =
        opened
            .metadata()
            .map_err(|error| SourceRootError::IdentityUnavailable {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
    if is_windows_reparse_point(opened_metadata.file_attributes()) {
        return Err(SourceRootError::SymlinkOrReparse {
            path: path.to_path_buf(),
        });
    }
    if !opened_metadata.is_dir() {
        return Err(SourceRootError::NotDirectory {
            path: path.to_path_buf(),
        });
    }
    let opened_identity = windows_identity(path, &opened)?;

    let after = fs::symlink_metadata(path).map_err(|error| SourceRootError::CannotInspect {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    if is_windows_reparse_point(after.file_attributes()) {
        return Err(SourceRootError::SymlinkOrReparse {
            path: path.to_path_buf(),
        });
    }
    let reopened = fs::OpenOptions::new()
        .access_mode(ROOT_ACCESS)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| SourceRootError::CannotInspect {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let path_identity = windows_identity(path, &reopened)?;
    if opened_identity != path_identity {
        return Err(SourceRootError::ChangedDuringValidation {
            path: path.to_path_buf(),
        });
    }
    Ok((opened_identity, opened))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_identity(path: &Path, opened: &fs::File) -> Result<SourceRootIdentity, SourceRootError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };

    let mut information = FILE_ID_INFO::default();
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            opened.as_raw_handle().cast(),
            FileIdInfo,
            std::ptr::addr_of_mut!(information).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        return Err(SourceRootError::IdentityUnavailable {
            path: path.to_path_buf(),
            reason: std::io::Error::last_os_error().to_string(),
        });
    }
    Ok(SourceRootIdentity::Windows {
        volume_serial_number: information.VolumeSerialNumber,
        file_id: information.FileId.Identifier,
    })
}

#[cfg(unix)]
fn open_unix_relative(
    root: &SourceRoot,
    relative: &Path,
    directory: bool,
) -> Result<fs::File, SourceRootError> {
    use rustix::fs::{openat, Mode, OFlags};

    let mut current = root
        .handle
        .try_clone()
        .map_err(|error| SourceRootError::CannotInspect {
            path: root.path.clone(),
            reason: error.to_string(),
        })?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        if directory {
            return Ok(current);
        }
        return Err(SourceRootError::InvalidRelative {
            path: relative.to_path_buf(),
        });
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(SourceRootError::InvalidRelative {
                path: relative.to_path_buf(),
            });
        };
        let last = index + 1 == components.len();
        let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        if !last || directory {
            flags |= OFlags::DIRECTORY;
        }
        let fd = openat(&current, *name, flags, Mode::empty()).map_err(|error| {
            let path = root.path.join(relative);
            if error == rustix::io::Errno::LOOP || error == rustix::io::Errno::NOTDIR {
                SourceRootError::SymlinkOrReparse { path }
            } else {
                unix_source_error(path, error)
            }
        })?;
        current = fs::File::from(fd);
    }
    let metadata = current
        .metadata()
        .map_err(|error| SourceRootError::CannotInspect {
            path: root.path.join(relative),
            reason: error.to_string(),
        })?;
    if directory && !metadata.is_dir() {
        return Err(SourceRootError::NotDirectory {
            path: root.path.join(relative),
        });
    }
    if !directory && !metadata.is_file() {
        return Err(SourceRootError::CannotInspect {
            path: root.path.join(relative),
            reason: "not a regular file".to_string(),
        });
    }
    Ok(current)
}

#[cfg(unix)]
fn unix_source_error(path: PathBuf, error: rustix::io::Errno) -> SourceRootError {
    if error == rustix::io::Errno::ACCESS || error == rustix::io::Errno::PERM {
        SourceRootError::PermissionDenied {
            path,
            reason: error.to_string(),
        }
    } else {
        SourceRootError::CannotInspect {
            path,
            reason: error.to_string(),
        }
    }
}

#[cfg(unix)]
fn open_unix_parent(
    root: &SourceRoot,
    relative: &Path,
) -> Result<(fs::File, OsString), SourceRootError> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let name = relative
        .file_name()
        .ok_or_else(|| SourceRootError::InvalidRelative {
            path: relative.to_path_buf(),
        })?
        .to_os_string();
    let directory = open_unix_relative(root, parent, true)?;
    Ok((directory, name))
}

#[cfg(unix)]
fn read_unix_dir(root: &SourceRoot, relative: &Path) -> Result<Vec<SourceEntry>, SourceRootError> {
    read_unix_dir_bounded(root, relative, usize::MAX)
}
#[cfg(unix)]
fn read_unix_dir_bounded(
    root: &SourceRoot,
    relative: &Path,
    maximum: usize,
) -> Result<Vec<SourceEntry>, SourceRootError> {
    use std::os::unix::ffi::OsStrExt;

    use rustix::fs::{statat, AtFlags, Dir, FileType};

    let directory = open_unix_relative(root, relative, true)?;
    let mut stream = Dir::read_from(&directory)
        .map_err(|error| unix_source_error(root.path.join(relative), error))?;
    let mut entries = Vec::new();
    for entry in &mut stream {
        let entry = entry.map_err(|error| unix_source_error(root.path.join(relative), error))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        if entries.len() >= maximum {
            return Err(SourceRootError::CannotInspect {
                path: root.path.join(relative),
                reason: "directory entry bound exceeded".into(),
            });
        }
        let name = std::ffi::OsStr::from_bytes(name);
        let stat = statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| unix_source_error(root.path.join(relative).join(name), error))?;
        let file_type = FileType::from_raw_mode(stat.st_mode);
        entries.push(SourceEntry {
            name: name.to_os_string(),
            kind: if file_type.is_file() {
                SourceEntryKind::File
            } else if file_type.is_dir() {
                SourceEntryKind::Directory
            } else if file_type.is_symlink() {
                SourceEntryKind::SymlinkOrReparse
            } else {
                SourceEntryKind::Other
            },
            size: u64::try_from(stat.st_size).unwrap_or(u64::MAX),
        });
    }
    Ok(entries)
}

#[cfg(windows)]
fn open_windows_relative(
    root: &SourceRoot,
    relative: &Path,
    directory: bool,
) -> Result<fs::File, SourceRootError> {
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_READ, SYNCHRONIZE};

    let mut current = root
        .handle
        .try_clone()
        .map_err(|error| SourceRootError::CannotInspect {
            path: root.path.clone(),
            reason: error.to_string(),
        })?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        if directory {
            return Ok(current);
        }
        return Err(SourceRootError::InvalidRelative {
            path: relative.to_path_buf(),
        });
    }

    let mut traversed = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(SourceRootError::InvalidRelative {
                path: relative.to_path_buf(),
            });
        };
        traversed.push(name);
        let last = index + 1 == components.len();
        let expect_directory = !last || directory;
        let options = FILE_OPEN_REPARSE_POINT
            | FILE_SYNCHRONOUS_IO_NONALERT
            | if expect_directory {
                FILE_DIRECTORY_FILE
            } else {
                FILE_NON_DIRECTORY_FILE
            };
        current = open_windows_child(
            root,
            &current,
            name,
            FILE_GENERIC_READ | SYNCHRONIZE,
            FILE_OPEN,
            options,
            &traversed,
        )?;
    }
    Ok(current)
}

#[cfg(windows)]
fn open_windows_parent(
    root: &SourceRoot,
    relative: &Path,
) -> Result<(fs::File, OsString), SourceRootError> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let name = relative
        .file_name()
        .ok_or_else(|| SourceRootError::InvalidRelative {
            path: relative.to_path_buf(),
        })?
        .to_os_string();
    Ok((open_windows_relative(root, parent, true)?, name))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn open_windows_child(
    root: &SourceRoot,
    parent: &fs::File,
    name: &std::ffi::OsStr,
    desired_access: u32,
    disposition: u32,
    create_options: u32,
    display_relative: &Path,
) -> Result<fs::File, SourceRootError> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};

    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{NtCreateFile, RtlNtStatusToDosErrorNoTeb};
    use windows_sys::Win32::Foundation::{HANDLE, OBJ_CASE_INSENSITIVE, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut wide: Vec<u16> = name.encode_wide().collect();
    let byte_len = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| SourceRootError::InvalidRelative {
            path: display_relative.to_path_buf(),
        })?;
    if wide.is_empty() {
        return Err(SourceRootError::InvalidRelative {
            path: display_relative.to_path_buf(),
        });
    }
    let unicode = UNICODE_STRING {
        Length: byte_len,
        MaximumLength: byte_len,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle().cast(),
        ObjectName: std::ptr::addr_of!(unicode),
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut status_block = IO_STATUS_BLOCK::default();
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: every pointer refers to a live, correctly sized Windows ABI
    // structure for the duration of the call. `handle` is adopted exactly once
    // on success and remains owned by the returned `File`.
    let status = unsafe {
        NtCreateFile(
            std::ptr::addr_of_mut!(handle),
            desired_access,
            std::ptr::addr_of!(attributes),
            std::ptr::addr_of_mut!(status_block),
            std::ptr::null(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            disposition,
            create_options,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        // SAFETY: this conversion is total for every NTSTATUS and has no pointer
        // preconditions.
        let os_error = unsafe { RtlNtStatusToDosErrorNoTeb(status) };
        return Err(SourceRootError::CannotInspect {
            path: root.path.join(display_relative),
            reason: std::io::Error::from_raw_os_error(os_error as i32).to_string(),
        });
    }
    if handle.is_null() {
        return Err(SourceRootError::IdentityUnavailable {
            path: root.path.join(display_relative),
            reason: "NtCreateFile returned a null handle".to_string(),
        });
    }
    // SAFETY: successful NtCreateFile transferred one owned HANDLE to us.
    let file = unsafe { fs::File::from_raw_handle(handle.cast()) };
    use std::os::windows::fs::MetadataExt;
    let metadata = file
        .metadata()
        .map_err(|error| SourceRootError::CannotInspect {
            path: root.path.join(display_relative),
            reason: error.to_string(),
        })?;
    if is_windows_reparse_point(metadata.file_attributes()) {
        return Err(SourceRootError::SymlinkOrReparse {
            path: root.path.join(display_relative),
        });
    }
    Ok(file)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn rename_windows_relative(
    root: &SourceRoot,
    from: &Path,
    to: &Path,
) -> Result<(), SourceRootError> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, RtlNtStatusToDosErrorNoTeb,
        FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
        FILE_RENAME_INFORMATION_0, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_READ_ATTRIBUTES, SYNCHRONIZE};
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let (from_parent, from_name) = open_windows_parent(root, from)?;
    let source = open_windows_child(
        root,
        &from_parent,
        &from_name,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        from,
    )?;
    let (to_parent, to_name) = open_windows_parent(root, to)?;
    let wide: Vec<u16> = to_name.encode_wide().collect();
    let name_bytes = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| SourceRootError::InvalidRelative {
            path: to.to_path_buf(),
        })?;
    let header = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let byte_len = header.checked_add(name_bytes as usize).ok_or_else(|| {
        SourceRootError::InvalidRelative {
            path: to.to_path_buf(),
        }
    })?;
    let words = byte_len.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0usize; words];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: `storage` is aligned for FILE_RENAME_INFORMATION and has room for its
    // fixed header plus every UTF-16 byte copied below.
    unsafe {
        (*info).Anonymous = FILE_RENAME_INFORMATION_0 {
            ReplaceIfExists: true,
        };
        (*info).RootDirectory = to_parent.as_raw_handle().cast();
        (*info).FileNameLength = name_bytes;
        std::ptr::copy_nonoverlapping(
            wide.as_ptr().cast::<u8>(),
            storage.as_mut_ptr().cast::<u8>().add(header),
            name_bytes as usize,
        );
    }
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: synchronous `source` is DELETE-capable, the destination parent
    // remains pinned, and `info` is the native ABI buffer of `byte_len` bytes.
    let status = unsafe {
        NtSetInformationFile(
            source.as_raw_handle().cast(),
            &mut status_block,
            info.cast(),
            byte_len as u32,
            FileRenameInformation,
        )
    };
    if status != 0 {
        // SAFETY: pure conversion of the returned native status.
        let code = unsafe { RtlNtStatusToDosErrorNoTeb(status) };
        return Err(SourceRootError::CannotInspect {
            path: root.path.join(to),
            reason: std::io::Error::from_raw_os_error(code as i32).to_string(),
        });
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn remove_windows_relative(root: &SourceRoot, relative: &Path) -> Result<(), SourceRootError> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
        FILE_READ_ATTRIBUTES, SYNCHRONIZE,
    };

    let (parent, name) = open_windows_parent(root, relative)?;
    let file = open_windows_child(
        root,
        &parent,
        &name,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        relative,
    )?;
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `file` is a live DELETE-capable handle and `disposition` is the
    // exact structure required by FileDispositionInfo.
    let succeeded = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle().cast(),
            FileDispositionInfo,
            std::ptr::addr_of!(disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        return Err(SourceRootError::CannotInspect {
            path: root.path.join(relative),
            reason: std::io::Error::last_os_error().to_string(),
        });
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn read_windows_dir(
    root: &SourceRoot,
    relative: &Path,
) -> Result<Vec<SourceEntry>, SourceRootError> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ID_BOTH_DIR_INFO,
    };

    let directory = open_windows_relative(root, relative, true)?;
    const BUFFER_BYTES: usize = 64 * 1024;
    let words = BUFFER_BYTES.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0usize; words];
    let mut restart = true;
    let mut entries = Vec::new();

    loop {
        let class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        restart = false;
        // SAFETY: the directory handle is live and `storage` is a writable
        // BUFFER_BYTES-sized region for the requested directory information.
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle().cast(),
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
            return Err(SourceRootError::CannotInspect {
                path: root.path.join(relative),
                reason: error.to_string(),
            });
        }

        let mut offset = 0usize;
        loop {
            let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
            if offset
                .checked_add(header)
                .is_none_or(|end| end > BUFFER_BYTES)
            {
                return Err(SourceRootError::CannotInspect {
                    path: root.path.join(relative),
                    reason: "Windows returned malformed directory information".to_string(),
                });
            }
            // SAFETY: the bounds check above establishes a complete fixed
            // FILE_ID_BOTH_DIR_INFO header at this aligned record offset.
            let info = unsafe {
                &*storage
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
            };
            let name_bytes = info.FileNameLength as usize;
            if name_bytes % std::mem::size_of::<u16>() != 0
                || offset
                    .checked_add(header)
                    .and_then(|start| start.checked_add(name_bytes))
                    .is_none_or(|end| end > BUFFER_BYTES)
            {
                return Err(SourceRootError::CannotInspect {
                    path: root.path.join(relative),
                    reason: "Windows returned malformed directory filename data".to_string(),
                });
            }
            // SAFETY: FileNameLength was validated against the backing buffer
            // and is an even number of bytes.
            let name = unsafe {
                let wide = std::slice::from_raw_parts(
                    std::ptr::addr_of!(info.FileName).cast::<u16>(),
                    name_bytes / std::mem::size_of::<u16>(),
                );
                OsString::from_wide(wide)
            };
            if name != "." && name != ".." {
                let attributes = info.FileAttributes;
                entries.push(SourceEntry {
                    name,
                    kind: if is_windows_reparse_point(attributes) {
                        SourceEntryKind::SymlinkOrReparse
                    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                        SourceEntryKind::Directory
                    } else {
                        SourceEntryKind::File
                    },
                    size: u64::try_from(info.EndOfFile).unwrap_or(u64::MAX),
                });
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            let next = info.NextEntryOffset as usize;
            if next < header
                || offset
                    .checked_add(next)
                    .is_none_or(|end| end >= BUFFER_BYTES)
            {
                return Err(SourceRootError::CannotInspect {
                    path: root.path.join(relative),
                    reason: "Windows returned an invalid directory record offset".to_string(),
                });
            }
            offset += next;
        }
    }
    Ok(entries)
}

#[cfg(any(windows, test))]
fn is_windows_reparse_point(attributes: u32) -> bool {
    attributes & WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(test)]
mod tests {
    use super::{
        is_windows_reparse_point, SourceRoot, SourceRootError, SourceRootIdentity,
        WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT,
    };
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn windows_identity_comparison_uses_the_full_refs_volume_and_file_id() {
        let expected = SourceRootIdentity::Windows {
            volume_serial_number: 0x0000_0001_0000_0029,
            file_id: [0x11; 16],
        };
        let other_volume = SourceRootIdentity::Windows {
            volume_serial_number: 0x0000_0002_0000_0029,
            file_id: [0x11; 16],
        };
        let other_file = SourceRootIdentity::Windows {
            volume_serial_number: 0x0000_0001_0000_0029,
            file_id: [0x11; 16],
        };
        let mut other_file = other_file;
        if let SourceRootIdentity::Windows { file_id, .. } = &mut other_file {
            file_id[15] = 0x12;
        }

        assert!(expected
            .verify_observed(Path::new("C:\\notes"), &expected)
            .is_ok());
        assert!(matches!(
            expected.verify_observed(Path::new("C:\\notes"), &other_volume),
            Err(SourceRootError::IdentityChanged { .. })
        ));
        assert!(matches!(
            expected.verify_observed(Path::new("C:\\notes"), &other_file),
            Err(SourceRootError::IdentityChanged { .. })
        ));

        let base = Path::new("C:\\app-data");
        assert_ne!(
            super::workspace_index_path_for_stable_identity(base, &expected),
            super::workspace_index_path_for_stable_identity(base, &other_volume),
            "the index key includes all 64 volume-serial bits"
        );
        assert_ne!(
            super::workspace_index_path_for_stable_identity(base, &expected),
            super::workspace_index_path_for_stable_identity(base, &other_file),
            "the index key includes all 128 file-ID bits"
        );
    }

    #[test]
    fn managed_index_address_follows_stable_object_identity_across_rename() {
        let base = Path::new("/app-data");
        let directory = tempdir().expect("tempdir");
        let before = SourceRoot::capture(directory.path().to_path_buf()).expect("capture root");
        let mut after = before.clone();
        after.path = directory.path().join("renamed-address");

        assert_eq!(
            super::workspace_index_path(base, &before),
            super::workspace_index_path(base, &after),
            "renaming one retained filesystem object must not fabricate a new managed index"
        );
    }

    #[test]
    fn generic_debug_and_error_surfaces_never_expose_native_authority_or_secret_context() {
        let temp = tempdir().expect("tempdir");
        let sentinel = temp.path().join("private-token-document-body");
        fs::create_dir(&sentinel).expect("create sentinel source");
        let root =
            SourceRoot::capture(fs::canonicalize(&sentinel).expect("canonical sentinel source"))
                .expect("capture source");
        let root_debug = format!("{root:?}");
        assert!(!root_debug.contains("private-token-document-body"));
        assert!(!root_debug.contains(&temp.path().to_string_lossy().into_owned()));

        let error = SourceRootError::CannotInspect {
            path: sentinel,
            reason:
                "postgres://user:password@host/db bearer-token bound-value secret document text"
                    .to_string(),
        };
        for surface in [format!("{error:?}"), error.to_string()] {
            assert!(!surface.contains("private-token-document-body"));
            assert!(!surface.contains("postgres://"));
            assert!(!surface.contains("bearer-token"));
            assert!(!surface.contains("bound-value"));
            assert!(!surface.contains("secret document text"));
            assert!(surface.contains("cannot-inspect"));
        }
    }

    #[test]
    fn windows_reparse_attribute_is_rejected_by_the_platform_abstraction() {
        assert!(is_windows_reparse_point(
            WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT
        ));
        assert!(is_windows_reparse_point(
            WINDOWS_FILE_ATTRIBUTE_REPARSE_POINT | 0x10
        ));
        assert!(!is_windows_reparse_point(0x10));
    }

    #[cfg(unix)]
    #[test]
    fn unix_identity_rejects_a_different_directory_at_the_same_path() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let admitted = SourceRoot::capture(canonical.clone()).expect("capture admitted root");

        fs::rename(&canonical, temp.path().join("moved")).expect("move admitted object");
        fs::create_dir(&canonical).expect("create replacement");

        assert!(matches!(
            admitted.verify(),
            Err(SourceRootError::IdentityChanged { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unix_identity_rejects_a_symlink_at_the_stored_root_path() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("notes");
        let replacement = temp.path().join("replacement");
        fs::create_dir(&root).expect("create root");
        fs::create_dir(&replacement).expect("create replacement");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let admitted = SourceRoot::capture(canonical.clone()).expect("capture admitted root");

        fs::remove_dir(&canonical).expect("remove admitted path");
        symlink(&replacement, &canonical).expect("install symlink");

        assert!(matches!(
            admitted.verify(),
            Err(SourceRootError::SymlinkOrReparse { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn retained_authority_reads_the_admitted_root_not_a_same_path_replacement() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("note.md"), "# admitted").expect("write admitted document");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let admitted = SourceRoot::capture(canonical.clone()).expect("capture admitted root");

        let moved = temp.path().join("moved-admitted");
        fs::rename(&canonical, &moved).expect("move admitted object");
        fs::create_dir(&canonical).expect("create same-path replacement");
        fs::write(canonical.join("note.md"), "# replacement").expect("write replacement");

        let opened = admitted
            .read_relative(Path::new("note.md"))
            .expect("retained authority still opens the admitted object");
        assert_eq!(opened.bytes, b"# admitted");
        assert_eq!(
            fs::read_to_string(canonical.join("note.md")).expect("replacement remains readable"),
            "# replacement",
            "the replacement object was never used"
        );
    }

    #[cfg(windows)]
    #[test]
    fn retained_root_blocks_path_replacement_without_blocking_child_edits() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("note.md"), "# admitted").expect("write admitted document");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let admitted = SourceRoot::capture(canonical.clone()).expect("capture admitted root");

        let rename = fs::rename(&canonical, temp.path().join("replacement-window"));
        assert!(
            rename.is_err(),
            "the retained Windows root must deny delete sharing during connector handoff"
        );
        fs::write(canonical.join("note.md"), "# edited")
            .expect("retaining the directory does not block child edits");
        assert_eq!(
            admitted
                .read_relative(Path::new("note.md"))
                .expect("read edited child")
                .bytes,
            b"# edited"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retained_authority_rejects_a_descendant_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("notes");
        let nested = root.join("nested");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&nested).expect("create nested directory");
        fs::create_dir(&outside).expect("create outside directory");
        fs::write(nested.join("note.md"), "# admitted").expect("write admitted document");
        fs::write(outside.join("note.md"), "# outside").expect("write outside document");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let admitted = SourceRoot::capture(canonical.clone()).expect("capture admitted root");

        fs::rename(&nested, root.join("moved-nested")).expect("move nested object");
        symlink(&outside, &nested).expect("install descendant escape");

        let error = admitted
            .read_relative(Path::new("nested/note.md"))
            .expect_err("descriptor-relative traversal must not follow the descendant symlink");
        assert!(
            matches!(error, SourceRootError::SymlinkOrReparse { .. }),
            "escape is a typed refusal: {error}"
        );
        assert_eq!(
            fs::read_to_string(outside.join("note.md")).expect("outside remains readable"),
            "# outside"
        );
    }

    #[test]
    fn an_absent_child_directory_is_created_and_returned_as_retained_authority() {
        let temp = tempdir().expect("tempdir");
        let parent_path = fs::canonicalize(temp.path()).expect("canonical parent");
        let parent = SourceRoot::capture(parent_path).expect("capture parent");

        let child = parent
            .create_or_capture_directory_relative(Path::new("diagnostics"))
            .expect("create diagnostics authority");

        assert!(child.path.is_dir());
        child.verify().expect("created authority remains current");
        assert_eq!(
            parent
                .create_or_capture_directory_relative(Path::new("diagnostics"))
                .expect("capture existing diagnostics authority")
                .identity,
            child.identity
        );
    }

    #[cfg(unix)]
    #[test]
    fn child_directory_creation_refuses_a_preexisting_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).expect("create outside");
        symlink(&outside, temp.path().join("diagnostics")).expect("install child symlink");
        let parent_path = fs::canonicalize(temp.path()).expect("canonical parent");
        let parent = SourceRoot::capture(parent_path).expect("capture parent");

        assert!(matches!(
            parent.create_or_capture_directory_relative(Path::new("diagnostics")),
            Err(SourceRootError::SymlinkOrReparse { .. })
        ));
    }
}
