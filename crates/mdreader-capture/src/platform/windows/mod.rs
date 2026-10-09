//! Windows private filesystem boundary. Capture durability belongs to the
//! canonical SQLite journal; Markdown publication is a rebuildable projection.
//! File flushing/rename alone never claims directory-fsync equivalence.
use crate::*;
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle},
    ptr,
};
use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::*};

mod authority;
#[cfg(test)]
pub(crate) mod diagnostics;
mod private;
mod security;
// The actual production operation/result is unchanged. Only unit-test builds
// report fixed stages and numeric OS errors; never paths or capture bodies.
macro_rules! trace_step {
    ($stage:literal, $operation:expr) => {{
        let result: crate::Result<_> = ($operation).map_err(crate::CaptureError::from);
        #[cfg(test)]
        $crate::platform::windows::diagnostics::record($stage, &result);
        result
    }};
}
pub(crate) use authority::DirectoryAuthority;
pub use private::{
    same_open_file, selected_file_identity, PrivateDirectory, SelectedDirectory,
    WindowsFileIdentity,
};
pub(crate) use trace_step;

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    authority::Chain::open(path, true, true)?.verify()
}

// Keep the entire parent chain pinned while opening the leaf. Every component
// is opened without following reparse points or sharing delete/write access.
#[allow(unsafe_code)]
fn open_file(path: &Path, creation: u32, writable: bool) -> Result<File> {
    open_file_with_lock(
        path,
        creation,
        writable,
        path.file_name().is_some_and(|name| name == "queue.lock"),
    )
}
#[allow(unsafe_code)]
pub(super) fn open_file_with_lock(
    path: &Path,
    creation: u32,
    writable: bool,
    lock_file: bool,
) -> Result<File> {
    open_file_policy(path, creation, writable, lock_file, true)
}
#[allow(unsafe_code)]
fn open_file_policy(
    path: &Path,
    creation: u32,
    writable: bool,
    lock_file: bool,
    private_parent: bool,
) -> Result<File> {
    validate_leaf(path)?;
    let parent = authority::Chain::open(
        path.parent().ok_or(CaptureError::UnsafePath)?,
        false,
        private_parent,
    )?;
    let wide = authority::wide(path)?;
    let descriptor = security::Descriptor::private()?;
    let attributes = descriptor.attributes();
    let access = if writable {
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | if lock_file { 0 } else { DELETE }
    } else {
        FILE_GENERIC_READ
    };
    // Lock files must allow another opener to reach the nonblocking byte lock.
    let share = if lock_file {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    } else {
        FILE_SHARE_READ
    };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT | if writable { FILE_FLAG_WRITE_THROUGH } else { 0 };
    // SAFETY: terminated live path, live explicit descriptor and valid flags.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            share,
            &attributes,
            creation,
            flags,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful CreateFileW transfers one owned handle.
    let file = unsafe { File::from_raw_handle(handle.cast()) };
    authority::inspect(&file, false, true)?;
    authority::ntfs(&file)?;
    parent.verify()?;
    Ok(file)
}

pub(crate) fn private_open(path: &Path, create_new: bool) -> Result<File> {
    open_file(
        path,
        if create_new { CREATE_NEW } else { OPEN_ALWAYS },
        true,
    )
}
pub(crate) fn ensure_regular_or_missing(path: &Path) -> Result<()> {
    match open_file(path, OPEN_EXISTING, false) {
        Ok(_) => Ok(()),
        Err(CaptureError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
pub(crate) fn regular_exists(path: &Path) -> Result<bool> {
    match open_file(path, OPEN_EXISTING, false) {
        Ok(_) => Ok(true),
        Err(CaptureError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
pub(crate) fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    let file = open_file(path, OPEN_EXISTING, false)?;
    if file.metadata()?.len() > max as u64 {
        return Err(CaptureError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(CaptureError::TooLarge);
    }
    Ok(bytes)
}
#[cfg(test)]
pub(crate) fn sync_directory(_: &Path) -> Result<()> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn lock_would_block(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock || error.raw_os_error() == Some(33)
}

/// Rename the opened temporary object into an opened destination directory.
/// File flushes are real; namespace durability still requires the explicit gate.
#[allow(unsafe_code)]
pub(crate) fn replace(file: &File, _: &Path, destination: &Path) -> Result<()> {
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, RtlNtStatusToDosErrorNoTeb,
        FILE_RENAME_INFORMATION,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
    validate_leaf(destination)?;
    trace_step!(
        "rename.inspect_source",
        authority::inspect(file, false, true)
    )?;
    let parent = trace_step!(
        "rename.pin_parent",
        authority::Chain::open(
            destination.parent().ok_or(CaptureError::UnsafePath)?,
            false,
            true,
        )
    )?;
    let directory = &parent.handles.last().ok_or(CaptureError::UnsafePath)?.1;
    if authority::identity(file)?.0 != authority::identity(directory)?.0 {
        return Err(CaptureError::UnsafePath);
    }
    trace_step!(
        "rename.destination_probe",
        ensure_regular_or_missing(destination)
    )?;
    let name = destination.file_name().ok_or(CaptureError::UnsafePath)?;
    let name = authority::wide(Path::new(name))?;
    let bytes = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName) + name.len() * 2;
    let mut words = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let info = words.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: usize-aligned allocation holds FILE_RENAME_INFORMATION and the bounded
    // variable UTF16 filename; the borrowed directory remains pinned throughout.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).RootDirectory = directory.as_raw_handle().cast();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
    }
    trace_step!("rename.first_flush", file.sync_all())?;
    let mut result = IO_STATUS_BLOCK::default();
    // NtSetInformationFile consumes the handle-relative native rename payload
    // directly. Preserve the pinned parent rather than replacing it with an
    // absolute pathname to accommodate Win32's rename wrapper.
    // SAFETY: synchronous live DELETE-capable file, live output status block,
    // pinned parent HANDLE and initialized correctly sized native payload.
    let status = unsafe {
        NtSetInformationFile(
            file.as_raw_handle().cast(),
            &mut result,
            info.cast(),
            bytes as u32,
            FileRenameInformation,
        )
    };
    #[cfg(test)]
    diagnostics::native_status(status);
    if status != 0 {
        // SAFETY: pure conversion of the returned NTSTATUS to a Win32 code.
        let code = unsafe { RtlNtStatusToDosErrorNoTeb(status) };
        return Err(std::io::Error::from_raw_os_error(code as i32).into());
    }
    trace_step!("rename.second_flush", file.sync_all())?;
    trace_step!("rename.parent_verify", parent.verify())
}

#[cfg(test)]
mod tests;

fn validate_leaf(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(CaptureError::UnsafePath)?;
    let stem = name
        .split('.')
        .next()
        .ok_or(CaptureError::UnsafePath)?
        .to_ascii_uppercase();
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.contains([':', '\\', '/'])
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(CaptureError::UnsafePath);
    }
    Ok(())
}

/// Pin against replacement while SQLite opens its own read/write descriptor.
pub(crate) fn pin_database(path: &Path) -> Result<File> {
    open_file_with_lock(path, OPEN_ALWAYS, true, true)
}

pub(crate) fn pin_existing_database(path: &Path) -> Result<Option<File>> {
    match open_file_with_lock(path, OPEN_EXISTING, true, true) {
        Ok(file) => Ok(Some(file)),
        Err(CaptureError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
