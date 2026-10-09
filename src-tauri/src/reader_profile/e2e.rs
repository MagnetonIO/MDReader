//! Explicit disposable desktop acceptance roots. Never compiled into production.
use mdreader_core::SourceRoot;
use std::{io::Read, path::Path};

const MARKER: &[u8] = b"mdreader-native-e2e-profile-v1";

pub(super) fn capture(
    identifier: &str,
    path: &Path,
    conflicting: bool,
) -> Result<SourceRoot, String> {
    let invalid = || "Invalid isolated E2E reader profile".to_string();
    if identifier != "dev.mdreader.app.e2e" || conflicting || !path.is_absolute() {
        return Err(invalid());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(invalid());
        }
    }
    #[cfg(windows)]
    mdreader_capture::PrivateDirectory::open(path).map_err(|_| invalid())?;
    let canonical = path.canonicalize().map_err(|_| invalid())?;
    let temporary = std::env::temp_dir().canonicalize().map_err(|_| invalid())?;
    if canonical.parent() != Some(temporary.as_path())
        || !canonical
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("mdreader-e2e-"))
    {
        return Err(invalid());
    }
    let root = SourceRoot::capture(canonical).map_err(|_| invalid())?;
    let mut marker = root
        .open_file_relative(Path::new(".mdreader-e2e-profile-v1"))
        .map_err(|_| invalid())?;
    let mut bytes = Vec::new();
    (&mut marker)
        .take(65)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes != MARKER {
        return Err(invalid());
    }
    root.verify().map_err(|_| invalid())?;
    Ok(root)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn disposable_root_requires_test_identity_marker_and_no_profile_conflict() {
        let temporary = tempfile::Builder::new()
            .prefix("mdreader-e2e-")
            .tempdir()
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let marker = temporary.path().join(".mdreader-e2e-profile-v1");
        assert!(capture("dev.mdreader.app.e2e", temporary.path(), false).is_err());
        std::fs::write(&marker, MARKER).unwrap();
        assert!(capture("dev.mdreader.app", temporary.path(), false).is_err());
        assert!(capture("dev.mdreader.app.e2e", temporary.path(), true).is_err());
        let root = capture("dev.mdreader.app.e2e", temporary.path(), false).unwrap();
        assert_eq!(root.path, temporary.path().canonicalize().unwrap());
        assert_eq!(std::fs::read(marker).unwrap(), MARKER);
    }
}
