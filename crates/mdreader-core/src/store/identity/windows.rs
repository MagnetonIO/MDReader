//! Bind the bundled Windows SQLite VFS handle to the full NTFS file identifier.
//! No borrowed HANDLE ownership escapes, and no second SQLite owner is opened.
use super::*;
use mdreader_capture::WindowsObjectStamp;
use std::{ffi::CStr, fs::File};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Object(u64, u64, i64); // Volume + both halves of the 128-bit ID.
fn object(stamp: WindowsObjectStamp) -> Object {
    Object(stamp.0, stamp.1, stamp.3)
}
fn optional(root: &crate::SourceRoot, name: &str) -> Result<Option<Object>> {
    root.verify().map_err(|_| ReaderError::Conflict)?;
    let file = root
        .open_optional_file_relative(Path::new(name))
        .map_err(|_| ReaderError::Conflict)?;
    let observed = file
        .as_ref()
        .map(|file| {
            mdreader_capture::observe_object(file, false, false)
                .map(object)
                .map_err(|_| ReaderError::Conflict)
        })
        .transpose()?;
    root.verify().map_err(|_| ReaderError::Conflict)?;
    Ok(observed)
}
pub(super) fn named(root: &crate::SourceRoot, name: &str) -> Result<Object> {
    optional(root, name)?.ok_or(ReaderError::Conflict)
}

// This boundary mirrors the Unix HAS_MOVED adapter. SQLite's public file control
// returns its actual borrowed Win32 handle; it is never adopted or closed.
#[allow(unsafe_code)]
fn actual(connection: &Connection, database: &'static CStr) -> Result<Object> {
    use std::{
        mem::ManuallyDrop,
        os::windows::io::{FromRawHandle, RawHandle},
    };
    let mut handle: RawHandle = std::ptr::null_mut();
    // SAFETY: shared Connection borrow, static terminated database name and an
    // initialized native-HANDLE output match SQLITE_FCNTL_WIN32_GET_HANDLE's ABI.
    let status = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            database.as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
            std::ptr::addr_of_mut!(handle).cast(),
        )
    };
    if status != rusqlite::ffi::SQLITE_OK || handle.is_null() || handle as isize == -1 {
        return Err(ReaderError::Unavailable);
    }
    // SAFETY: view exists only under this shared borrow. ManuallyDrop prevents
    // closing SQLite's HANDLE on every return/error path; no view escapes.
    let borrowed = ManuallyDrop::new(unsafe { File::from_raw_handle(handle) });
    mdreader_capture::observe_object(&borrowed, false, false)
        .map(object)
        .map_err(|_| ReaderError::Conflict)
}

pub(super) fn open(
    directory: &Path,
    name: &'static str,
    create: bool,
    open: impl FnOnce(&Path) -> Result<Connection>,
) -> Result<(Connection, Option<LibraryOwner>)> {
    // Capture before canonicalization: reparse points are never normalized into
    // an inferred grant. Revalidate the original object after path normalization.
    let selected =
        crate::SourceRoot::capture(directory.to_owned()).map_err(|_| ReaderError::Conflict)?;
    let root = crate::SourceRoot::restore(directory.canonicalize()?, selected.identity.clone())
        .map_err(|_| ReaderError::Conflict)?;
    selected.verify().map_err(|_| ReaderError::Conflict)?;
    let before = optional(&root, name)?;
    if before.is_none() && !create {
        return Err(ReaderError::Unavailable);
    }
    let connection = open(&root.path.join(name))?;
    let object = named(&root, name)?;
    if before.is_some_and(|before| before != object) || actual(&connection, c"main")? != object {
        return Err(ReaderError::Conflict);
    }
    let owner = LibraryOwner {
        lifetime: Arc::new(()),
        retained: Arc::new(Retained { root, object, name }),
    };
    verify(&connection, Some(&owner), "main", c"main")?;
    Ok((connection, Some(owner)))
}
pub(super) fn verify(
    connection: &Connection,
    owner: Option<&LibraryOwner>,
    sql_name: &'static str,
    c_name: &'static CStr,
) -> Result<()> {
    let retained = &owner.ok_or(ReaderError::Unavailable)?.retained;
    verify_live(retained)?;
    let path: String = connection
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name=?1",
            [sql_name],
            |row| row.get(0),
        )
        .map_err(|_| ReaderError::Unavailable)?;
    if Path::new(&path)
        .canonicalize()
        .map_err(|_| ReaderError::Conflict)?
        != retained.root.path.join(retained.name)
        || actual(connection, c_name)? != retained.object
    {
        return Err(ReaderError::Conflict);
    }
    verify_live(retained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReaderStore;

    #[test]
    fn windows_certificates_bind_actual_attached_handles_and_expire_with_owner() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("é library % # space");
        std::fs::create_dir(&directory).unwrap();
        let store = ReaderStore::open(&directory).unwrap();
        let certificate = store.library_identity().unwrap();
        certificate.verify_at(&directory).unwrap();
        store
            .store_identity()
            .unwrap()
            .verify_at(&directory)
            .unwrap();
        let other = temporary.path().join("other");
        std::fs::create_dir(&other).unwrap();
        assert!(matches!(
            certificate.verify_at(&other),
            Err(ReaderError::Conflict)
        ));
        let readonly = ReaderStore::open_readonly(&directory).unwrap();
        readonly
            .store_identity()
            .unwrap()
            .verify_at(&directory)
            .unwrap();
        drop(readonly);
        store
            .index
            .execute_batch(
                "DETACH DATABASE reader_authority; ATTACH DATABASE ':memory:' AS reader_authority",
            )
            .unwrap();
        assert!(store.store_identity().is_err());
        drop(store);
        assert!(matches!(
            certificate.verify(),
            Err(ReaderError::Unavailable)
        ));
        let reopened = ReaderStore::open(&directory).unwrap();
        reopened
            .store_identity()
            .unwrap()
            .verify_at(&directory)
            .unwrap();
        drop(reopened);
        // Hardlinked existing databases are rejected before an open callback can
        // initialize or migrate either name, even when their content is valid.
        std::fs::hard_link(
            directory.join("library.sqlite"),
            directory.join("alias.sqlite"),
        )
        .unwrap();
        assert!(matches!(
            open(&directory, "library.sqlite", true, |_| {
                panic!("hardlink must be rejected before opening SQLite")
            }),
            Err(ReaderError::Conflict)
        ));
    }
}
