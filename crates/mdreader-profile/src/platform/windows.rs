//! Current-user profile inventory authority on fixed local NTFS volumes.
//! Every directory component remains pinned, with no write/delete sharing.
use crate::{model::relative, Error, Result, RootBinding, RootLocator};
use mdreader_capture::{observe_named_object, observe_object, PrivateDirectory, SelectedDirectory};
use mdreader_core::source_root::{SourceRoot, SourceRootIdentity};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
#[path = "file_io.rs"]
mod file_io;
#[path = "windows/publication.rs"]
mod publication;
pub(crate) use file_io::{copy_file, hash_file};

pub(crate) type Stamp = (u64, u64, u64, i64, i64, i64, i64);
pub(crate) fn stamp(file: &File) -> Result<Stamp> {
    let directory = file.metadata().map_err(|_| Error::Storage)?.is_dir();
    observe_object(file, directory, false).map_err(|_| Error::Invalid)
}
pub(crate) fn private_file(file: &File) -> Result<()> {
    observe_object(file, false, true)
        .map(|_| ())
        .map_err(|_| Error::Integrity)
}
pub(crate) fn backup_file(file: &File) -> Result<()> {
    private_file(file)
}
pub(crate) fn same_identity(a: &Stamp, b: &Stamp) -> bool {
    (a.0, a.1, a.3) == (b.0, b.1, b.3)
}

