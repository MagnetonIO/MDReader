use crate::*;
use std::fs::OpenOptions;

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(CaptureError::UnsafePath);
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(path)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(CaptureError::UnsafePath);
        }
    }
    Ok(())
}
pub(crate) fn ensure_regular_or_missing(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(CaptureError::UnsafePath),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn regular_exists(path: &Path) -> Result<bool> {
    ensure_regular_or_missing(path)?;
    Ok(path.exists())
}
pub(crate) fn private_open(path: &Path, create_new: bool) -> Result<File> {
    ensure_regular_or_missing(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
pub(crate) fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    ensure_regular_or_missing(path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(CaptureError::TooLarge);
    }
    Ok(bytes)
}
#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub(crate) struct DirectoryAuthority {
    identity: (u64, u64),
}
impl DirectoryAuthority {
    pub(crate) fn capture(root: &Path) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(root)?;
        Ok(Self {
            identity: (m.dev(), m.ino()),
        })
    }
    pub(crate) fn verify(&self, root: &Path) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(root)?;
        if (m.dev(), m.ino()) != self.identity {
            return Err(CaptureError::UnsafePath);
        }
        Ok(())
    }
}
pub(crate) fn lock_would_block(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
}

pub(crate) fn replace(_file: &File, temporary: &Path, destination: &Path) -> Result<()> {
    fs::rename(temporary, destination)?;
    Ok(())
}
pub(crate) fn confirm_file(path: &Path) -> Result<()> {
    ensure_regular_or_missing(path)?;
    File::open(path)?.sync_all()?;
    Ok(())
}
