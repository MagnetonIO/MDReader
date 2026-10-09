//! Current-user profile inventory authority on fixed local NTFS volumes.
//! Every directory component remains pinned, with no write/delete sharing.
use crate::{model::relative, Error, Result, RootBinding, RootLocator};
use mdreader_capture::{observe_named_object, observe_object, PrivateDirectory, SelectedDirectory};
use mdreader_core::source_root::{SourceRoot, SourceRootIdentity};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

pub(crate) type Stamp = (u64, u64, u64, i64, i64, i64, i64);
pub(crate) fn stamp(file: &File) -> Result<Stamp> {
    let directory = file.metadata().map_err(|_| Error::Storage)?.is_dir();
    observe_object(file, directory, false).map_err(|_| Error::Invalid)
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
}
