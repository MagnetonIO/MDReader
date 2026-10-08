use crate::{model::validate_path, Error, Result};
use rustix::fs::{self as rfs, Mode, OFlags};
use std::{
    ffi::OsString,
    fs::{File, Metadata},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};
fn io(error: rustix::io::Errno) -> Error {
    Error::Io(error.into())
}
fn identity(m: &Metadata) -> (u64, u64) {
    (m.dev(), m.ino())
}
fn stamp(m: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
fn normalized(path: &Path) -> Result<PathBuf> {
    validate_path(path)?;
    let parent = std::fs::canonicalize(path.parent().ok_or(Error::Invalid)?)?;
    validate_path(&parent)?;
    Ok(parent.join(path.file_name().ok_or(Error::Invalid)?))
}
fn directory(path: &Path) -> Result<File> {
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
    for c in path.components() {
        match c {
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
pub(crate) fn private_directory(path: &Path) -> Result<File> {
    let handle = directory(&normalized(path)?)?;
    if handle.metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid);
    };
    Ok(handle)
}
pub(crate) fn validate_directory(handle: &File, path: &Path) -> Result<()> {
    let current = directory(&normalized(path)?)?;
    if identity(&handle.metadata()?) != identity(&current.metadata()?) {
        return Err(Error::Conflict);
    };
    Ok(())
}
pub(crate) fn open_file(parent: &File, name: &str) -> Result<File> {
    let file = File::from(
        rfs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io)?,
    );
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid);
    }
    Ok(file)
}
pub(crate) fn valid_destination(path: &Path) -> Result<()> {
    validate_path(path)?;
    let name = path
        .file_name()
        .and_then(|x| x.to_str())
        .ok_or(Error::Invalid)?;
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::Invalid);
    };
    Ok(())
}
pub(crate) struct Stage {
    parent: File,
    directory: File,
    parent_path: PathBuf,
    destination: OsString,
    name: String,
    pub path: PathBuf,
}
impl Stage {
    pub fn create(destination: &Path) -> Result<Self> {
        valid_destination(destination)?;
        let destination = normalized(destination)?;
        let parent_path = destination.parent().ok_or(Error::Invalid)?.to_path_buf();
        let parent = directory(&parent_path)?;
        if parent.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid);
        }
        let leaf = destination.file_name().ok_or(Error::Invalid)?.to_owned();
        match rfs::statat(&parent, &leaf, rfs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err(Error::Conflict),
            Err(e) if e == rustix::io::Errno::NOENT => {}
            Err(e) => return Err(io(e)),
        }
        let name = format!(".mdreader-export-{}", uuid::Uuid::new_v4());
        rfs::mkdirat(&parent, name.as_str(), Mode::RWXU).map_err(io)?;
        parent.sync_all()?;
        let handle = File::from(
            rfs::openat(
                &parent,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io)?,
        );
        let path = parent_path.join(&name);
        Ok(Self {
            parent,
            directory: handle,
            parent_path,
            destination: leaf,
            name,
            path,
        })
    }
    pub fn validate(&self) -> Result<()> {
        if identity(&self.parent.metadata()?)
            != identity(&directory(&self.parent_path)?.metadata()?)
        {
            return Err(Error::Conflict);
        };
        validate_directory(&self.directory, &self.path)
    }
    pub fn create_file(&self, name: &str) -> Result<File> {
        self.validate()?;
        Ok(File::from(
            rfs::openat(
                &self.directory,
                name,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(io)?,
        ))
    }
    pub fn remove_file(&self, name: &str) -> Result<()> {
        self.validate()?;
        rfs::unlinkat(&self.directory, name, rfs::AtFlags::empty()).map_err(io)
    }
    pub fn sync(&self) -> Result<()> {
        self.validate()?;
        self.directory.sync_all()?;
        Ok(())
    }
    pub fn publish(self) -> Result<()> {
        self.validate()?;
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "linux",
            target_os = "android",
            target_os = "redox"
        ))]
        rfs::renameat_with(
            &self.parent,
            self.name.as_str(),
            &self.parent,
            &self.destination,
            rfs::RenameFlags::NOREPLACE,
        )
        .map_err(|e| {
            if e == rustix::io::Errno::EXIST {
                Error::Conflict
            } else {
                io(e)
            }
        })?;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "linux",
            target_os = "android",
            target_os = "redox"
        )))]
        return Err(Error::Unsupported);
        self.parent.sync_all()?;
        validate_directory(&self.directory, &self.parent_path.join(self.destination))
    }
}
pub(crate) struct Source {
    pub file: File,
    parent: File,
    path: PathBuf,
    name: OsString,
    original: Metadata,
    sqlite: bool,
}
impl Source {
    pub fn open(path: &Path, sqlite: bool) -> Result<Self> {
        let path = normalized(path)?;
        let parent = directory(path.parent().ok_or(Error::Invalid)?)?;
        let name = path.file_name().ok_or(Error::Invalid)?.to_owned();
        let file = File::from(
            rfs::openat(
                &parent,
                &name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io)?,
        );
        let original = file.metadata()?;
        if !original.is_file() || original.len() == 0 {
            return Err(Error::Invalid);
        }
        let source = Self {
            file,
            parent,
            path,
            name,
            original,
            sqlite,
        };
        source.check_path()?;
        Ok(source)
    }
    pub fn identity(&self) -> (u64, u64) {
        identity(&self.original)
    }
    fn check_path(&self) -> Result<()> {
        let parent = directory(self.path.parent().ok_or(Error::Invalid)?)?;
        if identity(&parent.metadata()?) != identity(&self.parent.metadata()?) {
            return Err(Error::Conflict);
        }
        let current = File::from(
            rfs::openat(
                &self.parent,
                &self.name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io)?,
        );
        if stamp(&current.metadata()?) != stamp(&self.original)
            || stamp(&self.file.metadata()?) != stamp(&self.original)
        {
            return Err(Error::Conflict);
        }
        if self.sqlite {
            for suffix in ["-wal", "-shm", "-journal"] {
                let mut name = self.name.clone();
                name.push(suffix);
                match rfs::statat(&self.parent, &name, rfs::AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(_) => return Err(Error::Invalid),
                    Err(e) if e == rustix::io::Errno::NOENT => {}
                    Err(e) => return Err(io(e)),
                }
            }
        }
        Ok(())
    }
    pub fn validate(&mut self, hash: &str, bound: u64) -> Result<()> {
        let (length, current) = self.snapshot_hash(bound)?;
        if current != hash || length != self.original.len() {
            return Err(Error::Conflict);
        };
        self.check_path()
    }
    pub fn snapshot_hash(&mut self, bound: u64) -> Result<(u64, String)> {
        self.check_path()?;
        let result = crate::hash_file(&mut self.file, bound)?;
        self.check_path()?;
        Ok(result)
    }
}
