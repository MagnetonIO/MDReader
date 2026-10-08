//! Retain the selected namespace before any freeze, rather than adopt a later
//! replacement and accidentally omit the original queued/uncertain outputs.
use super::*;

pub(super) struct Namespace {
    #[cfg(unix)]
    roots: Vec<SourceRoot>,
    #[cfg(unix)]
    queue: File,
}
impl Namespace {
    pub fn capture(profile: &SourceRoot) -> Result<Self> {
        #[cfg(not(unix))]
        {
            let _ = profile;
            Ok(Self {})
        }
        #[cfg(unix)]
        {
            // Busy is not a quiescence proof. It means the normal open path
            // initialized/verified directories and another real writer holds
            // queue.lock. Retain identities now; freeze will drain that lock.
            match Spool::open(profile.path.join("captures"), Limits::default()) {
                Ok(_) | Err(CaptureError::Busy) => {}
                Err(CaptureError::UnsupportedPlatform) => return Err(Error::Unsupported),
                Err(CaptureError::UnsafePath) => return Err(Error::Conflict),
                Err(_) => return Err(Error::Unavailable),
            }
            let mut roots = Vec::new();
            let captures = profile.path.join("captures");
            for relative in ["", "pending", "markdown", "receipts", "temporary"] {
                roots.push(
                    SourceRoot::capture(captures.join(relative)).map_err(|_| Error::Conflict)?,
                );
            }
            let queue = roots[0]
                .open_optional_file_relative(Path::new("queue.lock"))
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            let namespace = Self { roots, queue };
            namespace.verify()?;
            Ok(namespace)
        }
    }
    pub fn verify(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            for root in &self.roots {
                root.verify().map_err(|_| Error::Conflict)?;
            }
            let queue = self.roots[0]
                .open_optional_file_relative(Path::new("queue.lock"))
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            let expected = self.queue.metadata().map_err(|_| Error::Unavailable)?;
            let current = queue.metadata().map_err(|_| Error::Unavailable)?;
            if current.nlink() != 1
                || (expected.dev(), expected.ino()) != (current.dev(), current.ino())
            {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }
}
