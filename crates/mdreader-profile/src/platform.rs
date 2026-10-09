use crate::RootLocator;
use crate::{model::relative, Error, Result, RootBinding};
use rustix::fs::{self as fs, Mode, OFlags};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

fn io(_: impl std::fmt::Display) -> Error {
    Error::Storage
}
pub(crate) type Stamp = (u64, u64, u64, i64, i64, i64, i64);
pub(crate) fn stamp(file: &File) -> Result<Stamp> {
    let m = file.metadata().map_err(io)?;
    Ok((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
fn directory(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        return Err(Error::Invalid);
    }
    let mut current = File::from(
        fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?,
    );
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => {
                current = File::from(
                    fs::openat(
                        &current,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                        Mode::empty(),
                    )
                    .map_err(io)?,
                )
            }
            _ => return Err(Error::Invalid),
        }
    }
    Ok(current)
}
pub(crate) struct Root {
    pub file: File,
    pub path: PathBuf,
    pub binding: RootBinding,
}
impl Root {
    pub fn capture(path: &Path) -> Result<Self> {
        let file = directory(path)?;
        let canonical = path.canonicalize().map_err(io)?;
        let m = file.metadata().map_err(io)?;
        let binding = RootBinding {
            windows_file_id: None,
            locator: RootLocator::capture(&canonical)?,
            device: m.dev(),
            inode: m.ino(),
        };
        let root = Self {
            file,
            path: canonical,
            binding,
        };
        root.verify()?;
        Ok(root)
    }
    pub fn private(path: &Path) -> Result<Self> {
        let root = Self::capture(path)?;
        if root.file.metadata().map_err(io)?.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid);
        }
        Ok(root)
    }
    pub fn verify(&self) -> Result<()> {
        let current = directory(&self.path)?;
        let current = stamp(&current)?;
        let original = stamp(&self.file)?;
        if (current.0, current.1) != (self.binding.device, self.binding.inode)
            || (original.0, original.1) != (current.0, current.1)
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub fn open(&self, path: &Path) -> Result<File> {
        relative(path)?;
        self.verify()?;
        let mut parent = self.file.try_clone().map_err(io)?;
        let mut parts = path.components().peekable();
        while let Some(Component::Normal(name)) = parts.next() {
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
            let file = File::from(
                fs::openat(
                    &parent,
                    name,
                    if parts.peek().is_some() {
                        flags | OFlags::DIRECTORY
                    } else {
                        flags
                    },
                    Mode::empty(),
                )
                .map_err(io)?,
            );
            if parts.peek().is_none() {
                let m = file.metadata().map_err(io)?;
                if !m.is_file() || m.nlink() != 1 {
                    return Err(Error::Invalid);
                }
                return Ok(file);
            }
            parent = file;
        }
        Err(Error::Invalid)
    }
    /// Inspect a named file without opening its inode. In particular, closing
    /// a separate SQLite/SHM descriptor can release this process's live locks.
    pub fn named_stamp(&self, path: &Path) -> Result<Stamp> {
        relative(path)?;
        self.verify()?;
        let parent = Self::capture(&self.path.join(path.parent().ok_or(Error::Invalid)?))?;
        let object = fs::statat(
            &parent.file,
            path.file_name().ok_or(Error::Invalid)?,
            fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(io)?;
        if fs::FileType::from_raw_mode(object.st_mode) != fs::FileType::RegularFile
            || object.st_nlink != 1
        {
            return Err(Error::Invalid);
        }
        fn signed(value: impl TryInto<i64>) -> Result<i64> {
            value.try_into().map_err(|_| Error::Bounds)
        }
        fn unsigned(value: impl TryInto<u64>) -> Result<u64> {
            value.try_into().map_err(|_| Error::Bounds)
        }
        let stamp = (
            unsigned(object.st_dev)?,
            unsigned(object.st_ino)?,
            unsigned(object.st_size)?,
            signed(object.st_mtime)?,
            signed(object.st_mtime_nsec)?,
            signed(object.st_ctime)?,
            signed(object.st_ctime_nsec)?,
        );
        parent.verify()?;
        self.verify()?;
        Ok(stamp)
    }
}
pub(crate) fn hash_file(file: &mut File, bound: u64) -> Result<(u64, String)> {
    use sha2::{Digest, Sha256};
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(io)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::Bounds)?;
        if total > bound {
            return Err(Error::Bounds);
        }
        hash.update(&buffer[..n]);
    }
    Ok((total, crate::model::hex(&hash.finalize())))
}
pub(crate) fn copy_file(source: &mut File, dest: &mut File, bound: u64) -> Result<(u64, String)> {
    source.seek(SeekFrom::Start(0)).map_err(io)?;
    let copied = std::io::copy(&mut source.take(bound.saturating_add(1)), dest).map_err(io)?;
    if copied > bound {
        return Err(Error::Bounds);
    }
    dest.sync_all().map_err(io)?;
    hash_file(dest, bound)
}
pub(crate) struct Stage {
    pub root: Root,
    parent: Root,
    temporary: String,
    destination: String,
}
impl Stage {
    pub fn create(destination: &Path) -> Result<Self> {
        let parent = Root::private(destination.parent().ok_or(Error::Invalid)?)?;
        let destination = destination
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(Error::Invalid)?
            .to_owned();
        if destination.len() > 64
            || destination.is_empty()
            || !destination
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(Error::Invalid);
        }
        let temporary = format!(".profile-stage-{}", uuid::Uuid::new_v4());
        fs::mkdirat(&parent.file, temporary.as_str(), Mode::from_raw_mode(0o700)).map_err(io)?;
        parent.file.sync_all().map_err(io)?;
        let root = Root::private(&parent.path.join(&temporary))?;
        Ok(Self {
            root,
            parent,
            temporary,
            destination,
        })
    }
    pub fn create_file(&self, name: &str) -> Result<File> {
        if name.contains('/') || name.is_empty() {
            return Err(Error::Invalid);
        }
        self.root.verify()?;
        Ok(File::from(
            fs::openat(
                &self.root.file,
                name,
                OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::from_raw_mode(0o600),
            )
            .map_err(io)?,
        ))
    }
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let mut file = self.create_file(name)?;
        file.write_all(bytes).map_err(io)?;
        file.sync_all().map_err(io)
    }
    pub fn publish(self) -> Result<PathBuf> {
        self.root.verify()?;
        self.parent.verify()?;
        self.root.file.sync_all().map_err(io)?;
        fs::renameat_with(
            &self.parent.file,
            self.temporary.as_str(),
            &self.parent.file,
            self.destination.as_str(),
            fs::RenameFlags::NOREPLACE,
        )
        .map_err(|_| Error::Conflict)?;
        self.parent.file.sync_all().map_err(io)?;
        self.parent.verify()?;
        let path = self.parent.path.join(self.destination);
        let published = Root::private(&path)?;
        if (published.binding.device, published.binding.inode)
            != (self.root.binding.device, self.root.binding.inode)
        {
            return Err(Error::Conflict);
        }
        Ok(path)
    }
}
