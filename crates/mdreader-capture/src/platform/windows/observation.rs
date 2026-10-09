//! Native metadata observations shared by profile preservation. No path opens.
use super::*;

/// Volume, low file-ID half, bytes, high file-ID half, write/change/create times.
/// The high half uses signed storage solely to preserve its exact bit pattern.
pub type WindowsObjectStamp = (u64, u64, u64, i64, i64, i64, i64);

/// Compatible metadata-only observation of a live SQLite name. The enclosing
/// authority still denies directory replacement; this observer adds no writer.
#[allow(unsafe_code)]
pub fn observe_named_object(path: &Path) -> Result<WindowsObjectStamp> {
    validate_leaf(path)?;
    let parent =
        authority::Chain::open(path.parent().ok_or(CaptureError::UnsafePath)?, false, false)?;
    let wide = authority::wide(path)?;
    // SAFETY: terminated path and fixed flags. No data access or mutation.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: the successful open transfers ownership of one handle.
    let file = unsafe { File::from_raw_handle(handle.cast()) };
    let result = observe_object(&file, false, false)?;
    parent.verify()?;
    Ok(result)
}

/// Observe the actual retained NTFS object, rejecting reparse points/hardlinks.
/// Access times are excluded because reading originals can legitimately change them.
#[allow(unsafe_code)]
pub fn observe_object(file: &File, directory: bool, private: bool) -> Result<WindowsObjectStamp> {
    authority::inspect(file, directory, private)?;
    authority::ntfs(file)?;
    let id = authority::identity(file)?;
    let mut basic = FILE_BASIC_INFO::default();
    // SAFETY: borrowed live handle and correctly sized initialized ABI output.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle().cast(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut low = [0u8; 8];
    let mut high = [0u8; 8];
    low.copy_from_slice(&id.1[..8]);
    high.copy_from_slice(&id.1[8..]);
    Ok((
        id.0,
        u64::from_le_bytes(low),
        file.metadata()?.len(),
        i64::from_le_bytes(high),
        basic.LastWriteTime,
        basic.ChangeTime,
        basic.CreationTime,
    ))
}