pub(crate) struct Root {
    pub file: File,
    pub path: PathBuf,
    pub binding: RootBinding,
    source: SourceRoot,
    chain: SelectedDirectory,
    private: Option<PrivateDirectory>,
}
impl Root {
    pub fn prepare_backup_journal(&self, name: &str) -> Result<()> {
        self.verify()?;
        let file = self
            .private
            .as_ref()
            .ok_or(Error::Invalid)?
            .create_database_file(Path::new(&format!("{name}-journal")))
            .map_err(|_| Error::Conflict)?;
        file.sync_all().map_err(|_| Error::Storage)?;
        drop(file);
        self.verify()
    }
    pub fn finish_backup_journal(&self, name: &str) -> Result<()> {
        self.verify()?;
        let relative = PathBuf::from(format!("{name}-journal"));
        if let Some(file) = self
            .source
            .open_optional_file_relative(&relative)
            .map_err(|_| Error::Conflict)?
        {
            private_file(&file)?;
            if file.metadata().map_err(|_| Error::Storage)?.len() != 0 {
                return Err(Error::Integrity);
            }
            drop(file);
            self.source
                .remove_file_relative(&relative)
                .map_err(|_| Error::Conflict)?;
        }
        self.verify()
    }
    pub fn open_backup_member(&self, path: &Path) -> Result<File> {
        relative(path)?;
        if path.components().count() != 1 {
            return Err(Error::Invalid);
        }
        self.verify()?;
        let file = self
            .private
            .as_ref()
            .ok_or(Error::Invalid)?
            .database_file(path, false)
            .map_err(|_| Error::Invalid)?
            .ok_or(Error::Storage)?;
        self.verify()?;
        Ok(file)
    }
    pub fn capture(path: &Path) -> Result<Self> {
        // Admit the original spelling before canonicalization: junction/symlink
        // ancestors must never become an implicitly accepted canonical grant.
        let chain = SelectedDirectory::open(path).map_err(|_| Error::Invalid)?;
        let canonical = path.canonicalize().map_err(|_| Error::Storage)?;
        let source = SourceRoot::capture(canonical.clone()).map_err(|_| Error::Invalid)?;
        let SourceRootIdentity::Windows {
            volume_serial_number,
            file_id,
        } = &source.identity
        else {
            return Err(Error::Unsupported);
        };
        let file = source
            .try_clone_directory_handle()
            .map_err(|_| Error::Storage)?;
        let observed = stamp(&file)?;
        let mut low = [0u8; 8];
        low.copy_from_slice(&file_id[..8]);
        let binding = RootBinding {
            locator: RootLocator::capture(&canonical)?,
            device: *volume_serial_number,
            inode: u64::from_le_bytes(low),
            windows_file_id: Some(*file_id),
        };
        if observed.0 != binding.device || observed.1 != binding.inode {
            return Err(Error::Conflict);
        }
        // Both spellings must identify the same object, including all 128 bits.
        if chain.identity().map_err(|_| Error::Conflict)?
            != SelectedDirectory::open(&canonical)
                .map_err(|_| Error::Conflict)?
                .identity()
                .map_err(|_| Error::Conflict)?
        {
            return Err(Error::Conflict);
        }
        let root = Self {
            file,
            path: canonical,
            binding,
            source,
            chain,
            private: None,
        };
        root.verify()?;
        Ok(root)
    }
    pub fn private(path: &Path) -> Result<Self> {
        let mut root = Self::capture(path)?;
        root.private = Some(PrivateDirectory::open(&root.path).map_err(|_| Error::Invalid)?);
        root.verify()?;
        Ok(root)
    }
    pub fn verify(&self) -> Result<()> {
        self.chain.verify().map_err(|_| Error::Conflict)?;
        self.source.verify().map_err(|_| Error::Conflict)?;
        if let Some(private) = &self.private {
            private.verify().map_err(|_| Error::Conflict)?;
        }
        let current = stamp(&self.file)?;
        let id = self.binding.windows_file_id.ok_or(Error::Invalid)?;
        let mut high = [0u8; 8];
        high.copy_from_slice(&id[8..]);
        if (current.0, current.1, current.3)
            != (
                self.binding.device,
                self.binding.inode,
                i64::from_le_bytes(high),
            )
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub fn open(&self, path: &Path) -> Result<File> {
        relative(path)?;
        self.verify()?;
        let file = self
            .source
            .open_optional_file_relative(path)
            .map_err(|_| Error::Invalid)?
            .ok_or(Error::Storage)?;
        stamp(&file)?;
        self.verify()?;
        Ok(file)
    }
    pub fn named_stamp(&self, path: &Path) -> Result<Stamp> {
        relative(path)?;
        self.verify()?;
        let observed = observe_named_object(&self.path.join(path)).map_err(|_| Error::Invalid)?;
        self.verify()?;
        Ok(observed)
    }
    fn into_private(self) -> Result<PrivateDirectory> {
        let Self {
            file,
            source,
            chain,
            private,
            ..
        } = self;
        drop(file);
        drop(source);
        drop(chain);
        private.ok_or(Error::Invalid)
    }
}

pub(crate) struct Stage {
    pub root: Root,
    parent: Root,
    temporary: String,
    destination: String,
    nested: bool,
}
impl Stage {
    pub fn create_nested(parent: &Stage, destination: &Path) -> Result<Self> {
        parent.root.verify()?;
        if destination.parent() != Some(parent.root.path.as_path()) {
            return Err(Error::Invalid);
        }
        let mut stage = Self::create(destination)?;
        if stage.parent.binding != parent.root.binding {
            return Err(Error::Conflict);
        }
        parent.root.verify()?;
        stage.nested = true;
        Ok(stage)
    }
    pub fn create(destination: &Path) -> Result<Self> {
        let parent = Root::private(destination.parent().ok_or(Error::Invalid)?)?;
        let destination = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::Invalid)?
            .to_owned();
        if destination.is_empty()
            || destination.len() > 64
            || !destination
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(Error::Invalid);
        }
        let temporary = format!(".profile-stage-{}", uuid::Uuid::new_v4());
        let created = parent
            .private
            .as_ref()
            .ok_or(Error::Invalid)?
            .create_child(Path::new(&temporary))
            .map_err(|_| Error::Conflict)?;
        let root = Root::private(&parent.path.join(&temporary))?;
        drop(created);
        parent.verify()?;
        Ok(Self {
            root,
            parent,
            temporary,
            destination,
            nested: false,
        })
    }
    pub fn create_file(&self, name: &str) -> Result<File> {
        self.root.verify()?;
        self.root
            .private
            .as_ref()
            .ok_or(Error::Invalid)?
            .create_database_file(Path::new(name))
            .map_err(|_| Error::Storage)
    }
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let mut file = self.create_file(name)?;
        file.write_all(bytes).map_err(|_| Error::Storage)?;
        file.sync_all().map_err(|_| Error::Storage)
    }
    pub fn publish(self) -> Result<PathBuf> {
        self.root.verify()?;
        self.parent.verify()?;
        let manifest = if self
            .root
            .source
            .open_optional_file_relative(Path::new("bundle.json"))
            .map_err(|_| Error::Conflict)?
            .is_some()
        {
            "bundle.json"
        } else {
            "manifest.json"
        };
        let digest = hash_file(&mut self.root.open(Path::new(manifest))?, 8 * 1024 * 1024)?.1;
        let destination = self.parent.path.join(&self.destination);
        let mut expected = self.root.binding.clone();
        expected.locator = RootLocator::capture(&destination)?;
        let mut journal = if self.nested {
            None
        } else {
            Some(publication::Journal::open(&self.parent.path)?)
        };
        if let Some(journal) = journal.as_mut() {
            journal.prepare(
                &self.destination,
                &self.temporary,
                &expected,
                manifest,
                &digest,
            )?;
        }
        self.parent.verify()?;
        let published = self
            .root
            .into_private()?
            .publish_sibling(Path::new(&self.destination))
            .map_err(|_| Error::ReconciliationRequired)?;
        let root = Root::private(&destination).map_err(|_| Error::ReconciliationRequired)?;
        if root.binding != expected {
            return Err(Error::ReconciliationRequired);
        }
        let mut file = root
            .open(Path::new(manifest))
            .map_err(|_| Error::ReconciliationRequired)?;
        let before = stamp(&file)?;
        if hash_file(&mut file, 8 * 1024 * 1024)?.1 != digest {
            return Err(Error::ReconciliationRequired);
        }
        root.verify()?;
        if let Some(journal) = journal.as_mut() {
            journal.finish(
                &self.destination,
                &self.temporary,
                &expected,
                manifest,
                &digest,
            )?;
        }
        if stamp(&file)? != before || root.named_stamp(Path::new(manifest))? != before {
            return Err(Error::ReconciliationRequired);
        }
        root.verify()?;
        published
            .verify()
            .map_err(|_| Error::ReconciliationRequired)?;
        self.parent.verify()?;
        Ok(destination)
    }
}
