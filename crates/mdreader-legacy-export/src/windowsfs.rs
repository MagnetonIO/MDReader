//! Private NTFS archive admission and completed-stage publication. These
//! handles bind exact archived bytes, never prove that a live app is stopped.
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
    pub(crate) fn identity(&self) -> (u64, u64, i64) {
        (self.original.0, self.original.1, self.original.3)
    }
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

pub(crate) fn open_file(parent: &Directory, name: &str) -> Result<File> {
    leaf(name)?;
    validate_directory(parent, &parent.path)?;
    let file = private_observation_reader(&parent.path.join(name)).map_err(|_| Error::Invalid)?;
    observe_object(&file, false, true).map_err(|_| Error::Invalid)?;
    validate_directory(parent, &parent.path)?;
    Ok(file)
}

/// File contents are flushed by their writer before this identity-preserving,
/// no-replace rename. Windows provides no portable parent-directory fsync here;
/// a receipt is admitted only after complete post-publication verification.
pub(crate) struct Stage {
    parent: Directory,
    directory: PrivateDirectory,
    destination: String,
    pub(crate) path: PathBuf,
}
impl Stage {
    pub(crate) fn create(destination: &Path) -> Result<Self> {
        valid_destination(destination)?;
        let parent = private_directory(destination.parent().ok_or(Error::Invalid)?)?;
        let destination = destination
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::Invalid)?
            .to_owned();
        match std::fs::symlink_metadata(parent.path.join(&destination)) {
            Ok(_) => return Err(Error::Conflict),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let name = format!(".mdreader-export-{}", uuid::Uuid::new_v4());
        let directory = parent
            .private
            .create_child(Path::new(&name))
            .map_err(|_| Error::Conflict)?;
        let path = parent.path.join(name);
        let stage = Self {
            parent,
            directory,
            destination,
            path,
        };
        stage.validate()?;
        Ok(stage)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        validate_directory(&self.parent, &self.parent.path)?;
        self.directory.verify().map_err(|_| Error::Conflict)?;
        let named = private_directory(&self.path)?;
        if self.directory.identity().map_err(|_| Error::Conflict)?
            != named.private.identity().map_err(|_| Error::Conflict)?
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub(crate) fn create_file(&self, name: &str) -> Result<File> {
        leaf(name)?;
        self.validate()?;
        let file = self
            .directory
            .create_database_file(Path::new(name))
            .map_err(|_| Error::Conflict)?;
        self.validate()?;
        Ok(file)
    }
    pub(crate) fn remove_file(&self, name: &str) -> Result<()> {
        leaf(name)?;
        self.validate()?;
        let file = open_file(&private_directory(&self.path)?, name)?;
        drop(file);
        std::fs::remove_file(self.path.join(name))?;
        self.validate()
    }
    pub(crate) fn sync(&self) -> Result<()> {
        // Every writer sync_all()s its contents. This is a namespace identity
        // fence, not a claim of Windows directory-flush or volume durability.
        self.validate()
    }
    pub(crate) fn publish(self) -> Result<()> {
        self.validate()?;
        let expected = self.directory.identity().map_err(|_| Error::Conflict)?;
        let published = self
            .directory
            .publish_sibling(Path::new(&self.destination))
            .map_err(|_| Error::Conflict)?;
        if published.identity().map_err(|_| Error::Conflict)? != expected {
            return Err(Error::Conflict);
        }
        validate_directory(&self.parent, &self.parent.path)?;
        published.verify().map_err(|_| Error::Conflict)
    }
}
