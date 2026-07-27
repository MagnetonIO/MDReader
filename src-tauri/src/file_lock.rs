//! Small cross-process advisory-lock primitive for app-data sidecars.
//!
//! Atomic replacement protects readers from partial bytes, but it does not make
//! a load → modify → replace sequence atomic across two MDReader processes. A
//! stable sidecar inode does: every cooperating process locks the same file even
//! while the protected JSON/log file is replaced or rotated.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

#[must_use = "dropping the guard releases the cross-process advisory lock"]
pub(crate) struct AdvisoryFileLock(File);

impl AdvisoryFileLock {
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "lock sidecar has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }

    pub(crate) fn acquire_file(file: File) -> io::Result<Self> {
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}

impl Drop for AdvisoryFileLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
