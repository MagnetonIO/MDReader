//! Bounded private OS handoff copies. Copies survive presentation for one hour;
//! only generated regular files are cleaned through retained directory handles.
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
fn cleanup(root: &SourceRoot) -> Result<(), String> {
    let mut observed = 0;
    let mut retained = 0;
    let mut bytes = 0;
    for item in fs::read_dir(&root.path).map_err(failure)?.take(65) {
        observed += 1;
        if observed > 64 {
            return Err("too_large".into());
        }
        let item = item.map_err(failure)?;
        let name = item.file_name();
        let text = name.to_str().ok_or("unavailable")?;
        // Generated UUID precedes the display name, never an original locator.
        if text.len() < 38
            || text
                .get(..36)
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .is_none()
            || text.as_bytes().get(36) != Some(&b'-')
            || !matches!(
                Path::new(text).extension().and_then(|x| x.to_str()),
                Some("md" | "zip")
            )
        {
            return Err("unavailable".into());
        }
        let file = root
            .open_optional_file_relative(Path::new(&name))
            .map_err(failure)?
            .ok_or("unavailable")?;
        let meta = file.metadata().map_err(failure)?;
        if !meta.is_file() || !private(&meta, false) || meta.len() > MAX_COPY {
            return Err("unavailable".into());
        }
        let age = SystemTime::now()
            .duration_since(meta.modified().map_err(failure)?)
            .unwrap_or_default();
        if age > Duration::from_secs(3600) {
            root.remove_file_relative(Path::new(&name))
                .map_err(failure)?;
        } else {
            retained += 1;
            bytes += meta.len();
        }
    }
    root.verify().map_err(failure)?;
    if retained >= 3 || bytes > 60 * 1024 * 1024 {
        return Err("busy".into());
    }
    Ok(())
}
pub(super) fn create(cache: &Path, name: &str) -> Result<Copy, String> {
    fs::create_dir_all(cache).map_err(failure)?;
    let parent = SourceRoot::capture(cache.canonicalize().map_err(failure)?).map_err(failure)?;
    let root = parent
        .create_or_capture_directory_relative(Path::new("mdreader-transfers-v1"))
        .map_err(failure)?;
    let meta = fs::symlink_metadata(&root.path).map_err(failure)?;
    if !meta.is_dir() || !private(&meta, true) {
        return Err("unavailable".into());
    }
    cleanup(&root)?;
    let filename = format!("{}-{name}", uuid::Uuid::new_v4());
    let file = root
        .create_new_relative(Path::new(&filename))
        .map_err(failure)?;
    let path = root.path.join(filename);
    Ok(Copy { root, path, file })
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
