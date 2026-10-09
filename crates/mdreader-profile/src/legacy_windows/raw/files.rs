use super::*;
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_HANDLE_EOF, INVALID_HANDLE_VALUE},
    Storage::FileSystem::*,
};

pub(super) fn retain(root: &SourceRoot, relative: &Path) -> Result<File> {
    // Pin every parent first; no reparse point can redirect the absolute open.
    let parent = mdreader_capture::SelectedDirectory::open(
        &root.path.join(relative.parent().ok_or(Error::Invalid)?),
    )
    .map_err(|_| Error::Conflict)?;
    let path = root.path.join(relative);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(|_| Error::NotQuiescent)?;
    let named = root
        .open_optional_file_relative(relative)
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    if observe_object(&file, false, false).map_err(|_| Error::Conflict)?
        != observe_object(&named, false, false).map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    single_stream(&path)?;
    parent.verify().map_err(|_| Error::Conflict)?;
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(file)
}
#[allow(unsafe_code)]
pub(super) fn single_stream(path: &Path) -> Result<()> {
    streams(path, false)
}
pub(super) fn directory_streams(path: &Path) -> Result<()> {
    streams(path, true)
}
#[allow(unsafe_code)]
fn streams(path: &Path, directory: bool) -> Result<()> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.len() > 32767 || wide.contains(&0) {
        return Err(Error::Bounds);
    }
    wide.push(0);
    let mut data = WIN32_FIND_STREAM_DATA::default();
    // SAFETY: bounded terminated local path and correctly initialized API output.
    let find = unsafe {
        FindFirstStreamW(
            wide.as_ptr(),
            FindStreamInfoStandard,
            (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
            0,
        )
    };
    if find == INVALID_HANDLE_VALUE {
        // Directories normally have no data stream; attached alternate streams
        // must still be refused rather than omitted from a raw preservation claim.
        // SAFETY: query the immediate result of FindFirstStreamW.
        if directory && unsafe { GetLastError() } == ERROR_HANDLE_EOF {
            return Ok(());
        }
        return Err(Error::Unsupported);
    }
    let result = (|| {
        let end = data
            .cStreamName
            .iter()
            .position(|c| *c == 0)
            .ok_or(Error::Bounds)?;
        if directory || data.cStreamName[..end] != "::$DATA".encode_utf16().collect::<Vec<_>>() {
            return Err(Error::Unsupported);
        }
        // SAFETY: exact search handle and reusable correctly typed API output.
        if unsafe { FindNextStreamW(find, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) } != 0
        {
            return Err(Error::Unsupported);
        }
        // SAFETY: read status immediately after the final enumeration call.
        if unsafe { GetLastError() } != ERROR_HANDLE_EOF {
            return Err(Error::Conflict);
        }
        Ok(())
    })();
    // SAFETY: close exactly the search handle returned by FindFirstStreamW.
    if unsafe { FindClose(find) } == 0 {
        return Err(Error::Storage);
    }
    result
}
