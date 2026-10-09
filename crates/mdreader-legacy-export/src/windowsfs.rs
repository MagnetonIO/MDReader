//! Read-only admission of completed, private NTFS archives. These handles are
//! evidence about exact archived bytes, never proof that a live app is stopped.
use crate::{model::validate_path, Error, Result};
use mdreader_capture::{
    observe_named_object, observe_object, private_observation_reader, PrivateDirectory,
    WindowsObjectStamp,
};
use std::{
    fs::File,
    path::{Component, Path, PathBuf},
};

pub(crate) struct Directory {
    private: PrivateDirectory,
    path: PathBuf,
}
fn leaf(name: &str) -> Result<()> {
    let path = Path::new(name);
    if name.is_empty()
        || name.len() > 256
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
        || name
            .bytes()
            .any(|b| b < 0x20 || matches!(b, b':' | b'/' | b'\\'))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(crate) fn private_directory(path: &Path) -> Result<Directory> {
    validate_path(path)?;
    // Admit the original spelling before canonicalization; junctions must not
    // become implicit grants. Both complete native object identities must match.
    let private = PrivateDirectory::open(path).map_err(|_| Error::Invalid)?;
    let canonical = path.canonicalize()?;
    let current = PrivateDirectory::open(&canonical).map_err(|_| Error::Invalid)?;
    if private.identity().map_err(|_| Error::Conflict)?
        != current.identity().map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    Ok(Directory {
        private,
        path: canonical,
    })
}
pub(crate) fn validate_directory(directory: &Directory, path: &Path) -> Result<()> {
    directory.private.verify().map_err(|_| Error::Conflict)?;
    let current = private_directory(path)?;
    if directory.private.identity().map_err(|_| Error::Conflict)?
        != current.private.identity().map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    Ok(())
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
    }
    Ok(())
}
pub(crate) struct Source {
    pub(crate) file: File,
    parent: Directory,
    path: PathBuf,
    original: WindowsObjectStamp,
    sqlite: bool,
}
impl Source {
    pub(crate) fn open(path: &Path, sqlite: bool) -> Result<Self> {
        validate_path(path)?;
        let parent = private_directory(path.parent().ok_or(Error::Invalid)?)?;
        let name = path
            .file_name()
            .and_then(|x| x.to_str())
            .ok_or(Error::Unsupported)?;
        leaf(name)?;
        let path = parent.path.join(name);
        let file = private_observation_reader(&path).map_err(|_| Error::Invalid)?;
        // Full NTFS file ID, private protected DACL, one link, no reparse point.
        let original = observe_object(&file, false, true).map_err(|_| Error::Invalid)?;
        if sqlite && original.2 == 0 {
            return Err(Error::Invalid);
        }
        let source = Self {
            file,
            parent,
            path,
            original,
            sqlite,
        };
        source.check_path()?;
        Ok(source)
    }
    pub(crate) fn check_path(&self) -> Result<()> {
        validate_directory(&self.parent, self.path.parent().ok_or(Error::Invalid)?)?;
        if observe_named_object(&self.path).map_err(|_| Error::Conflict)? != self.original
            || observe_object(&self.file, false, true).map_err(|_| Error::Conflict)?
                != self.original
        {
            return Err(Error::Conflict);
        }
        if self.sqlite {
            for suffix in ["-wal", "-shm", "-journal"] {
                let name = format!(
                    "{}{suffix}",
                    self.path
                        .file_name()
                        .and_then(|x| x.to_str())
                        .ok_or(Error::Invalid)?
                );
                match std::fs::symlink_metadata(self.parent.path.join(name)) {
                    Ok(_) => return Err(Error::Invalid),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
    pub(crate) fn validate(&mut self, hash: &str, bound: u64) -> Result<()> {
        let (length, current) = self.snapshot_hash(bound)?;
        if current != hash || length != self.original.2 {
            return Err(Error::Conflict);
        }
        self.check_path()
    }
    pub(crate) fn snapshot_hash(&mut self, bound: u64) -> Result<(u64, String)> {
        self.check_path()?;
        let result = crate::hash_file(&mut self.file, bound)?;
        self.check_path()?;
        Ok(result)
    }
}
