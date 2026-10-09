//! Bounded private OS handoff copies. Copies survive presentation for one hour;
//! only generated copies are cleaned through retained directory handles. Each
//! copy has a unique private directory and its original display filename.
mod cleanup;
use mdreader_core::SourceRoot;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub(super) const MAX_COPY: u64 = 20 * 1024 * 1024;
pub(super) struct Copy {
    pub root: SourceRoot,
    pub path: PathBuf,
    pub file: File,
}
fn failure<T>(_: T) -> String {
    "transfer_unconfirmed".into()
}
#[cfg(unix)]
fn private(metadata: &fs::Metadata, directory: bool) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    metadata.permissions().mode() & 0o077 == 0 && (directory || metadata.nlink() == 1)
}
#[cfg(not(unix))]
fn private(_: &fs::Metadata, _: bool) -> bool {
    false
}
pub(super) fn create(cache: &Path, name: &str) -> Result<Copy, String> {
    cleanup::filename(name)?;
    fs::create_dir_all(cache).map_err(failure)?;
    let parent = SourceRoot::capture(cache.canonicalize().map_err(failure)?).map_err(failure)?;
    let root = parent
        .create_or_capture_directory_relative(Path::new("mdreader-transfers-v1"))
        .map_err(failure)?;
    let meta = fs::symlink_metadata(&root.path).map_err(failure)?;
    if !meta.is_dir() || !private(&meta, true) {
        return Err("unavailable".into());
    }
    cleanup::expired(&root)?;
    let root = root
        .create_or_capture_directory_relative(Path::new(&uuid::Uuid::new_v4().to_string()))
        .map_err(failure)?;
    let file = root.create_new_relative(Path::new(name)).map_err(failure)?;
    let path = root.path.join(name);
    Ok(Copy { root, path, file })
}

/// Completed exports and cancelled sheets no longer need a handoff copy. An
/// Android share recipient may still read its URI, so presented shares expire.
pub(super) fn release(cache: &Path, path: &Path) -> Result<(), String> {
    let parent = SourceRoot::capture(cache.canonicalize().map_err(failure)?).map_err(failure)?;
    let root = parent
        .create_or_capture_directory_relative(Path::new("mdreader-transfers-v1"))
        .map_err(failure)?;
    let relative = path.strip_prefix(&root.path).map_err(failure)?;
    let directory = relative.parent().ok_or("unavailable")?;
    if directory.components().count() != 1
        || uuid::Uuid::parse_str(directory.to_str().ok_or("unavailable")?).is_err()
    {
        return Err("unavailable".into());
    }
    let name = relative
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("unavailable")?;
    cleanup::filename(name)?;
    let child = root
        .create_or_capture_directory_relative(directory)
        .map_err(failure)?;
    child
        .remove_file_relative(Path::new(name))
        .map_err(failure)?;
    root.remove_empty_directory_relative(directory)
        .map_err(failure)
}
impl Copy {
    pub(super) fn finish(self) -> Result<(PathBuf, u32, String), String> {
        self.file.sync_all().map_err(failure)?;
        let size = self.file.metadata().map_err(failure)?.len();
        if size > MAX_COPY {
            return Err("too_large".into());
        }
        let relative = self.path.file_name().ok_or("unavailable")?;
        let mut input = self
            .root
            .open_optional_file_relative(Path::new(relative))
            .map_err(failure)?
            .ok_or("unavailable")?;
        let original = same_file::Handle::from_file(self.file.try_clone().map_err(failure)?)
            .map_err(failure)?;
        let reopened =
            same_file::Handle::from_file(input.try_clone().map_err(failure)?).map_err(failure)?;
        if original != reopened {
            return Err("transfer_unconfirmed".into());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        let mut total = 0;
        loop {
            let n = input.read(&mut buffer).map_err(failure)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_COPY {
                return Err("too_large".into());
            }
            hash.update(&buffer[..n]);
        }
        if total != size || input.metadata().map_err(failure)?.len() != size {
            return Err("transfer_unconfirmed".into());
        }
        self.root.verify().map_err(failure)?;
        File::open(&self.root.path)
            .and_then(|f| f.sync_all())
            .map_err(failure)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.file
                .set_permissions(fs::Permissions::from_mode(0o400))
                .map_err(failure)?;
        }
        Ok((self.path, size as u32, format!("{:x}", hash.finalize())))
    }
}
