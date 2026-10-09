//! Bind an actual borrowed SQLite main handle without opening another DB FD.
use crate::{
    platform::{Root, Stamp},
    Error, Result,
};
use rusqlite::Connection;
use std::path::Path;

fn refused(phase: &'static str) -> Error {
    #[cfg(test)]
    eprintln!("profile live database refused at {phase}");
    #[cfg(not(test))]
    let _ = phase;
    Error::Conflict
}

pub(crate) fn check(
    connection: &Connection,
    root: &Root,
    path: &Path,
    expected: &Stamp,
) -> Result<()> {
    crate::model::relative(path)?;
    if !source_name_matches(
        Path::new(connection.path().ok_or(Error::Conflict)?),
        &root.path.join(path),
    ) {
        return Err(refused("vfs_name"));
    }
    if root.named_stamp(path)? != *expected {
        return Err(refused("named_before"));
    }
    check_live_handle(connection, expected)?;
    if root.named_stamp(path)? != *expected {
        return Err(refused("named_after"));
    }
    check_live_handle(connection, expected)?;
    root.verify()
}

#[cfg(not(windows))]
fn source_name_matches(actual: &Path, expected: &Path) -> bool {
    actual == expected
}

#[cfg(windows)]
fn source_name_matches(actual: &Path, expected: &Path) -> bool {
    use mdreader_core::SourceRoot;
    use std::path::{Component, Prefix};
    let mut actual_parts = actual.components();
    let mut expected_parts = expected.components();
    let drive = |component| match component {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => Some(drive),
            _ => None,
        },
        _ => None,
    };
    if !matches!((drive(actual_parts.next()), drive(expected_parts.next())),
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(&b))
    {
        return false;
    }
    if actual_parts.eq(expected_parts) {
        return true;
    }
    // SQLite may retain a DOS short spelling of a selected absolute directory.
    // Admit that original spelling through native no-reparse ancestor pins
    // before resolving it. Only the exact retained parent's full NTFS identity
    // and exact member name are accepted. No database descriptor is opened.
    if !actual.is_absolute() || actual.file_name() != expected.file_name() {
        return false;
    }
    let Some((actual_parent, expected_parent)) = actual.parent().zip(expected.parent()) else {
        return false;
    };
    let Ok(selected) = SourceRoot::capture(actual_parent.to_owned()) else {
        return false;
    };
    let Ok(bound) = SourceRoot::capture(expected_parent.to_owned()) else {
        return false;
    };
    let Ok(canonical) = actual_parent.canonicalize() else {
        return false;
    };
    selected.identity == bound.identity
        && canonical == expected_parent
        && selected.verify().is_ok()
        && bound.verify().is_ok()
}

#[cfg(all(test, windows))]
#[test]
fn windows_vfs_drive_spelling_keeps_exact_member_and_absolute_root() {
    assert!(source_name_matches(
        Path::new(r"D:\reader\library.sqlite"),
        Path::new(r"\\?\D:\reader\library.sqlite")
    ));
    assert!(!source_name_matches(
        Path::new(r"D:\other\library.sqlite"),
        Path::new(r"\\?\D:\reader\library.sqlite")
    ));
    assert!(!source_name_matches(
        Path::new(r"D:reader\library.sqlite"),
        Path::new(r"\\?\D:\reader\library.sqlite")
    ));
    assert!(!source_name_matches(
        Path::new(r"E:\reader\library.sqlite"),
        Path::new(r"\\?\D:\reader\library.sqlite")
    ));
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn check_live_handle(connection: &Connection, _: &Stamp) -> Result<()> {
    let mut moved: std::ffi::c_int = -1;
    // SAFETY: this shared borrow retains the actual live SQLite connection.
    // The static NUL-terminated main name and initialized int out-pointer match
    // SQLITE_FCNTL_HAS_MOVED's ABI. No pointer/ownership escapes; no SQL writes.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            std::ptr::addr_of_mut!(moved).cast(),
        )
    };
    if result != rusqlite::ffi::SQLITE_OK {
        return Err(Error::Unsupported);
    }
    if moved != 0 {
        return Err(Error::Conflict);
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn check_live_handle(connection: &Connection, expected: &Stamp) -> Result<()> {
    use std::{
        fs::File,
        mem::ManuallyDrop,
        os::windows::io::{FromRawHandle, RawHandle},
    };
    let mut handle: RawHandle = std::ptr::null_mut();
    // SAFETY: this shared borrow retains SQLite's actual main connection. The
    // file-control output is a borrowed HANDLE, never closed or allowed to escape.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
            std::ptr::addr_of_mut!(handle).cast(),
        )
    };
    if result != rusqlite::ffi::SQLITE_OK || handle.is_null() || handle as isize == -1 {
        return Err(Error::Unsupported);
    }
    // SAFETY: ManuallyDrop ensures this view cannot close SQLite's borrowed handle.
    let borrowed = ManuallyDrop::new(unsafe { File::from_raw_handle(handle) });
    if crate::platform::stamp(&borrowed)? != *expected {
        return Err(refused("native_handle"));
    }
    Ok(())
}
