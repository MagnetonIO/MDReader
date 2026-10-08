//! Bind an already-open SQLite library to the retained original filesystem
//! object. A filename or pre/post stat alone cannot exclude open-time ABA.
use super::{ReaderError, Result};
use rusqlite::Connection;
#[cfg(unix)]
use rustix::fs::{statat, AtFlags, FileType, Stat};
use std::{
    path::Path,
    sync::{Arc, Weak},
};

/// Opaque live file/root proof tied to the actual ReaderStore lifetime. SQLite
/// retains its original file; cloning never adopts a replacement or outlives the
/// owner's certificate lifetime. Not a writer-freeze proof.
#[derive(Clone)]
pub struct LibraryIdentity {
    retained: Arc<Retained>,
    lifetime: Weak<()>,
}

// SQLite itself retains the original database file descriptor. Opening and
// closing another DB descriptor can release this process's POSIX SQLite locks.
// Certificates therefore retain only a weak live-owner proof and stat identity.
pub(super) struct LibraryOwner {
    // Drop this token before the connection; certificates never upgrade it.
    lifetime: Arc<()>,
    retained: Arc<Retained>,
}

struct Retained {
    #[cfg(unix)]
    root: crate::SourceRoot,
    #[cfg(unix)]
    object: Stat,
}

pub(super) fn certificate(owner: &LibraryOwner) -> LibraryIdentity {
    LibraryIdentity {
        retained: owner.retained.clone(),
        lifetime: Arc::downgrade(&owner.lifetime),
    }
}

impl LibraryIdentity {
    /// Require an explicitly selected profile to name this original library.
    /// No profile discovery or rebinding is performed.
    pub fn verify_at(&self, directory: &Path) -> Result<()> {
        self.check_live()?;
        let retained = &self.retained;
        verify_live(retained)?;
        #[cfg(unix)]
        if directory
            .canonicalize()
            .map_err(|_| ReaderError::Conflict)?
            != retained.root.path
        {
            return Err(ReaderError::Conflict);
        }
        #[cfg(not(unix))]
        let _ = directory;
        verify_live(retained)?;
        self.check_live()
    }

    /// Revalidate names through the retained root without opening SQLite or any
    /// database descriptor. Owner drop invalidates this certificate permanently.
    pub fn verify(&self) -> Result<()> {
        self.check_live()?;
        verify_live(&self.retained)?;
        self.check_live()
    }

    fn check_live(&self) -> Result<()> {
        // Never upgrade: an in-flight verifier must not keep the owner's
        // liveness token alive across SQLite connection teardown.
        if self.lifetime.strong_count() == 0 {
            return Err(ReaderError::Unavailable);
        }
        Ok(())
    }
}

#[cfg(unix)]
fn optional_object(root: &crate::SourceRoot) -> Result<Option<Stat>> {
    root.verify().map_err(|_| ReaderError::Conflict)?;
    let directory = root
        .try_clone_directory_handle()
        .map_err(|_| ReaderError::Conflict)?;
    let object = match statat(&directory, "library.sqlite", AtFlags::SYMLINK_NOFOLLOW) {
        Ok(object) => Some(object),
        Err(rustix::io::Errno::NOENT) => None,
        Err(_) => return Err(ReaderError::Conflict),
    };
    if let Some(object) = &object {
        if FileType::from_raw_mode(object.st_mode) != FileType::RegularFile || object.st_nlink != 1
        {
            return Err(ReaderError::Conflict);
        }
    }
    root.verify().map_err(|_| ReaderError::Conflict)?;
    Ok(object)
}

#[cfg(unix)]
fn named_object(root: &crate::SourceRoot) -> Result<Stat> {
    optional_object(root)?.ok_or(ReaderError::Conflict)
}

fn verify_live(identity: &Retained) -> Result<()> {
    #[cfg(unix)]
    {
        let named = named_object(&identity.root)?;
        if (named.st_dev, named.st_ino) != (identity.object.st_dev, identity.object.st_ino) {
            return Err(ReaderError::Conflict);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = identity;
        Err(ReaderError::Unavailable)
    }
}

pub(super) fn open_library(
    directory: &Path,
    create: bool,
    open: impl FnOnce(&Path) -> Result<Connection>,
) -> Result<(Connection, Option<LibraryOwner>)> {
    #[cfg(unix)]
    {
        let root = crate::SourceRoot::capture(directory.canonicalize()?)
            .map_err(|_| ReaderError::Conflict)?;
        // Reject preexisting symlinks/hardlinks before SQLite can follow or
        // initialize them; only a fresh writer may create an absent library.
        let before = optional_object(&root)?;
        if before.is_none() && !create {
            return Err(ReaderError::Unavailable);
        }
        let connection = open(&root.path.join("library.sqlite"))?;
        sqlite_file_has_not_moved(&connection)?;
        let object = named_object(&root)?;
        if before
            .is_some_and(|before| (before.st_dev, before.st_ino) != (object.st_dev, object.st_ino))
        {
            return Err(ReaderError::Conflict);
        }
        sqlite_file_has_not_moved(&connection)?;
        let identity = LibraryOwner {
            lifetime: Arc::new(()),
            retained: Arc::new(Retained { root, object }),
        };
        verify_connection(&connection, Some(&identity))?;
        Ok((connection, Some(identity)))
    }
    #[cfg(not(unix))]
    {
        let _ = create;
        Ok((open(&directory.join("library.sqlite"))?, None))
    }
}

pub(super) fn verify_connection(
    connection: &Connection,
    identity: Option<&LibraryOwner>,
) -> Result<()> {
    #[cfg(unix)]
    {
        let identity = identity.ok_or(ReaderError::Unavailable)?;
        verify_live(&identity.retained)?;
        let actual = connection.path().ok_or(ReaderError::Unavailable)?;
        if Path::new(actual)
            .canonicalize()
            .map_err(|_| ReaderError::Conflict)?
            != identity.retained.root.path.join("library.sqlite")
        {
            return Err(ReaderError::Conflict);
        }
        sqlite_file_has_not_moved(connection)?;
        verify_live(&identity.retained)
    }
    #[cfg(not(unix))]
    {
        let _ = (connection, identity);
        Ok(())
    }
}

/// The bundled Unix VFS compares its actual open file with the named database.
/// Unsupported VFS implementations do not issue a certificate.
#[cfg(unix)]
#[allow(unsafe_code)]
fn sqlite_file_has_not_moved(connection: &Connection) -> Result<()> {
    let mut moved: std::ffi::c_int = -1;
    // SAFETY: the shared borrow keeps this live Connection open for the entire
    // synchronous call. The static NUL-terminated main name and initialized int
    // out-pointer satisfy SQLITE_FCNTL_HAS_MOVED's documented ABI. No pointer or
    // SQLite/file ownership escapes, and this opcode performs no SQL mutation.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            std::ptr::addr_of_mut!(moved).cast(),
        )
    };
    if result != rusqlite::ffi::SQLITE_OK {
        return Err(ReaderError::Unavailable);
    }
    if moved != 0 {
        return Err(ReaderError::Conflict);
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
