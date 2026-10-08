//! The actual OS writer lease acquired BEFORE opening/migrating ReaderStore.
use super::{Error, Result};
use mdreader_core::SourceRoot;
use std::{fs::File, path::Path, sync::Arc};

pub(crate) struct StartupOwner {
    root: SourceRoot,
    lock: File,
    pid: u32,
}
impl StartupOwner {
    pub(crate) fn acquire(directory: &Path) -> Result<Arc<Self>> {
        let root = SourceRoot::capture(directory.canonicalize().map_err(|_| Error::Unavailable)?)
            .map_err(|_| Error::Conflict)?;
        let lock = root
            .open_lock_relative(Path::new("reader.lock"))
            .map_err(|_| Error::Unavailable)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::Busy)?;
        let owner = Arc::new(Self {
            root,
            lock,
            pid: std::process::id(),
        });
        owner.verify()?;
        Ok(owner)
    }
    pub(super) fn directory(&self) -> &Path {
        &self.root.path
    }
    pub(super) fn verify(&self) -> Result<()> {
        if self.pid != std::process::id() {
            return Err(Error::Conflict);
        }
        self.root.verify().map_err(|_| Error::Conflict)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // This is reader.lock, never a SQLite DB descriptor. Closing the
            // comparison descriptor cannot release SQLite's inode locks.
            let named = self
                .root
                .open_optional_file_relative(Path::new("reader.lock"))
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            let original = self.lock.metadata().map_err(|_| Error::Unavailable)?;
            let current = named.metadata().map_err(|_| Error::Unavailable)?;
            if current.nlink() != 1
                || (original.dev(), original.ino()) != (current.dev(), current.ino())
            {
                return Err(Error::Conflict);
            }
        }
        #[cfg(not(unix))]
        let _ = &self.lock;
        Ok(())
    }
}
