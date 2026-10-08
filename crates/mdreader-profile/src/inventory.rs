#[cfg(unix)]
use crate::platform::{stamp, Root, Stamp};
use crate::RelativeLocator;
use crate::{
    model::{digest, relative},
    Disposition, Error, Limits, ManifestEntry, Result, RootBinding,
};
use std::path::{Path, PathBuf};

/// Explicit classification supplied by the reviewed native profile owner.
/// Every discovered regular file must be present exactly once; no implicit skips.
#[derive(Clone)]
pub struct InventorySelection {
    pub relative: PathBuf,
    pub disposition: Disposition,
}
pub struct Inventory {
    pub(crate) binding: RootBinding,
    pub(crate) identity: String,
    pub(crate) entries: Vec<ManifestEntry>,
    #[cfg(unix)]
    pub(crate) root: Root,
    #[cfg(unix)]
    pub(crate) files: Vec<(PathBuf, std::fs::File, Stamp)>,
    #[cfg(unix)]
    pub(crate) directories: Vec<(PathBuf, Stamp)>,
}
impl Inventory {
    pub fn capture(root: &Path, selected: &[InventorySelection], limits: &Limits) -> Result<Self> {
        limits.validate()?;
        #[cfg(not(unix))]
        {
            let _ = (root, selected);
            Err(Error::Unsupported)
        }
        #[cfg(unix)]
        {
            if selected.len() > limits.max_entries {
                return Err(Error::Bounds);
            }
            let root = Root::capture(root)?;
            let mut requested = std::collections::BTreeMap::new();
            for choice in selected {
                relative(&choice.relative)?;
                if choice
                    .relative
                    .file_name()
                    .is_some_and(|s| s == "reader-endpoint.json")
                    && choice.disposition
                        != Disposition::Excluded(crate::Exclusion::RuntimeEndpointOrLock)
                {
                    return Err(Error::Invalid);
                }
                if requested
                    .insert(choice.relative.clone(), choice.disposition)
                    .is_some()
                {
                    return Err(Error::Invalid);
                }
            }
            let mut directories = Vec::new();
            let mut paths = Vec::new();
            walk(&root, &PathBuf::new(), limits, &mut directories, &mut paths)?;
            paths.sort();
            if paths.len() != requested.len() {
                return Err(Error::Unclassified);
            }
            let mut files = Vec::new();
            let mut entries = Vec::new();
            let mut total = 0u64;
            for path in paths {
                let disposition = *requested.get(&path).ok_or(Error::Unclassified)?;
                let file = root.open(&path)?;
                let metadata = stamp(&file)?;
                if metadata.2 > limits.max_file_bytes {
                    return Err(Error::Bounds);
                }
                total = total.checked_add(metadata.2).ok_or(Error::Bounds)?;
                if total > limits.max_total_bytes {
                    return Err(Error::Bounds);
                }
                entries.push(ManifestEntry {
                    source_stamp: metadata,
                    relative: RelativeLocator::capture(&path)?,
                    disposition,
                    member: None,
                    bytes: metadata.2,
                    sha256: None,
                });
                files.push((path, file, metadata));
            }
            let binding = root.binding.clone();
            let file_stamps = files.iter().map(|(_, _, stamp)| *stamp).collect::<Vec<_>>();
            let directory_stamps = directories
                .iter()
                .filter(|(path, _)| !path.as_os_str().is_empty())
                .map(|(path, stamp)| Ok((RelativeLocator::capture(path)?, *stamp)))
                .collect::<Result<Vec<_>>>();
            // The root directory has an empty relative name; bind it separately.
            let directory_stamps = directory_stamps?;
            let identity = digest(&(
                binding.clone(),
                entries.clone(),
                file_stamps,
                directory_stamps,
            ))?;
            let result = Self {
                root,
                binding,
                identity,
                entries,
                files,
                directories,
            };
            result.revalidate()?;
            Ok(result)
        }
    }
    pub fn binding(&self) -> &RootBinding {
        &self.binding
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }
    pub(crate) fn revalidate(&self) -> Result<()> {
        #[cfg(not(unix))]
        {
            Err(Error::Unsupported)
        }
        #[cfg(unix)]
        {
            self.root.verify()?;
            for (path, original, expected) in &self.files {
                if stamp(original)? != *expected || stamp(&self.root.open(path)?)? != *expected {
                    return Err(Error::Conflict);
                }
            }
            for (path, expected) in &self.directories {
                let current = Root::capture(&self.root.path.join(path))?;
                if stamp(&current.file)? != *expected {
                    return Err(Error::Conflict);
                }
            }
            Ok(())
        }
    }
}
#[cfg(unix)]
fn walk(
    root: &Root,
    relative: &Path,
    limits: &Limits,
    directories: &mut Vec<(PathBuf, Stamp)>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if relative.components().count() > limits.max_depth {
        return Err(Error::Bounds);
    }
    let directory = Root::capture(&root.path.join(relative))?;
    let initial = stamp(&directory.file)?;
    directories.push((relative.to_path_buf(), initial));
    for entry in std::fs::read_dir(&directory.path).map_err(|_| Error::Storage)? {
        if directories.len() + files.len() > limits.max_entries {
            return Err(Error::Bounds);
        }
        let entry = entry.map_err(|_| Error::Storage)?;
        let kind = entry.file_type().map_err(|_| Error::Storage)?;
        let path = relative.join(entry.file_name());
        if kind.is_dir() {
            walk(root, &path, limits, directories, files)?;
        } else if kind.is_file() {
            files.push(path);
        } else {
            return Err(Error::Invalid);
        }
    }
    directory.verify()?;
    if stamp(&directory.file)? != initial {
        return Err(Error::Conflict);
    }
    Ok(())
}
