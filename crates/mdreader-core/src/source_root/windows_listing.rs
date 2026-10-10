//! Bounded directory observations through the exact retained Windows object.
//! A fresh listing cursor must not resolve the Win32 spelling "." as an NT
//! child name or reopen a display path. Root and nested listing handles are
//! independent cursors bound to the same retained objects.
use super::*;

#[allow(unsafe_code)]
fn open_listing(
    root: &SourceRoot,
    relative: &Path,
    pinned: &fs::File,
) -> Result<fs::File, SourceRootError> {
    use std::os::windows::{
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle},
    };
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        ReOpenFile, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, SYNCHRONIZE,
    };
    let path = root.path.join(relative);
    let identity = windows_identity(&path, pinned)?;
    let directory = if relative.as_os_str().is_empty() {
        // The SourceRoot object was opened by OpenOptions/CreateFile. ReOpenFile
        // is documented for those objects, including directories; duplicated
        // root handles retain that same object. Do not apply it to NT-created
        // nested handles or submit a Win32 "." component to NtCreateFile.
        // SAFETY: pinned owns a live CreateFile directory object. The API gives
        // a new independent cursor with read-only sharing and no reparse follow.
        let handle = unsafe {
            ReOpenFile(
                pinned.as_raw_handle().cast(),
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(SourceRootError::CannotInspect {
                path,
                reason: std::io::Error::last_os_error().to_string(),
            });
        }
        // SAFETY: ReOpenFile transferred a new owned handle, adopted once.
        unsafe { fs::File::from_raw_handle(handle.cast()) }
    } else {
        // Resolve the actual final component against its retained parent. Both
        // the original nested object and parent remain pinned with no write or
        // delete sharing while the independent read-only listing cursor opens.
        let (parent, name) = open_windows_parent(root, relative)?;
        open_windows_child(
            root,
            &parent,
            &name,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            relative,
        )?
    };
    let metadata = directory
        .metadata()
        .map_err(|error| SourceRootError::CannotInspect {
            path: path.clone(),
            reason: error.to_string(),
        })?;
    if is_windows_reparse_point(metadata.file_attributes()) || !metadata.is_dir() {
        return Err(SourceRootError::SymlinkOrReparse { path });
    }
    if windows_identity(&path, &directory)? != identity
        || windows_identity(&path, pinned)? != identity
    {
        return Err(SourceRootError::ChangedDuringValidation { path });
    }
    root.verify()?;
    Ok(directory)
}

#[cfg(test)]
mod tests;

#[allow(unsafe_code)]
pub(super) fn read(
    root: &SourceRoot,
    relative: &Path,
    maximum: usize,
) -> Result<Vec<SourceEntry>, SourceRootError> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ID_BOTH_DIR_INFO,
    };

    root.verify()?;
    let pinned = open_windows_relative(root, relative, true)?;
    let directory = open_listing(root, relative, &pinned)?;
    const BUFFER_BYTES: usize = 64 * 1024;
    let words = BUFFER_BYTES.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0usize; words];
    let mut restart = true;
    let mut entries = Vec::new();

    loop {
        let class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        restart = false;
        // SAFETY: the directory handle is live and `storage` is a writable
        // BUFFER_BYTES-sized region for the requested directory information.
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle().cast(),
                class,
                storage.as_mut_ptr().cast(),
                BUFFER_BYTES as u32,
            )
        };
        if succeeded == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(SourceRootError::CannotInspect {
                path: root.path.join(relative),
                reason: error.to_string(),
            });
        }

        let mut offset = 0usize;
        loop {
            let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
            if offset
                .checked_add(header)
                .is_none_or(|end| end > BUFFER_BYTES)
            {
                return Err(SourceRootError::CannotInspect {
                    path: root.path.join(relative),
                    reason: "Windows returned malformed directory information".to_string(),
                });
            }
            // SAFETY: the bounds check above establishes a complete fixed
            // FILE_ID_BOTH_DIR_INFO header at this aligned record offset.
            let info = unsafe {
                &*storage
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
            };
            let name_bytes = info.FileNameLength as usize;
            if !name_bytes.is_multiple_of(std::mem::size_of::<u16>())
                || offset
                    .checked_add(header)
                    .and_then(|start| start.checked_add(name_bytes))
                    .is_none_or(|end| end > BUFFER_BYTES)
            {
                return Err(SourceRootError::CannotInspect {
                    path: root.path.join(relative),
                    reason: "Windows returned malformed directory filename data".to_string(),
                });
            }
            // SAFETY: FileNameLength was validated against the backing buffer
            // and is an even number of bytes.
            let name = unsafe {
                let wide = std::slice::from_raw_parts(
                    std::ptr::addr_of!(info.FileName).cast::<u16>(),
                    name_bytes / std::mem::size_of::<u16>(),
                );
                OsString::from_wide(wide)
            };
            if name != "." && name != ".." {
                if entries.len() >= maximum {
                    return Err(SourceRootError::CannotInspect {
                        path: root.path.join(relative),
                        reason: "directory entry limit exceeded".into(),
                    });
                }
                let attributes = info.FileAttributes;
                entries.push(SourceEntry {
                    name,
                    kind: if is_windows_reparse_point(attributes) {
                        SourceEntryKind::SymlinkOrReparse
                    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                        SourceEntryKind::Directory
                    } else {
                        SourceEntryKind::File
                    },
                    size: u64::try_from(info.EndOfFile).unwrap_or(u64::MAX),
                });
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            let next = info.NextEntryOffset as usize;
            if next < header
                || !next.is_multiple_of(std::mem::align_of::<FILE_ID_BOTH_DIR_INFO>())
                || offset
                    .checked_add(next)
                    .is_none_or(|end| end >= BUFFER_BYTES)
            {
                return Err(SourceRootError::CannotInspect {
                    path: root.path.join(relative),
                    reason: "Windows returned an invalid directory record offset".to_string(),
                });
            }
            offset += next;
        }
    }
    root.verify()?;
    Ok(entries)
}
