use super::*;
use std::{
    io::Read,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
};
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
pub(super) fn verify_archive(
    path: &Path,
    identity: &SnapshotIdentity,
    limits: &Limits,
) -> Result<()> {
    limits.validate()?;
    if !crate::model::valid_hash(&identity.manifest_sha256) {
        return Err(Error::Invalid);
    }
    let root = crate::platform::Root::private(path)?;
    let mut file = root.open(Path::new("manifest.json"))?;
    crate::platform::private_file(&file)?;
    let initial = crate::platform::stamp(&file)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(limits.max_manifest_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > limits.max_manifest_bytes
        || crate::model::hash(&bytes) != identity.manifest_sha256
        || crate::platform::stamp(&file)? != initial
    {
        return Err(Error::Integrity);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
    if manifest.version != 1
        || manifest.format != "mdreader-legacy-raw-windows"
        || manifest.id != identity.id
        || manifest.files.len() + manifest.directories.len() > limits.max_entries
    {
        return Err(Error::Integrity);
    }
    manifest.original.validate()?;
    use std::os::windows::ffi::OsStringExt;
    let mut directories = std::collections::HashSet::new();
    for directory in &manifest.directories {
        if directory.relative.len() > 16384 || directory.relative.contains(&0) {
            return Err(Error::Integrity);
        }
        let path = PathBuf::from(std::ffi::OsString::from_wide(&directory.relative));
        if !path.as_os_str().is_empty() {
            crate::model::relative(&path)?;
        }
        if !directories.insert(path) {
            return Err(Error::Integrity);
        }
    }
    if !directories.contains(Path::new(""))
        || directories.iter().any(|path| {
            !path.as_os_str().is_empty()
                && path
                    .parent()
                    .is_none_or(|parent| !directories.contains(parent))
        })
    {
        return Err(Error::Integrity);
    }
    let mut total = 0u64;
    let mut relative = std::collections::HashSet::new();
    for (index, entry) in manifest.files.iter().enumerate() {
        let locator = entry.relative.restore()?;
        if locator
            .parent()
            .is_none_or(|parent| !directories.contains(parent))
            || !relative.insert(locator)
            || entry.member != format!("{index:05}.raw")
            || entry.bytes != entry.stamp.2
            || entry.bytes > limits.max_file_bytes
            || !crate::model::valid_hash(&entry.sha256)
        {
            return Err(Error::Integrity);
        }
        total = total.checked_add(entry.bytes).ok_or(Error::Bounds)?;
        if total > limits.max_total_bytes {
            return Err(Error::Bounds);
        }
        let mut member = root.open(Path::new(&entry.member))?;
        crate::platform::private_file(&member)?;
        let before = crate::platform::stamp(&member)?;
        if crate::platform::hash_file(&mut member, limits.max_file_bytes)?
            != (entry.bytes, entry.sha256.clone())
            || crate::platform::stamp(&member)? != before
            || root.named_stamp(Path::new(&entry.member))? != before
        {
            return Err(Error::Integrity);
        }
    }
    let source = SourceRoot::capture(root.path.clone()).map_err(|_| Error::Conflict)?;
    let observed = source
        .read_dir_relative_bounded(Path::new(""), limits.max_entries.saturating_add(1))
        .map_err(|_| Error::Conflict)?;
    if observed.len() != manifest.files.len() + 1
        || observed.iter().any(|entry| {
            entry.kind != SourceEntryKind::File
                || (entry.name != "manifest.json"
                    && !manifest
                        .files
                        .iter()
                        .any(|file| entry.name == file.member.as_str()))
        })
    {
        return Err(Error::Integrity);
    }
    source.verify().map_err(|_| Error::Conflict)?;
    root.verify()?;
    if root.named_stamp(Path::new("manifest.json"))? != initial {
        return Err(Error::Conflict);
    }
    Ok(())
}
