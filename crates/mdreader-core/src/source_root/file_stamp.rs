//! Open-handle metadata for bounded source-file snapshots. No pathname is opened
//! here; the caller must retain its original no-follow SourceRoot authority.
use std::{fs::File, io};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceFileStamp(Vec<u64>);

/// Open an absolute path only if it is a regular file, without following a
/// final symlink and without blocking on a FIFO or device (checked before
/// and after opening, so a swap in between is refused rather than waited on).
pub fn open_regular(path: &std::path::Path) -> io::Result<File> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    #[cfg(unix)]
    let file = {
        use rustix::fs::{open, Mode, OFlags};
        File::from(open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )?)
    };
    #[cfg(not(unix))]
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    Ok(file)
}

/// Refuse hardlinked/nonregular files, then bind identity, size and write/change
/// metadata. Compare both the retained handle and a fresh retained-root open
/// after reading. This is a stable-observation fence, not a writer lock.
pub fn single_link_stamp(file: &File) -> io::Result<SourceFileStamp> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("source file is not regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(io::Error::other("source file has multiple links"));
        }
        Ok(SourceFileStamp(vec![
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
        ]))
    }
    #[cfg(windows)]
    {
        windows(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(io::Error::other("source file observation unsupported"))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows(file: &File) -> io::Result<SourceFileStamp> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
    };
    let mut identity = BY_HANDLE_FILE_INFORMATION::default();
    let mut basic = FILE_BASIC_INFO::default();
    // SAFETY: the borrowed File remains live, and both output pointers name
    // correctly sized initialized Windows ABI structures for these calls.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut identity) } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle().cast(),
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if identity.nNumberOfLinks != 1 || identity.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(io::Error::other("source file links unavailable"));
    }
    Ok(SourceFileStamp(vec![
        u64::from(identity.dwVolumeSerialNumber),
        u64::from(identity.nFileIndexHigh),
        u64::from(identity.nFileIndexLow),
        u64::from(identity.nFileSizeHigh),
        u64::from(identity.nFileSizeLow),
        basic.LastWriteTime as u64,
        basic.ChangeTime as u64,
        basic.CreationTime as u64,
    ]))
}
