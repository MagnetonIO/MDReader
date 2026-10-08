//! Selected bounded ciphertext only, with exact file identity and no overwrite.
use super::owner::Cancel;
use mdreader_core::SourceRoot;
use mdreader_service::accounts::AccountError;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, AccountError>;
pub(super) const MAX_FILE: usize = 33792;
pub(super) fn path(bytes: Zeroizing<Vec<u8>>) -> Result<PathBuf> {
    let value = Zeroizing::new(
        std::str::from_utf8(bytes.as_slice())
            .map_err(|_| AccountError::Invalid)?
            .to_owned(),
    );
    let path = Path::new(value.as_str());
    if value.is_empty()
        || value.len() > 8192
        || value.chars().any(char::is_control)
        || !path.is_absolute()
    {
        return Err(AccountError::Invalid);
    }
    Ok(path.to_owned())
}
fn parent(path: &Path) -> Result<(SourceRoot, &Path)> {
    let root = SourceRoot::capture(path.parent().ok_or(AccountError::Invalid)?.to_owned())
        .map_err(|_| AccountError::Storage)?;
    let name = Path::new(path.file_name().ok_or(AccountError::Invalid)?);
    Ok((root, name))
}
pub(super) fn read(path: &Path, cancel: &Arc<Cancel>) -> Result<Vec<u8>> {
    cancel.check()?;
    let (root, name) = parent(path)?;
    let mut file = root
        .open_optional_file_relative(name)
        .map_err(|_| AccountError::Storage)?
        .ok_or(AccountError::Storage)?;
    let before = file.metadata().map_err(|_| AccountError::Storage)?;
    if !before.is_file() || before.len() > MAX_FILE as u64 {
        return Err(AccountError::Limit);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AccountError::Storage)?;
    let after = file.metadata().map_err(|_| AccountError::Storage)?;
    let observed = root
        .open_optional_file_relative(name)
        .map_err(|_| AccountError::Storage)?
        .ok_or(AccountError::Conflict)?;
    if bytes.len() > MAX_FILE
        || before.len() != after.len()
        || unix_changed(&before, &after)
        || before.modified().ok() != after.modified().ok()
        || same_file::Handle::from_file(file).map_err(|_| AccountError::Storage)?
            != same_file::Handle::from_file(observed).map_err(|_| AccountError::Storage)?
    {
        return Err(AccountError::Conflict);
    }
    root.verify().map_err(|_| AccountError::Conflict)?;
    cancel.check()?;
    Ok(bytes)
}
pub(super) fn write(path: &Path, bytes: &[u8], cancel: &Arc<Cancel>) -> Result<()> {
    if bytes.len() > MAX_FILE {
        return Err(AccountError::Limit);
    }
    let (root, name) = parent(path)?;
    cancel.check()?;
    let mut file = root
        .create_new_relative(name)
        .map_err(|_| AccountError::Storage)?;
    // Already-issued filesystem effects remain owned through completion. On
    // cancellation/error keep uncertain encrypted output; never unlink by name.
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| AccountError::Storage)?;
    #[cfg(unix)]
    root.try_clone_directory_handle()
        .map_err(|_| AccountError::Storage)?
        .sync_all()
        .map_err(|_| AccountError::Storage)?;
    root.verify().map_err(|_| AccountError::Conflict)?;
    let observed = root
        .open_optional_file_relative(name)
        .map_err(|_| AccountError::Storage)?
        .ok_or(AccountError::Conflict)?;
    if same_file::Handle::from_file(file).map_err(|_| AccountError::Storage)?
        != same_file::Handle::from_file(observed).map_err(|_| AccountError::Storage)?
        || read(path, cancel)? != bytes
    {
        return Err(AccountError::Conflict);
    }
    root.verify().map_err(|_| AccountError::Conflict)?;
    cancel.check()
}

fn unix_changed(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        before.nlink() != 1
            || after.nlink() != 1
            || (before.ctime(), before.ctime_nsec()) != (after.ctime(), after.ctime_nsec())
    }
    #[cfg(not(unix))]
    {
        let _ = (before, after);
        false
    }
}
