//! Descriptor-relative publication. A path swap cannot redirect writes into a symlink target.
use crate::{IntegrationError as Error, Result};
use rustix::fs::{self as rfs, Mode, OFlags};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn io(error: rustix::io::Errno) -> Error {
    Error::Io(error.into())
}
pub(crate) fn directory(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        return Err(Error::Invalid);
    }
    let mut current = File::from(
        rfs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?,
    );
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                current = File::from(
                    rfs::openat(
                        &current,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(io)?,
                );
            }
            _ => return Err(Error::Invalid),
        }
    }
    Ok(current)
}
fn parent(path: &Path) -> Result<(File, &std::ffi::OsStr)> {
    Ok((
        directory(path.parent().ok_or(Error::Invalid)?)?,
        path.file_name().ok_or(Error::Invalid)?,
    ))
}
fn verify_parent(handle: &File, path: &Path) -> Result<()> {
    let current = directory(path.parent().ok_or(Error::Invalid)?)?;
    let a = handle.metadata()?;
    let b = current.metadata()?;
    if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
        return Err(Error::Conflict);
    }
    Ok(())
}
fn read_at(parent: &File, name: &std::ffi::OsStr, bound: usize) -> Result<Option<String>> {
    let fd = match rfs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => return Err(io(error)),
    };
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(Error::Invalid);
    }
    if metadata.len() > bound as u64 {
        return Err(Error::Bounds);
    }
    let mut bytes = Vec::new();
    file.take(bound as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > bound {
        return Err(Error::Bounds);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| Error::Invalid)
}
pub(crate) fn read_optional(path: &Path, bound: usize) -> Result<Option<String>> {
    let (parent, name) = match parent(path) {
        Ok(value) => value,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let result = read_at(&parent, name, bound)?;
    verify_parent(&parent, path)?;
    Ok(result)
}
pub(crate) fn lock_file(path: &Path) -> Result<File> {
    let (parent, name) = parent(path)?;
    let file = File::from(
        rfs::openat(
            &parent,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io)?,
    );
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid);
    }
    verify_parent(&parent, path)?;
    Ok(file)
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    // Normalize system aliases before opening the parent, never the final component.
    let parent_path = std::fs::canonicalize(path.parent().ok_or(Error::Invalid)?)?;
    let normalized = parent_path.join(path.file_name().ok_or(Error::Invalid)?);
    let (parent, name) = parent(&normalized)?;
    match rfs::mkdirat(&parent, name, Mode::RWXU) {
        Ok(()) => parent.sync_all()?,
        Err(error) if error == rustix::io::Errno::EXIST => {}
        Err(error) => return Err(io(error)),
    };
    let directory = File::from(
        rfs::openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?,
    );
    if directory.metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid);
    }
    verify_parent(&parent, &normalized)
}
pub(crate) fn replace(
    path: &Path,
    expected: Option<&str>,
    replacement: Option<&str>,
) -> Result<()> {
    let (parent, name) = parent(path)?;
    replace_at(path, &parent, name, expected, replacement)
}
pub(crate) fn replace_under(
    root: &Path,
    identity: (u64, u64),
    path: &Path,
    expected: Option<&str>,
    replacement: Option<&str>,
) -> Result<()> {
    let mut handle = directory(root)?;
    let metadata = handle.metadata()?;
    if (metadata.dev(), metadata.ino()) != identity {
        return Err(Error::Conflict);
    }
    let relative = path.strip_prefix(root).map_err(|_| Error::Invalid)?;
    for component in relative.parent().ok_or(Error::Invalid)?.components() {
        let Component::Normal(name) = component else {
            return Err(Error::Invalid);
        };
        handle = File::from(
            rfs::openat(
                &handle,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io)?,
        );
    }
    replace_at(
        path,
        &handle,
        relative.file_name().ok_or(Error::Invalid)?,
        expected,
        replacement,
    )
}
fn replace_at(
    path: &Path,
    parent: &File,
    name: &std::ffi::OsStr,
    expected: Option<&str>,
    replacement: Option<&str>,
) -> Result<()> {
    if read_at(parent, name, 4 * 1024 * 1024)?.as_deref() != expected {
        return Err(Error::Conflict);
    }
    if let Some(replacement) = replacement {
        let temporary = format!(
            ".mdreader-integration-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let result = (|| {
            let mut file = File::from(
                rfs::openat(
                    parent,
                    temporary.as_str(),
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )
                .map_err(io)?,
            );
            file.write_all(replacement.as_bytes())?;
            if expected.is_some() {
                let previous = File::from(
                    rfs::openat(
                        parent,
                        name,
                        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(io)?,
                );
                file.set_permissions(previous.metadata()?.permissions())?;
            }
            file.sync_all()?;
            verify_parent(parent, path)?;
            if read_at(parent, name, 4 * 1024 * 1024)?.as_deref() != expected {
                return Err(Error::Conflict);
            }
            rfs::renameat(parent, temporary.as_str(), parent, name).map_err(io)?;
            parent.sync_all()?;
            verify_parent(parent, path)
        })();
        let _ = rfs::unlinkat(parent, temporary.as_str(), rfs::AtFlags::empty());
        result
    } else {
        verify_parent(parent, path)?;
        if read_at(parent, name, 4 * 1024 * 1024)?.as_deref() != expected {
            return Err(Error::Conflict);
        }
        if expected.is_some() {
            rfs::unlinkat(parent, name, rfs::AtFlags::empty()).map_err(io)?;
            parent.sync_all()?;
        }
        verify_parent(parent, path)
    }
}
