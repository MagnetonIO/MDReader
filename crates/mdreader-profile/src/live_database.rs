//! Bind an actual borrowed SQLite main handle without opening another DB FD.
use crate::{
    platform::{Root, Stamp},
    Error, Result,
};
use rusqlite::Connection;
use std::path::Path;

pub(crate) fn check(
    connection: &Connection,
    root: &Root,
    path: &Path,
    expected: &Stamp,
) -> Result<()> {
    crate::model::relative(path)?;
    if Path::new(connection.path().ok_or(Error::Conflict)?) != root.path.join(path)
        || root.named_stamp(path)? != *expected
    {
        return Err(Error::Conflict);
    }
    check_live_handle(connection, expected)?;
    if root.named_stamp(path)? != *expected {
        return Err(Error::Conflict);
    }
    check_live_handle(connection, expected)?;
    root.verify()
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
        return Err(Error::Conflict);
    }
    Ok(())
}
