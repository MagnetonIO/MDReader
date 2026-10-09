//! Historical managed filenames are confined to native migration preservation.
//! These helpers neither open a database nor admit a source.
#[cfg(test)]
use mdreader_core::SourceRoot;
use mdreader_core::SourceRootIdentity;
use std::path::{Path, PathBuf};

/// Locate historical per-source state for preservation and cleanup only.
///
/// Stable object identity determines the key, so a replacement directory at
/// the same canonical path cannot inherit an older source object's index and a
/// rename does not orphan the existing index.
#[cfg(test)]
pub(crate) fn workspace_index_path(base: &Path, root: &SourceRoot) -> PathBuf {
    workspace_index_path_for_stable_identity(base, &root.identity)
}

/// Locate historical state for a persisted identity; never a new reader index.
pub(crate) fn workspace_index_path_for_stable_identity(
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
/// reader state must use the ReaderStore SQLite layout instead.
pub(crate) fn legacy_workspace_index_path_for_path_identity(
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn historical_keys_retain_full_windows_object_identity() {
        let base = Path::new("/private/app-data");
        let original = SourceRootIdentity::Windows {
            volume_serial_number: 1,
            file_id: [17; 16],
        };
        let other_volume = SourceRootIdentity::Windows {
            volume_serial_number: (1_u64 << 32) + 1,
            file_id: [17; 16],
        };
        let other_file = SourceRootIdentity::Windows {
            volume_serial_number: 1,
            file_id: [18; 16],
        };
        assert_ne!(
            workspace_index_path_for_stable_identity(base, &original),
            workspace_index_path_for_stable_identity(base, &other_volume)
        );
        assert_ne!(
            workspace_index_path_for_stable_identity(base, &original),
            workspace_index_path_for_stable_identity(base, &other_file)
        );
    }
    #[test]
    fn historical_address_survives_source_rename() {
        let directory = tempfile::tempdir().unwrap();
        let before = SourceRoot::capture(directory.path().to_path_buf()).unwrap();
        let mut after = before.clone();
        after.path = directory.path().join("renamed-address");
        assert_eq!(
            workspace_index_path(Path::new("/app-data"), &before),
            workspace_index_path(Path::new("/app-data"), &after)
        );
    }
}
