//! Retained local directory chains plus source-relative no-reparse publication.
use super::*;
use crate::durable::RootIdentity;
use mdreader_core::SourceRoot;
use std::{
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(crate) fn path_identity(path: &Path) -> Result<RootIdentity> {
    Ok(RootIdentity::Windows {
        windows: directory(path)?.identity().map_err(capture)?,
    })
}
fn read(root: &SourceRoot, relative: &Path, bound: usize) -> Result<Option<String>> {
    let Some(file) = root.open_optional_file_relative(relative).map_err(core)? else {
        return Ok(None);
    };
    let identity = mdreader_capture::selected_file_identity(&file).map_err(capture)?;
    let before = file.metadata()?;
    if before.len() > bound as u64 {
        return Err(Error::Bounds);
    }
    let mut bytes = Vec::new();
    (&file).take(bound as u64 + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = root
        .open_optional_file_relative(relative)
        .map_err(core)?
        .ok_or(Error::Conflict)?;
    if bytes.len() > bound
        || before.len() != after.len()
        || before.modified()? != after.modified()?
        || identity != mdreader_capture::selected_file_identity(&named).map_err(capture)?
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(core)?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| Error::Invalid)
}
pub(crate) fn read_optional(path: &Path, bound: usize) -> Result<Option<String>> {
    let parent = path.parent().ok_or(Error::Invalid)?;
    match std::fs::symlink_metadata(parent) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let chain = directory(parent)?;
    let root = SourceRoot::capture(parent.to_owned()).map_err(core)?;
    let value = read(
        &root,
        Path::new(path.file_name().ok_or(Error::Invalid)?),
        bound,
    )?;
    chain.verify().map_err(capture)?;
    Ok(value)
}
pub(crate) fn safe_target(root: &Path, target: &Path) -> Result<()> {
    let chain = directory(root)?;
    let relative = target.strip_prefix(root).map_err(|_| Error::Invalid)?;
    let mut current = root.to_owned();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(Error::Invalid);
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(_) if current == target => {
                read_optional(&current, 4 * 1024 * 1024)?;
            }
            Ok(_) => {
                directory(&current)?.verify().map_err(capture)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    chain.verify().map_err(capture)
}
pub(crate) fn create_config_parent(root: &Path, target: &Path) -> Result<()> {
    safe_target(root, target)?;
    let chain = directory(root)?;
    let owner = SourceRoot::capture(root.to_owned()).map_err(core)?;
    let relative = target
        .parent()
        .ok_or(Error::Invalid)?
        .strip_prefix(root)
        .map_err(|_| Error::Invalid)?;
    if !relative.as_os_str().is_empty() {
        owner
            .create_or_capture_directory_relative(relative)
            .map_err(core)?
            .verify()
            .map_err(core)?;
    }
    chain.verify().map_err(capture)?;
    safe_target(root, target)
}
pub(crate) fn replace_config(
    root: &Path,
    expected_identity: RootIdentity,
    path: &Path,
    expected: Option<&str>,
    replacement: Option<&str>,
) -> Result<()> {
    let chain = directory(root)?;
    if path_identity(root)? != expected_identity {
        return Err(Error::Conflict);
    }
    let owner = SourceRoot::capture(root.to_owned()).map_err(core)?;
    let relative = path.strip_prefix(root).map_err(|_| Error::Invalid)?;
    let parent = relative.parent().ok_or(Error::Invalid)?;
    if read(&owner, relative, 4 * 1024 * 1024)?.as_deref() != expected {
        return Err(Error::Conflict);
    }
    if let Some(text) = replacement {
        let temporary = parent.join(format!(
            ".mdreader-integration-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let parent_authority = directory(&root.join(parent))?;
        let mut file = parent_authority
            .create_private_file(Path::new(temporary.file_name().ok_or(Error::Invalid)?))
            .map_err(capture)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // Another editor may race compare/rename; this is the same explicitly
        // documented limitation as Unix, never permission to overwrite stale bytes.
        chain.verify().map_err(capture)?;
        if read(&owner, relative, 4 * 1024 * 1024)?.as_deref() != expected {
            return Err(Error::Conflict);
        }
        owner.rename_relative(&temporary, relative).map_err(core)?;
    } else if expected.is_some() {
        chain.verify().map_err(capture)?;
        if read(&owner, relative, 4 * 1024 * 1024)?.as_deref() != expected {
            return Err(Error::Conflict);
        }
        owner.remove_file_relative(relative).map_err(core)?;
    }
    chain.verify().map_err(capture)?;
    if read(&owner, relative, 4 * 1024 * 1024)?.as_deref() != replacement {
        return Err(Error::Conflict);
    }
    Ok(())
}
