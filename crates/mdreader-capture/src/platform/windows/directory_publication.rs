//! Consuming sibling-directory publication. Only this completed stage's leaf
//! pin transfers; ancestor pins and a non-write-sharing identity observer stay
//! held throughout. The caller owns durable publication/recovery evidence.
use super::*;

#[allow(unsafe_code)]
pub(super) fn create(path: &Path) -> Result<()> {
    let parent =
        authority::Chain::open(path.parent().ok_or(CaptureError::UnsafePath)?, false, true)?;
    validate_leaf(path)?;
    let wide = authority::wide(path)?;
    let descriptor = security::Descriptor::private()?;
    let attributes = descriptor.attributes();
    // SAFETY: live terminated path and explicit protected DACL. Existing names
    // are errors, including existing private directories; no permission repair.
    if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    parent.verify()
}

#[allow(unsafe_code)]
pub(super) fn publish(mut directory: PrivateDirectory, name: &Path) -> Result<PrivateDirectory> {
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, RtlNtStatusToDosErrorNoTeb,
        FILE_RENAME_INFORMATION,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
    directory.verify()?;
    if name.components().count() != 1
        || !matches!(name.components().next(), Some(Component::Normal(_)))
    {
        return Err(CaptureError::UnsafePath);
    }
    let (source, _, _) = directory
        .chain
        .handles
        .last()
        .ok_or(CaptureError::UnsafePath)?;
    let source = source.clone();
    let parent = source.parent().ok_or(CaptureError::UnsafePath)?;
    let destination = parent.join(name);
    validate_leaf(&destination)?;
    let ancestors = authority::Chain::open(parent, false, true)?;
    let expected = directory.identity()?;
    // Retain the original object itself so its full file ID cannot be recycled
    // during transfer. Denied write sharing still prevents reparse mutation.
    let observer = open_directory(&source, false)?;
    if super::private::object_identity(&observer)? != expected {
        return Err(CaptureError::UnsafePath);
    }
    // All callers must have closed children. Any retained child/root pin makes
    // the following DELETE open fail safely; no sharing override is requested.
    drop(
        directory
            .chain
            .handles
            .pop()
            .ok_or(CaptureError::UnsafePath)?,
    );
    let owned = open_directory(&source, true)?;
    if super::private::object_identity(&owned)? != expected {
        return Err(CaptureError::UnsafePath);
    }
    let wide = authority::wide(name)?;
    let bytes = size_of::<FILE_RENAME_INFORMATION>() + wide.len() * 2;
    let mut words = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let info = words.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: aligned bounded allocation and initialized Windows ABI payload.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = ((wide.len() - 1) * 2) as u32;
        ptr::copy_nonoverlapping(
            wide.as_ptr(),
            ptr::addr_of_mut!((*info).FileName).cast(),
            wide.len(),
        );
    }
    ancestors.verify()?;
    let mut result = IO_STATUS_BLOCK::default();
    // SAFETY: synchronous live DELETE handle and bounded live ABI buffers.
    let status = unsafe {
        NtSetInformationFile(
            owned.as_raw_handle().cast(),
            &mut result,
            info.cast(),
            bytes as u32,
            FileRenameInformation,
        )
    };
    if status != 0 {
        // SAFETY: pure returned NTSTATUS conversion.
        let code = unsafe { RtlNtStatusToDosErrorNoTeb(status) };
        return Err(std::io::Error::from_raw_os_error(code as i32).into());
    }
    drop(owned);
    let published = PrivateDirectory::open(&destination)?;
    if published.identity()? != expected {
        return Err(CaptureError::UnsafePath);
    }
    ancestors.verify()?;
    published.verify()?;
    Ok(published)
}

#[allow(unsafe_code)]
fn open_directory(path: &Path, delete: bool) -> Result<File> {
    let wide = authority::wide(path)?;
    // SAFETY: terminated path and fixed flags. Observer shares deletion only;
    // the mutation handle denies it. Neither permits write sharing.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE | if delete { DELETE } else { 0 },
            FILE_SHARE_READ | if delete { 0 } else { FILE_SHARE_DELETE },
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: successful native open transfers ownership of one handle.
    let file = unsafe { File::from_raw_handle(handle.cast()) };
    authority::inspect(&file, true, true)?;
    authority::ntfs(&file)?;
    Ok(file)
}
