//! Retained no-follow archive membership/stamps after one complete validation.
//! Final lifecycle fences do not hash large originals under a SQLite transaction.
use super::*;
use mdreader_profile::VerifiedSnapshot;
use std::{collections::BTreeSet, ffi::OsString, fs::File};
pub(super) struct Fence {
    root: SourceRoot,
    files: Vec<(PathBuf, File, mdreader_capture::WindowsObjectStamp)>,
    names: BTreeSet<OsString>,
}
impl Fence {
    pub(super) fn capture(path: &Path, snapshot: &VerifiedSnapshot) -> Result<Self> {
        let root = selection::restore(&mdreader_profile::RootBinding::capture_private(path)?)?;
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
        let mut names = BTreeSet::new();
        for path in paths {
            if path.components().count() != 1 {
                return Err(Error::Invalid);
            }
            names.insert(path.file_name().ok_or(Error::Invalid)?.to_owned());
            let file = root
                .open_optional_file_relative(&path)
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Integrity)?;
            let stamp = mdreader_capture::observe_object(&file, false, true)
                .map_err(|_| Error::Conflict)?;
            files.push((path, file, stamp));
        }
        let result = Self { root, files, names };
        snapshot.revalidate(&Limits::default())?;
        result.verify()?;
        Ok(result)
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        let names = self
            .root
            .read_dir_relative_bounded(Path::new(""), self.names.len() + 1)
            .map_err(|_| Error::Conflict)?
            .into_iter()
            .map(|entry| entry.name)
            .collect::<BTreeSet<_>>();
        if names != self.names {
            return Err(Error::Conflict);
        }
        for (path, file, stamp) in &self.files {
            let named = self
                .root
                .open_optional_file_relative(path)
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            if mdreader_capture::observe_object(file, false, true).map_err(|_| Error::Conflict)?
                != *stamp
                || mdreader_capture::observe_object(&named, false, true)
                    .map_err(|_| Error::Conflict)?
                    != *stamp
            {
                return Err(Error::Conflict);
            }
        }
        self.root.verify().map_err(|_| Error::Conflict)
    }
}
