use super::types::*;
use crate::{device::assets, SourceRoot};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
#[cfg(not(windows))]
type Stamp = crate::source_root::file_stamp::SourceFileStamp;
#[cfg(windows)]
type Stamp = mdreader_capture::WindowsObjectStamp;

pub(super) struct FileEvidence {
    root: SourceRoot,
    path: PathBuf,
    file: File,
    stamp: Stamp,
}
fn stamp(file: &File) -> PublicationResult<Stamp> {
    #[cfg(windows)]
    {
        // Full 128-bit NTFS identity, regular one-link object, change/write
        // stamps and protected current-user DACL. No pathname-only evidence.
        mdreader_capture::observe_object(file, false, true)
            .map_err(|_| crate::device::Error::AuthorityChanged.into())
    }
    #[cfg(not(windows))]
    {
        Ok(crate::source_root::file_stamp::single_link_stamp(file)?)
    }
}
impl FileEvidence {
    pub(super) fn read(
        root: &SourceRoot,
        path: PathBuf,
        bytes: u32,
    ) -> PublicationResult<(Self, Vec<u8>)> {
        let mut file = root
            .open_optional_file_relative(&path)?
            .ok_or(PublicationError::Incomplete)?;
        let before = stamp(&file)?;
        if file.metadata()?.len() != u64::from(bytes) {
            return Err(PublicationError::Incomplete);
        }
        let mut body = Vec::with_capacity(bytes as usize + 1);
        Read::by_ref(&mut file)
            .take(u64::from(bytes) + 1)
            .read_to_end(&mut body)?;
        if body.len() != bytes as usize || stamp(&file)? != before {
            return Err(PublicationError::Incomplete);
        }
        let evidence = Self {
            root: root.clone(),
            path,
            file,
            stamp: before,
        };
        evidence.verify()?;
        Ok((evidence, body))
    }
    pub fn verify(&self) -> PublicationResult<()> {
        self.root.verify()?;
        let current = self
            .root
            .open_optional_file_relative(&self.path)?
            .ok_or(PublicationError::Incomplete)?;
        if stamp(&self.file)? != self.stamp || stamp(&current)? != self.stamp {
            return Err(PublicationError::Incomplete);
        }
        Ok(())
    }
}
fn write_exact(root: &SourceRoot, path: &Path, bytes: &[u8]) -> PublicationResult<()> {
    if let Some(mut existing) = root.open_optional_file_relative(path)? {
        let size = existing.metadata()?.len();
        if size > bytes.len() as u64 {
            return Err(PublicationError::Incomplete);
        }
        let mut prefix = vec![0; size as usize];
        existing.read_exact(&mut prefix)?;
        if prefix != bytes[..size as usize] {
            return Err(PublicationError::Incomplete);
        }
        let mut file = root.open_append_relative(path)?;
        file.write_all(&bytes[size as usize..])?;
        file.sync_all()?;
    } else {
        let mut file = root.create_new_relative(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    root.try_clone_directory_handle()?.sync_all()?;
    root.verify()?;
    Ok(())
}
impl WritePlan {
    /// Retry only exact known bytes, including a matching interrupted prefix.
    /// No ReaderStore or SQLite borrow is retained during file I/O.
    pub fn write(self, input: IncomingDocument) -> PublicationResult<PreparedIncomingCopy> {
        if input.digest != self.intent.digest {
            return Err(PublicationError::Conflict);
        }
        write_exact(&self.root, Path::new("document.md"), &input.markdown)?;
        let root = self.assets.incoming_root();
        for (record, asset) in self.intent.assets.iter().zip(&input.assets) {
            write_exact(
                &root,
                Path::new(&format!("{}.bin", record.id)),
                &asset.bytes,
            )?;
        }
        drop(input);
        self.read()
    }
    /// Explicit reopen reads only exact persisted generated names, with strict caps.
    pub fn read(self) -> PublicationResult<PreparedIncomingCopy> {
        if self.intent.bytes > 1048576 {
            return Err(PublicationError::InvalidInput);
        }
        let (document_evidence, markdown) =
            FileEvidence::read(&self.root, "document.md".into(), self.intent.bytes)?;
        let mut evidence = vec![document_evidence];
        let root = self.assets.incoming_root();
        let mut selected = Vec::new();
        for record in &self.intent.assets {
            if record.bytes == 0 || record.bytes > 4194304 {
                return Err(PublicationError::InvalidInput);
            }
            let (file, body) =
                FileEvidence::read(&root, format!("{}.bin", record.id).into(), record.bytes)?;
            evidence.push(file);
            selected.push(IncomingAsset::new(
                record.target.clone(),
                record.mime.clone(),
                body,
            )?);
        }
        let document = IncomingDocument::new(self.intent.name.clone(), markdown, selected)?;
        if document.digest != self.intent.digest || document.revision != self.intent.revision {
            return Err(PublicationError::Incomplete);
        }
        Ok(PreparedIncomingCopy {
            plan: self,
            document,
            evidence,
        })
    }
}
impl PreparedIncomingCopy {
    /// Full pixel decode under one carried runtime permit, outside the owner mutex.
    pub fn validate(self, permit: assets::ValidationPermit) -> PublicationResult<ReadyIncoming> {
        let mut rasters = Vec::new();
        for (asset, record) in self.document.assets.iter().zip(&self.plan.intent.assets) {
            let raster = assets::decode_incoming(&asset.bytes)?;
            if raster.hash != record.hash || raster.mime != record.mime {
                return Err(PublicationError::InvalidInput);
            }
            rasters.push(raster);
        }
        for evidence in &self.evidence {
            evidence.verify()?;
        }
        Ok(ReadyIncoming {
            plan: self.plan,
            _permit: permit,
            rasters,
            evidence: self.evidence,
        })
    }
}
