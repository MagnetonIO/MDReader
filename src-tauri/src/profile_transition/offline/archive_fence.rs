//! Short final evidence checks after full archive verification, without hashing
//! gigabytes under the selector transaction. Archive files are never live DBs.
use super::*;
use mdreader_core::SourceRoot;
use mdreader_profile::VerifiedSnapshot;
use std::{fs::File, os::unix::fs::MetadataExt};
type Stamp = (u64, u64, u64, i64, i64, i64, i64);
fn stamp(file: &File) -> Result<Stamp> {
    let m = file.metadata().map_err(|_| Error::Storage)?;
    if !m.is_file() || m.nlink() != 1 {
        return Err(Error::Integrity);
    }
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
pub(super) struct ArchiveFence {
    root: SourceRoot,
    files: Vec<(PathBuf, File, Stamp)>,
    names: std::collections::BTreeSet<std::ffi::OsString>,
}
impl ArchiveFence {
    pub(super) fn capture(path: &Path, snapshot: &VerifiedSnapshot) -> Result<Self> {
        let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
        let mut paths = vec![PathBuf::from("manifest.json")];
        paths.extend(
            snapshot
                .manifest()
                .entries
                .iter()
                .filter_map(|entry| entry.member.as_ref().map(PathBuf::from)),
        );
        if paths.len() > Limits::default().max_entries + 1 {
            return Err(Error::Bounds);
        }
        let mut files = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for path in paths {
            if path.components().count() != 1 || path.file_name().is_none() {
                return Err(Error::Invalid);
            }
            names.insert(path.file_name().ok_or(Error::Invalid)?.to_owned());
            let file = root
                .open_optional_file_relative(&path)
                .map_err(|_| Error::Integrity)?
                .ok_or(Error::Integrity)?;
            let expected = stamp(&file)?;
            files.push((path, file, expected));
        }
        let fence = Self { root, files, names };
        // Evidence was captured BEFORE this complete hash validation. A raced
        // replacement/change during hashing cannot be adopted as the baseline.
        snapshot.revalidate(&Limits::default())?;
        fence.verify()?;
        Ok(fence)
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        let mut names = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&self.root.path).map_err(|_| Error::Storage)? {
            names.insert(entry.map_err(|_| Error::Storage)?.file_name());
            if names.len() > self.names.len() {
                return Err(Error::Conflict);
            }
        }
        if names != self.names {
            return Err(Error::Conflict);
        }
        for (path, file, expected) in &self.files {
            let named = self
                .root
                .open_optional_file_relative(path)
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            if &stamp(file)? != expected || &stamp(&named)? != expected {
                return Err(Error::Conflict);
            }
        }
        self.root.verify().map_err(|_| Error::Conflict)
    }
}
use mdreader_profile::Limits;
