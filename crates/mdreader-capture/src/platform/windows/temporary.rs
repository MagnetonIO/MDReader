//! Bounded cleanup inventory for known projection sibling stages only.
//! Native wildcard enumeration avoids scanning every acknowledged Markdown body.
use super::*;
use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;

struct Search(windows_sys::Win32::Foundation::HANDLE);
#[allow(unsafe_code)]
impl Drop for Search {
    fn drop(&mut self) {
        // SAFETY: FindFirstFileW transferred this one owned search HANDLE.
        unsafe {
            FindClose(self.0);
        }
    }
}

#[allow(unsafe_code)]
pub(crate) fn projection_temporaries(directory: &Path, maximum: usize) -> Result<Vec<PathBuf>> {
    let parent = authority::Chain::open(directory, false, true)?;
    let pattern = authority::wide(&directory.join(".capture-*.tmp"))?;
    let mut entry = WIN32_FIND_DATAW::default();
    // SAFETY: terminated wildcard within the pinned directory, bounded output.
    let handle = unsafe { FindFirstFileW(pattern.as_ptr(), &mut entry) };
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            parent.verify()?;
            return Ok(Vec::new());
        }
        return Err(error.into());
    }
    let search = Search(handle);
    let mut paths = Vec::new();
    loop {
        let length = entry
            .cFileName
            .iter()
            .position(|v| *v == 0)
            .ok_or(CaptureError::UnsafePath)?;
        let name =
            String::from_utf16(&entry.cFileName[..length]).map_err(|_| CaptureError::UnsafePath)?;
        let (pid, digest) = name
            .strip_prefix(".capture-")
            .and_then(|v| v.strip_suffix(".tmp"))
            .and_then(|v| v.split_once('-'))
            .ok_or(CaptureError::UnsafePath)?;
        if pid.parse::<u32>().is_err()
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
            || entry.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)
                != 0
        {
            return Err(CaptureError::UnsafePath);
        }
        if paths.len() == maximum {
            return Err(CaptureError::TooLarge);
        }
        paths.push(directory.join(name));
        // SAFETY: owned live search HANDLE and fixed output structure.
        if unsafe { FindNextFileW(search.0, &mut entry) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(error.into());
        }
    }
    parent.verify()?;
    Ok(paths)
}
