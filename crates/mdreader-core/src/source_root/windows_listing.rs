//! Bounded directory observations through the exact retained Windows object.
//! A fresh listing cursor must not resolve the Win32 spelling "." as an NT
//! child name or reopen a display path. Root and nested listing handles are
//! independent cursors bound to the same retained objects.
use super::*;

// Only the actual core unit-test harness emits these diagnostics. Shipping and
// dependency builds retain identical errors without exposing native context.
fn refused(phase: &'static str, raw_os_error: Option<i32>) {
    #[cfg(test)]
    eprintln!("Windows retained listing refused at {phase}; raw_os_error={raw_os_error:?}");
    #[cfg(not(test))]
    let _ = (phase, raw_os_error);
}
fn at<T>(phase: &'static str, result: Result<T, SourceRootError>) -> Result<T, SourceRootError> {
    result.inspect_err(|_| refused(phase, None))
}

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
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        NtCreateFile, RtlNtStatusToDosErrorNoTeb, FILE_DIRECTORY_FILE, FILE_OPEN,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::{HANDLE, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let path = root.path.join(relative);
    let identity = at("pinned_identity_before", windows_identity(&path, pinned))?;
    // Reopen the exact pinned object with an empty NT relative name, as in
    // Microsoft's WSL filesystem::ReopenFile (see the loader diagnostics doc).
    // This applies to both CreateFile roots and NtCreateFile nested pins. It
    // needs neither a display-path lookup nor a permanent list/read-data right
    // on the traversal pin, which would obstruct native sibling rename.
    // The empty string owns a real terminator: Length excludes it, while
    // MaximumLength describes its full live buffer, like RtlInitUnicodeString.
    let mut empty = [0u16];
    let unicode = UNICODE_STRING {
        Length: 0,
        MaximumLength: std::mem::size_of::<u16>() as u16,
        Buffer: empty.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: pinned.as_raw_handle().cast(),
        ObjectName: std::ptr::addr_of!(unicode),
        Attributes: 0,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut status_block = IO_STATUS_BLOCK::default();
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: pinned remains owned for the whole synchronous call; every ABI
    // pointer refers to a live correctly sized object. FILE_OPEN cannot create
    // or replace anything. The resulting independent listing handle denies
    // write/delete sharing and requests no-follow directory access only.
    let status = unsafe {
        NtCreateFile(
            std::ptr::addr_of_mut!(handle),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            std::ptr::addr_of!(attributes),
            std::ptr::addr_of_mut!(status_block),
            std::ptr::null(),
            0,
            FILE_SHARE_READ,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        // SAFETY: total conversion of the captured NTSTATUS; no GetLastError
        // sampling after an NT call and no dependence on thread error state.
        let raw_os_error = unsafe { RtlNtStatusToDosErrorNoTeb(status) } as i32;
        refused(
            if relative.as_os_str().is_empty() {
                "root_reopen_listing"
            } else {
                "nested_reopen_listing"
            },
            Some(raw_os_error),
        );
        return Err(SourceRootError::CannotInspect {
            path,
            reason: std::io::Error::from_raw_os_error(raw_os_error).to_string(),
        });
    }
    if handle.is_null() {
        refused("listing_null_handle", None);
        return Err(SourceRootError::IdentityUnavailable {
            path,
            reason: "NtCreateFile returned a null directory handle".to_string(),
        });
    }
    // SAFETY: successful NtCreateFile transferred a new handle, adopted once.
    let directory = unsafe { fs::File::from_raw_handle(handle.cast()) };
    let metadata = directory.metadata().map_err(|error| {
        refused("listing_metadata", error.raw_os_error());
        SourceRootError::CannotInspect {
            path: path.clone(),
            reason: error.to_string(),
        }
    })?;
    if is_windows_reparse_point(metadata.file_attributes()) || !metadata.is_dir() {
        refused("listing_object_type", None);
        return Err(SourceRootError::SymlinkOrReparse { path });
    }
    if at("listing_identity", windows_identity(&path, &directory))? != identity
        || at("pinned_identity_after", windows_identity(&path, pinned))? != identity
    {
        refused("listing_identity_changed", None);
        return Err(SourceRootError::ChangedDuringValidation { path });
    }
    at("root_after_listing_open", root.verify())?;
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

    at("root_before_listing", root.verify())?;
    let pinned = at(
        "pinned_directory",
        open_windows_relative(root, relative, true),
    )?;
    let directory = open_listing(root, relative, &pinned)?;
    const BUFFER_BYTES: usize = 64 * 1024;
    let words = BUFFER_BYTES.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0usize; words];
    let mut restart = true;
    let mut entries = Vec::new();

    loop {
        let query_phase = if restart {
            "query_directory_restart"
        } else {
            "query_directory_continue"
        };
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
            refused(query_phase, error.raw_os_error());
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
                refused("directory_record_header", None);
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
                refused("directory_record_filename", None);
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
                    refused("directory_entry_bound", None);
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
                refused("directory_record_offset", None);
                return Err(SourceRootError::CannotInspect {
                    path: root.path.join(relative),
                    reason: "Windows returned an invalid directory record offset".to_string(),
                });
            }
            offset += next;
        }
    }
    at("root_after_enumeration", root.verify())?;
    Ok(entries)
}
