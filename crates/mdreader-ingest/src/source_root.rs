//! Stable filesystem-object identity for local library roots.
//!
//! A canonical path is only a name. The directory behind that name can be
//! renamed away and replaced without changing the path string, so every active
//! source keeps the identity captured from an opened directory handle and
//! revalidates it before using the root again.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    /// Windows volume serial number and file index, obtained from an opened
    /// directory handle.
    Windows {
        /// Serial number of the containing volume.
        volume_serial_number: u32,
        /// Stable file identifier within the volume.
        file_index: u64,
    },
}

/// A native canonical path bound to the filesystem object admitted there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRoot {
    /// Canonical native path used to address the root.
    pub path: PathBuf,
    /// Stable identity of the directory object at `path`.
    pub identity: SourceRootIdentity,
}

impl SourceRoot {
    /// Bind an already-canonical directory path to its opened object identity.
    pub fn capture(path: PathBuf) -> Result<Self, SourceRootError> {
        let identity = SourceRootIdentity::capture(&path)?;
        Ok(Self { path, identity })
    }

    /// Revalidate both the path shape and the opened object identity.
    pub fn verify(&self) -> Result<(), SourceRootError> {
        self.identity.verify(&self.path)
    }
}

/// A typed refusal to treat a path as the previously admitted source root.
#[derive(Debug)]
pub enum SourceRootError {
    /// The path or its metadata could not be opened.
    CannotInspect {
        /// Root path being inspected.
        path: PathBuf,
        /// Operating-system failure.
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
    /// The platform has no supported stable filesystem-object identity.
    #[cfg(not(any(unix, windows)))]
    UnsupportedPlatform,
}

impl std::fmt::Display for SourceRootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CannotInspect { path, reason } => {
                write!(
                    formatter,
                    "cannot inspect source root {}: {reason}",
                    path.display()
                )
            }
            Self::NotDirectory { path } => {
                write!(
                    formatter,
                    "source root is not a directory: {}",
                    path.display()
                )
            }
            Self::SymlinkOrReparse { path } => write!(
                formatter,
                "source root identity changed: {} is a symlink or reparse point",
                path.display()
            ),
            Self::IdentityUnavailable { path, reason } => write!(
                formatter,
                "source root identity is unavailable for {}: {reason}",
                path.display()
            ),
            Self::IdentityChanged { path } => write!(
                formatter,
                "source root identity changed since it was admitted: {}",
                path.display()
            ),
            Self::ChangedDuringValidation { path } => write!(
                formatter,
                "source root changed while its identity was being validated: {}",
                path.display()
            ),
            #[cfg(not(any(unix, windows)))]
            Self::UnsupportedPlatform => {
                formatter.write_str("stable source root identity is unsupported on this platform")
            }
        }
    }
}

impl std::error::Error for SourceRootError {}

impl SourceRootIdentity {
    /// Capture identity from an opened directory while rejecting a path-level
    /// symlink/reparse substitution before and after the open.
    pub fn capture(path: &Path) -> Result<Self, SourceRootError> {
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
/// Both native path units and stable object identity participate in the key, so
/// a replacement directory at the same canonical path cannot inherit an older
/// source object's index.
pub fn workspace_index_path(base: &Path, root: &SourceRoot) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-source-root-v2\0");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hasher.update(b"unix-path\0");
        hasher.update(root.path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        hasher.update(b"windows-path\0");
        for unit in root.path.as_os_str().encode_wide() {
            hasher.update(&unit.to_le_bytes());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        hasher.update(b"platform-path\0");
        hasher.update(root.path.to_string_lossy().as_bytes());
    }
    match &root.identity {
        SourceRootIdentity::Unix { device, inode } => {
            hasher.update(b"unix-identity\0");
            hasher.update(&device.to_le_bytes());
            hasher.update(&inode.to_le_bytes());
        }
        SourceRootIdentity::Windows {
            volume_serial_number,
            file_index,
        } => {
            hasher.update(b"windows-identity\0");
            hasher.update(&volume_serial_number.to_le_bytes());
            hasher.update(&file_index.to_le_bytes());
        }
    }
    let hash = hasher.finalize().to_hex().to_string();
    base.join("workspaces").join(hash).join("catdb.db")
}

#[cfg(unix)]
fn capture_unix(path: &Path) -> Result<SourceRootIdentity, SourceRootError> {
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
    Ok(opened_identity)
}

#[cfg(windows)]
fn capture_windows(path: &Path) -> Result<SourceRootIdentity, SourceRootError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

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
        .access_mode(0)
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
        .access_mode(0)
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
    Ok(opened_identity)
}

#[cfg(windows)]
fn windows_identity(path: &Path, opened: &fs::File) -> Result<SourceRootIdentity, SourceRootError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    // SAFETY: `information` is a plain Windows output structure initialized to
    // zero, and the handle remains owned by `opened` for the duration of the
    // call. `GetFileInformationByHandle` writes exactly that structure.
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let succeeded =
        unsafe { GetFileInformationByHandle(opened.as_raw_handle().cast(), &mut information) };
    if succeeded == 0 {
        return Err(SourceRootError::IdentityUnavailable {
            path: path.to_path_buf(),
            reason: std::io::Error::last_os_error().to_string(),
        });
    }
    if is_windows_reparse_point(information.dwFileAttributes) {
        return Err(SourceRootError::SymlinkOrReparse {
            path: path.to_path_buf(),
        });
    }
    let volume_serial_number = information.dwVolumeSerialNumber;
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(SourceRootIdentity::Windows {
        volume_serial_number,
        file_index,
    })
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
    fn windows_identity_comparison_uses_both_volume_and_file_id() {
        let expected = SourceRootIdentity::Windows {
            volume_serial_number: 41,
            file_index: 9001,
        };
        let other_volume = SourceRootIdentity::Windows {
            volume_serial_number: 42,
            file_index: 9001,
        };
        let other_file = SourceRootIdentity::Windows {
            volume_serial_number: 41,
            file_index: 9002,
        };

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
}
