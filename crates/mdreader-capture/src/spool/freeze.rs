//! Preservation-only cross-process exclusion using the actual mutation lock.
use super::*;

/// Owns the exclusive queue lock and retained namespace objects. It neither
/// drains nor rewrites pending events, projections, receipts or temporary files.
/// No Clone/Debug/serialization: retain in the real service process throughout
/// preservation, never treat a remote acknowledgement as this capability.
pub struct FrozenSpool {
    spool: Spool,
    lock: QueueLock,
    #[cfg(unix)]
    objects: Vec<(PathBuf, File)>,
    #[cfg(windows)]
    directories: Vec<(PathBuf, platform::windows::PrivateDirectory)>,
    released: bool,
}
impl Spool {
    /// Nonblocking. The service may retry only Busy until its bounded deadline,
    /// without retaining a ReaderStore mutex. All supported hook mutation paths
    /// acquire this same lock through Spool; no empty-queue assumption is made.
    pub fn freeze(self) -> Result<FrozenSpool> {
        #[cfg(not(any(unix, windows)))]
        return Err(CaptureError::UnsupportedPlatform);
        #[cfg(windows)]
        {
            let lock = self.lock()?;
            let mut directories = Vec::new();
            for relative in ["", "pending", "markdown", "receipts", "temporary"] {
                let path = self.root.join(relative);
                let retained = platform::windows::PrivateDirectory::open(&path)?;
                directories.push((path, retained));
            }
            let frozen = FrozenSpool {
                spool: self,
                lock,
                directories,
                released: false,
            };
            frozen.revalidate()?;
            Ok(frozen)
        }
        #[cfg(unix)]
        {
            let lock = self.lock()?;
            let mut objects = Vec::new();
            for relative in ["", "pending", "markdown", "receipts", "temporary"] {
                let path = self.root.join(relative);
                let file = File::open(&path)?;
                objects.push((path, file));
            }
            objects.push((self.root.join("queue.lock"), lock.try_clone()?));
            let frozen = FrozenSpool {
                spool: self,
                lock,
                objects,
                released: false,
            };
            frozen.revalidate()?;
            Ok(frozen)
        }
    }
}
impl FrozenSpool {
    pub fn root(&self) -> &Path {
        self.spool.root()
    }
    /// Reopen every exact pathname and compare it to its retained object. A
    /// detached old descriptor is insufficient after rename/unlink replacement.
    pub fn revalidate(&self) -> Result<()> {
        if self.released {
            return Err(CaptureError::UnsafePath);
        }
        self.spool.verify()?;
        #[cfg(not(any(unix, windows)))]
        return Err(CaptureError::UnsupportedPlatform);
        #[cfg(windows)]
        {
            for (path, retained) in &self.directories {
                retained.verify()?;
                let named = platform::windows::PrivateDirectory::open(path)?;
                if retained.identity()? != named.identity()? {
                    return Err(CaptureError::UnsafePath);
                }
            }
            // The retained QueueLock owns the same byte lock used by EVERY
            // mutation path. The named opener neither creates nor repairs it.
            let root = platform::windows::PrivateDirectory::open(self.spool.root())?;
            let named = root.open_existing_lock(Path::new("queue.lock"))?;
            if !platform::windows::same_open_file(&self.lock, &named)? {
                return Err(CaptureError::UnsafePath);
            }
            Ok(())
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            for (path, retained) in &self.objects {
                let expected = retained.metadata()?;
                let named = fs::symlink_metadata(path)?;
                if named.file_type().is_symlink()
                    || (expected.dev(), expected.ino(), expected.is_dir())
                        != (named.dev(), named.ino(), named.is_dir())
                    || (!named.is_dir() && (!named.is_file() || named.nlink() != 1))
                {
                    return Err(CaptureError::UnsafePath);
                }
                let reopened = File::open(path)?.metadata()?;
                if (reopened.dev(), reopened.ino()) != (expected.dev(), expected.ino()) {
                    return Err(CaptureError::UnsafePath);
                }
            }
            Ok(())
        }
    }
    /// Preservation-only release. On an error the caller must quarantine and
    /// retain this guard, rather than claim that hook admission safely resumed.
    pub fn release(&mut self) -> Result<()> {
        self.revalidate()?;
        fs2::FileExt::unlock(&*self.lock)?;
        self.released = true;
        Ok(())
    }
}
