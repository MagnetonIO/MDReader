//! Path-based local library roots with a retained directory handle.
//!
//! A root is matched by its canonical path. Persisted device/inode values are
//! not stable (macOS renumbers a volume's `st_dev` across reboots), so restoring
//! a persisted root adopts whatever directory the path names now. The persisted
//! identity is kept on the restored root as its binding key (source ids, grant
//! ids and journals are derived from it), while owners that store it compare it
//! with [`SourceRoot::observed_identity`] and persist the observed value. A live
//! root still refuses to act through its retained handle once the path names a
//! different object, so it never silently reads a renamed-away directory;
//! restoring the path again recovers without a re-pick.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub mod file_stamp;
mod private_match;
#[cfg(any(windows, test))]
pub(crate) use private_match::short_name_component;
pub(crate) use private_match::{exclusion_relative, private_path_ambiguous};
#[cfg(unix)]
mod read_symlink;
mod remove_directory;
#[cfg(windows)]
mod windows_listing;
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
    /// Binding key: the identity captured at admission, or the persisted one
    /// for a restored root. It may differ from the object's current device or
    /// inode; see [`SourceRoot::observed_identity`].
    pub identity: SourceRootIdentity,
    handle: Arc<fs::File>,
    exclusions: Arc<Vec<PathBuf>>,
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
    /// Roots are the same root when they name the same canonical path.
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
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
            exclusions: Arc::new(Vec::new()),
        })
    }

    /// Reopen a persisted path, adopting the directory it names now. `stored`
    /// is retained as the binding key and never compared with the object.
    pub fn restore(path: PathBuf, stored: SourceRootIdentity) -> Result<Self, SourceRootError> {
        let observed = Self::capture(path)?;
        Ok(Self {
            identity: stored,
            ..observed
        })
    }

    /// The device/inode (or volume/file id) of the retained directory now,
    /// for owners that persist identity alongside the path.
    pub fn observed_identity(&self) -> Result<SourceRootIdentity, SourceRootError> {
        handle_identity(&self.path, &self.handle)
    }

    /// Attach mandatory native-owned exclusions. This can only narrow a grant.
    /// Paths remain internal and never become renderer-issued file authority.
    pub fn excluding(mut self, exclusions: Vec<PathBuf>) -> Result<Self, SourceRootError> {
        if exclusions.len() > 256 || exclusions.iter().any(|p| !p.is_absolute()) {
            return Err(SourceRootError::InvalidRelative {
                path: self.path.clone(),
            });
        }
        let mut all = (*self.exclusions).clone();
        all.extend(exclusions);
        all.sort();
        all.dedup();
        self.exclusions = Arc::new(all);
        self.authorize_relative(Path::new(""))?;
        Ok(self)
    }
    pub fn allows_relative(&self, relative: &Path) -> bool {
        self.authorize_relative(relative).is_ok()
    }
    pub(crate) fn authorize_relative(&self, relative: &Path) -> Result<(), SourceRootError> {
        validate_relative(relative, true)?;
        let path = self.path.join(relative);
        if !self.exclusions.is_empty() && private_path_ambiguous(&path) {
            return Err(SourceRootError::PermissionDenied {
                path,
                reason: "ambiguous private storage alias".into(),
            });
        }
        if self
            .exclusions
            .iter()
            .any(|private| exclusion_relative(&path, private).is_some())
        {
            return Err(SourceRootError::PermissionDenied {
                path,
                reason: "private native storage".into(),
            });
        }
        Ok(())
    }
    pub(crate) fn excludes_namespace(&self, private: &Path) -> bool {
        self.exclusions
            .iter()
            .any(|denied| exclusion_relative(private, denied).is_some())
    }
    /// Revalidate that the path is still a plain directory and still names the
    /// object behind the retained handle. Both identities are observed now, so
    /// a renumbered device never fails this; only an in-session replacement of
    /// the directory does, and restoring the path again adopts it.
    pub fn verify(&self) -> Result<(), SourceRootError> {
        let observed = SourceRootIdentity::capture(&self.path)?;
        if observed == handle_identity(&self.path, &self.handle)? {
            Ok(())
        } else {
            Err(SourceRootError::IdentityChanged {
                path: self.path.clone(),
            })
        }
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
        self.authorize_relative(relative)?;
        validate_relative(relative, false)?;
        self.authorize_relative(relative)?;
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
        self.authorize_relative(relative)?;
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

    /// Flush the exact retained destination directory after publication.
    pub fn sync_directory_relative(&self, relative: &Path) -> Result<(), SourceRootError> {
        self.authorize_relative(relative)?;
        validate_relative(relative, true)?;
        #[cfg(unix)]
        let directory = open_unix_relative(self, relative, true)?;
        #[cfg(windows)]
        let directory = open_windows_relative(self, relative, true)?;
        #[cfg(any(unix, windows))]
        {
            directory
                .sync_all()
                .map_err(|error| SourceRootError::CannotInspect {
                    path: self.path.join(relative),
                    reason: error.to_string(),
                })?;
            self.verify()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(SourceRootError::PermissionDenied {
                path: self.path.join(relative),
                reason: "directory synchronization unsupported".into(),
            })
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
        self.authorize_relative(relative)?;
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
        let child = source_root_from_opened_directory(self.path.join(relative), opened)?
            .excluding((*self.exclusions).clone())?;
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
        self.authorize_relative(relative)?;
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
        self.authorize_relative(relative)?;
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
        self.authorize_relative(relative)?;
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
        self.authorize_relative(relative)?;
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
        self.authorize_relative(from)?;
        validate_relative(from, false)?;
        self.authorize_relative(to)?;
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
            rename_windows_relative(self, from, to, true)
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

    /// Publish a complete staged regular file without replacing any destination.
    pub fn publish_new_relative(&self, from: &Path, to: &Path) -> Result<(), SourceRootError> {
        self.authorize_relative(from)?;
        validate_relative(from, false)?;
        self.authorize_relative(to)?;
        validate_relative(to, false)?;
        #[cfg(unix)]
        {
            let (from_parent, from_name) = open_unix_parent(self, from)?;
            let (to_parent, to_name) = open_unix_parent(self, to)?;
            rustix::fs::linkat(
                &from_parent,
                &from_name,
                &to_parent,
                &to_name,
                rustix::fs::AtFlags::empty(),
            )
            .map_err(|error| SourceRootError::CannotInspect {
                path: self.path.join(to),
                reason: error.to_string(),
            })?;
            self.remove_file_relative(from)?;
            to_parent
                .sync_all()
                .map_err(|error| SourceRootError::CannotInspect {
                    path: self.path.join(to),
                    reason: error.to_string(),
                })
        }
        #[cfg(windows)]
        {
            rename_windows_relative(self, from, to, false)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(SourceRootError::PermissionDenied {
                path: self.path.join(to),
                reason: "exclusive publication unsupported".into(),
            })
        }
    }

    /// Remove one source-relative regular file without following it.
    pub fn remove_file_relative(&self, relative: &Path) -> Result<(), SourceRootError> {
        self.authorize_relative(relative)?;
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
        self.authorize_relative(relative)?;
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

    /// Retained-handle directory admission with a hard entry allocation bound.
    #[cfg(any(unix, windows))]
    pub fn read_dir_relative_bounded(
        &self,
        relative: &Path,
        maximum: usize,
    ) -> Result<Vec<SourceEntry>, SourceRootError> {
        self.authorize_relative(relative)?;
        validate_relative(relative, true)?;
        #[cfg(unix)]
        {
            read_unix_dir_bounded(self, relative, maximum)
        }
        #[cfg(windows)]
        {
            read_windows_dir_bounded(self, relative, maximum)
        }
    }

    /// Traversal never crosses a mount/volume boundary from the selected root.
    pub fn same_filesystem(&self, relative: &Path) -> Result<bool, SourceRootError> {
        self.authorize_relative(relative)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let directory = open_unix_relative(self, relative, true)?;
            let metadata = directory
                .metadata()
                .map_err(|e| SourceRootError::CannotInspect {
                    path: self.path.join(relative),
                    reason: e.to_string(),
                })?;
            Ok(metadata.dev()
                == self
                    .handle
                    .metadata()
                    .map_err(|e| SourceRootError::CannotInspect {
                        path: self.path.clone(),
                        reason: e.to_string(),
                    })?
                    .dev())
        }
        #[cfg(windows)]
        {
            let directory = open_windows_relative(self, relative, true)?;
            let observed = windows_identity(&self.path.join(relative), &directory)?;
            Ok(
                matches!((&self.identity,observed),(SourceRootIdentity::Windows{volume_serial_number:a,..},SourceRootIdentity::Windows{volume_serial_number:b,..}) if *a==b),
            )
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(SourceRootError::IdentityUnavailable {
                path: self.path.clone(),
                reason: "filesystem authority unavailable".into(),
            })
        }
    }
    /// Scanner-only metadata enumeration. Ignore matching still happens in the
    /// descriptor walker; unrelated regular file names need no stat/open.
    pub fn read_scan_directory(
        &self,
        relative: &Path,
        maximum: usize,
    ) -> Result<Vec<SourceEntry>, SourceRootError> {
        self.authorize_relative(relative)?;
        #[cfg(unix)]
        {
            read_unix_dir_filtered(self, relative, maximum, true)
        }
        #[cfg(windows)]
        {
            Ok(read_windows_dir_bounded(self, relative, maximum)?
                .into_iter()
                .filter(|e| {
                    self.allows_relative(&relative.join(&e.name)) && scan_entry(&e.name, e.kind)
                })
                .collect())
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = maximum;
            Err(SourceRootError::IdentityUnavailable {
                path: self.path.clone(),
                reason: "descriptor scanner unavailable".into(),
            })
        }
    }
    /// List through the retained directory handle, refusing symlink/reparse parents.
    /// Entries are observations; callers must reopen files and revalidate after reads.
    pub fn read_dir_relative(&self, relative: &Path) -> Result<Vec<SourceEntry>, SourceRootError> {
        self.authorize_relative(relative)?;
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

    /// Confirm that `path` is still a plain directory. Matching is path-based:
    /// a different device or inode at the same path is accepted.
    pub fn verify(&self, path: &Path) -> Result<(), SourceRootError> {
        Self::capture(path).map(drop)
    }
}

/// Identity of the object behind an already-open directory handle, observed now.
fn handle_identity(path: &Path, handle: &fs::File) -> Result<SourceRootIdentity, SourceRootError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = handle
            .metadata()
            .map_err(|error| SourceRootError::IdentityUnavailable {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
        Ok(SourceRootIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    #[cfg(windows)]
    {
        windows_identity(path, handle)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, handle);
        Err(SourceRootError::UnsupportedPlatform)
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
        exclusions: Arc::new(Vec::new()),
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
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_TRAVERSE, SYNCHRONIZE,
    };

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    // Do not request list/read-data on the lifetime pin: native rename opens
    // its destination internally without sharing read-data access. Retain the
    // same denial of write/delete sharing and open listing handles only during
    // enumeration.
    const ROOT_ACCESS: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

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

    root.authorize_relative(relative)?;
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
        } else {
            // A FIFO or device swapped in under a file name must not block
            // the opening thread; regular files ignore O_NONBLOCK.
            flags |= OFlags::NONBLOCK;
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
    root.authorize_relative(relative)?;
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

fn scan_entry(name: &OsStr, kind: SourceEntryKind) -> bool {
    let path = Path::new(name);
    match kind {
        SourceEntryKind::Directory => name == ".git" || !name.to_string_lossy().starts_with('.'),
        SourceEntryKind::File => {
            name == ".gitignore"
                || name == ".ignore"
                || name == ".contextignore"
                || name == "exclude"
                || crate::scan::is_markdown(path, true)
        }
        _ => matches!(
            name.to_str(),
            Some(".ignore" | ".gitignore" | ".contextignore" | "exclude")
        ),
    }
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
    read_unix_dir_filtered(root, relative, maximum, false)
}
#[cfg(unix)]
fn read_unix_dir_filtered(
    root: &SourceRoot,
    relative: &Path,
    maximum: usize,
    scan: bool,
) -> Result<Vec<SourceEntry>, SourceRootError> {
    use std::os::unix::ffi::OsStrExt;

    use rustix::fs::{statat, AtFlags, Dir, FileType};

    let directory = open_unix_relative(root, relative, true)?;
    let mut stream = Dir::read_from(&directory)
        .map_err(|error| unix_source_error(root.path.join(relative), error))?;
    let mut entries = Vec::new();
    let mut examined = 0usize;
    for entry in &mut stream {
        let entry = entry.map_err(|error| unix_source_error(root.path.join(relative), error))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        examined = examined.saturating_add(1);
        if examined > maximum {
            return Err(SourceRootError::CannotInspect {
                path: root.path.join(relative),
                reason: "directory entry bound exceeded".into(),
            });
        }
        let name = std::ffi::OsStr::from_bytes(name);
        if !root.allows_relative(&relative.join(name)) {
            continue;
        }
        let declared = entry.file_type();
        if scan
            && ((declared.is_file() && !scan_entry(name, SourceEntryKind::File))
                || (declared.is_dir() && !scan_entry(name, SourceEntryKind::Directory))
                || (declared.is_symlink()
                    && !matches!(
                        name.to_str(),
                        Some(".ignore" | ".gitignore" | ".contextignore" | "exclude")
                    )))
        {
            continue;
        }
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
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_GENERIC_READ, FILE_READ_ATTRIBUTES, FILE_TRAVERSE, SYNCHRONIZE,
    };

    root.authorize_relative(relative)?;
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
            if expect_directory {
                FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE
            } else {
                FILE_GENERIC_READ | SYNCHRONIZE
            },
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
    use windows_sys::Wdk::Storage::FileSystem::{
        NtCreateFile, RtlNtStatusToDosErrorNoTeb, FILE_DIRECTORY_FILE,
    };
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
            if create_options & FILE_DIRECTORY_FILE != 0 {
                // Relative parent capabilities stay pinned for the whole
                // operation, just like the root; never allow their replacement
                // or write-based reparse mutation during sibling publication.
                FILE_SHARE_READ
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
            },
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
    replace: bool,
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
    // A non-null RootDirectory makes Windows reopen the destination directory
    // for FILE_WRITE_DATA, conflicting with the lifetime no-write-sharing pin.
    // Native callers stage sibling files: prove the parents are identical and
    // use the documented NULL-root simple-name rename. Never drop root pins to
    // accommodate a cross-directory move.
    if windows_identity(&root.path.join(from), &from_parent)?
        != windows_identity(&root.path.join(to), &to_parent)?
    {
        return Err(SourceRootError::CannotInspect {
            path: root.path.join(to),
            reason: "Windows pinned publication requires a sibling temporary file".into(),
        });
    }
    let wide: Vec<u16> = to_name.encode_wide().collect();
    let name_bytes = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| SourceRootError::InvalidRelative {
            path: to.to_path_buf(),
        })?;
    let header = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let byte_len = std::mem::size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(name_bytes as usize)
        .ok_or_else(|| SourceRootError::InvalidRelative {
            path: to.to_path_buf(),
        })?;
    let words = byte_len.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0usize; words];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: `storage` is aligned for FILE_RENAME_INFORMATION and has room for its
    // fixed header plus every UTF-16 byte copied below.
    unsafe {
        (*info).Anonymous = FILE_RENAME_INFORMATION_0 {
            ReplaceIfExists: replace,
        };
        (*info).RootDirectory = std::ptr::null_mut();
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
fn read_windows_dir(
    root: &SourceRoot,
    relative: &Path,
) -> Result<Vec<SourceEntry>, SourceRootError> {
    read_windows_dir_bounded(root, relative, usize::MAX)
}
#[cfg(windows)]
fn read_windows_dir_bounded(
    root: &SourceRoot,
    relative: &Path,
    maximum: usize,
) -> Result<Vec<SourceEntry>, SourceRootError> {
    windows_listing::read(root, relative, maximum)
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

        // Identities stay distinguishable (handle coherence relies on it); a
        // persisted value differing from the observed one is no longer an error.
        assert_eq!(expected, expected.clone());
        assert_ne!(expected, other_volume);
        assert_ne!(expected, other_file);
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
    fn a_live_handle_refuses_a_replaced_directory_but_restoring_the_path_adopts_it() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        let canonical = fs::canonicalize(&root).expect("canonical root");
        let admitted = SourceRoot::capture(canonical.clone()).expect("capture admitted root");

        fs::rename(&canonical, temp.path().join("moved")).expect("move admitted object");
        fs::create_dir(&canonical).expect("create replacement");

        // The retained handle names the moved-away object, so it must not act.
        assert!(matches!(
            admitted.verify(),
            Err(SourceRootError::IdentityChanged { .. })
        ));
        // The path still names a directory: restore adopts the new object.
        let restored = SourceRoot::restore(canonical.clone(), admitted.identity.clone())
            .expect("path-based restore");
        restored.verify().expect("restored root is coherent");
        assert_ne!(
            restored.observed_identity().unwrap(),
            admitted.observed_identity().unwrap()
        );
        assert_eq!(restored, admitted, "roots match by path");
        admitted
            .identity
            .verify(&canonical)
            .expect("stored identity is advisory");
    }

    #[cfg(unix)]
    #[test]
    fn a_renumbered_device_at_the_same_path_restores_with_the_observed_identity() {
        let temp = tempdir().expect("tempdir");
        let canonical = fs::canonicalize(temp.path()).expect("canonical root");
        let current = SourceRoot::capture(canonical.clone()).expect("capture");
        let SourceRootIdentity::Unix { device, inode } = current.identity.clone() else {
            unreachable!()
        };
        for stale in [
            SourceRootIdentity::Unix {
                device: device.wrapping_add(1),
                inode,
            },
            SourceRootIdentity::Unix {
                device: device.wrapping_add(1),
                inode: inode.wrapping_add(7),
            },
        ] {
            let restored = SourceRoot::restore(canonical.clone(), stale.clone()).expect("restore");
            assert_eq!(restored.identity, stale, "the persisted key is retained");
            assert_eq!(restored.observed_identity().unwrap(), current.identity);
            assert_eq!(restored, current);
            restored.verify().expect("verify");
            stale
                .verify(&canonical)
                .expect("path-based identity verify");
        }
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
