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
    #[cfg(any(unix, windows))]
    root: crate::SourceRoot,
    #[cfg(unix)]
    object: Stat,
    #[cfg(windows)]
    object: windows::Object,
    #[cfg(any(unix, windows))]
    name: &'static str,
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
        #[cfg(any(unix, windows))]
        if directory
            .canonicalize()
            .map_err(|_| ReaderError::Conflict)?
            != retained.root.path
        {
            return Err(ReaderError::Conflict);
        }
        #[cfg(not(any(unix, windows)))]
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

    #[cfg(windows)]
    pub(super) fn verify_private(&self) -> Result<()> {
        self.verify()?;
        let root = &self.retained.root;
        let private = mdreader_capture::PrivateDirectory::open(&root.path)
            .map_err(|_| ReaderError::Conflict)?;
        let file = root
            .open_optional_file_relative(Path::new(self.retained.name))
            .map_err(|_| ReaderError::Conflict)?
            .ok_or(ReaderError::Conflict)?;
        mdreader_capture::observe_object(&file, false, true).map_err(|_| ReaderError::Conflict)?;
        private.verify().map_err(|_| ReaderError::Conflict)?;
        self.verify()
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
fn optional_object(root: &crate::SourceRoot, name: &str) -> Result<Option<Stat>> {
    root.verify().map_err(|_| ReaderError::Conflict)?;
    let directory = root
        .try_clone_directory_handle()
        .map_err(|_| ReaderError::Conflict)?;
    let object = match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
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
fn named_object(root: &crate::SourceRoot, name: &str) -> Result<Stat> {
    optional_object(root, name)?.ok_or(ReaderError::Conflict)
}

fn verify_live(identity: &Retained) -> Result<()> {
    #[cfg(unix)]
    {
        let named = named_object(&identity.root, identity.name)?;
        if (named.st_dev, named.st_ino) != (identity.object.st_dev, identity.object.st_ino) {
            return Err(ReaderError::Conflict);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        if windows::named(&identity.root, identity.name)? != identity.object {
            return Err(ReaderError::Conflict);
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = identity;
        Err(ReaderError::Unavailable)
    }
}

/// Both main databases and the index's attached authority belong to this
/// exact live store. Verifying names is not a writer-drain proof.
#[derive(Clone)]
pub struct StoreIdentity {
    pub(super) library: LibraryIdentity,
    pub(super) index: LibraryIdentity,
}
impl StoreIdentity {
    pub fn verify_at(&self, directory: &Path) -> Result<()> {
        self.library.verify_at(directory)?;
        self.index.verify_at(directory)
    }
}

pub(super) fn open_library(
    directory: &Path,
    create: bool,
    open: impl FnOnce(&Path) -> Result<Connection>,
) -> Result<(Connection, Option<LibraryOwner>)> {
    open_named(directory, "library.sqlite", create, open)
}

pub(super) fn open_index(
    directory: &Path,
    create: bool,
    open: impl FnOnce(&Path) -> Result<Connection>,
) -> Result<(Connection, Option<LibraryOwner>)> {
    open_named(directory, "index.sqlite", create, open)
}

fn open_named(
    directory: &Path,
    name: &'static str,
    create: bool,
    open: impl FnOnce(&Path) -> Result<Connection>,
) -> Result<(Connection, Option<LibraryOwner>)> {
    #[cfg(unix)]
    {
        let root = crate::SourceRoot::capture(directory.canonicalize()?)
            .map_err(|_| ReaderError::Conflict)?;
        // Reject preexisting symlinks/hardlinks before SQLite can follow or
        // initialize them; only a fresh writer may create an absent library.
        let before = optional_object(&root, name)?;
        if before.is_none() && !create {
            return Err(ReaderError::Unavailable);
        }
        let connection = open(&root.path.join(name))?;
        sqlite_file_has_not_moved(&connection, c"main")?;
        let object = named_object(&root, name)?;
        if before
            .is_some_and(|before| (before.st_dev, before.st_ino) != (object.st_dev, object.st_ino))
        {
            return Err(ReaderError::Conflict);
        }
        sqlite_file_has_not_moved(&connection, c"main")?;
        let identity = LibraryOwner {
            lifetime: Arc::new(()),
            retained: Arc::new(Retained { root, object, name }),
        };
        verify_connection(&connection, Some(&identity))?;
        Ok((connection, Some(identity)))
    }
    #[cfg(windows)]
    {
        windows::open(directory, name, create, open)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = create;
        Ok((open(&directory.join(name))?, None))
    }
}

pub(super) fn verify_connection(
    connection: &Connection,
    identity: Option<&LibraryOwner>,
) -> Result<()> {
    verify_database(connection, identity, "main", c"main")
}

pub(super) fn verify_attached(
    connection: &Connection,
    identity: Option<&LibraryOwner>,
) -> Result<()> {
    verify_database(
        connection,
        identity,
        "reader_authority",
        c"reader_authority",
    )
}

fn verify_database(
    connection: &Connection,
    identity: Option<&LibraryOwner>,
    sql_name: &'static str,
    c_name: &'static std::ffi::CStr,
) -> Result<()> {
    #[cfg(unix)]
    {
        let identity = identity.ok_or(ReaderError::Unavailable)?;
        verify_live(&identity.retained)?;
        let actual: String = connection
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name=?1",
                [sql_name],
                |row| row.get(0),
            )
            .map_err(|_| ReaderError::Unavailable)?;
        if Path::new(&actual)
            .canonicalize()
            .map_err(|_| ReaderError::Conflict)?
            != identity.retained.root.path.join(identity.retained.name)
        {
            return Err(ReaderError::Conflict);
        }
        sqlite_file_has_not_moved(connection, c_name)?;
        verify_live(&identity.retained)
    }
    #[cfg(windows)]
    {
        windows::verify(connection, identity, sql_name, c_name)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (connection, identity, sql_name, c_name);
        Ok(())
    }
}

/// The bundled Unix VFS compares its actual open file with the named database.
/// Unsupported VFS implementations do not issue a certificate.
#[cfg(unix)]
#[allow(unsafe_code)]
fn sqlite_file_has_not_moved(
    connection: &Connection,
    database: &'static std::ffi::CStr,
) -> Result<()> {
    let mut moved: std::ffi::c_int = -1;
    // SAFETY: the shared borrow keeps this live Connection open for the entire
    // synchronous call. The static NUL-terminated database name and initialized int
    // out-pointer satisfy SQLITE_FCNTL_HAS_MOVED's documented ABI. No pointer or
    // SQLite/file ownership escapes, and this opcode performs no SQL mutation.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            database.as_ptr(),
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

#[cfg(windows)]
mod windows;
