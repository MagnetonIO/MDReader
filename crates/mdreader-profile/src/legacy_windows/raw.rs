//! Opaque original preservation. Windows sharing denial owns the bytes, while
//! the application fence owns observed GUI/service closure. No SQLite original
//! is opened and no legacy grant is interpreted or restored.
use super::*;
use crate::{Limits, RelativeLocator, RootBinding, SnapshotIdentity};
use mdreader_capture::WindowsObjectStamp;
use mdreader_core::source_root::SourceEntryKind;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Seek, SeekFrom},
};
mod files;
mod recovery;
pub use recovery::RecoveredDatabase;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u8,
    id: uuid::Uuid,
    original: RootBinding,
    directories: Vec<Directory>,
    files: Vec<Entry>,
}
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Directory {
    relative: Vec<u16>,
    stamp: WindowsObjectStamp,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    relative: RelativeLocator,
    stamp: WindowsObjectStamp,
    member: String,
    bytes: u64,
    sha256: String,
}
struct Original {
    file: File,
    stamp: WindowsObjectStamp,
}
struct Scan {
    directories: BTreeMap<PathBuf, (SourceRoot, WindowsObjectStamp)>,
    files: Vec<PathBuf>,
}
fn scan(root: &SourceRoot, limits: &Limits) -> Result<Scan> {
    let mut result = Scan {
        directories: BTreeMap::new(),
        files: Vec::new(),
    };
    walk(root, Path::new(""), limits, &mut result)?;
    result.files.sort();
    Ok(result)
}
fn walk(root: &SourceRoot, relative: &Path, limits: &Limits, scan: &mut Scan) -> Result<()> {
    if relative.components().count() > limits.max_depth
        || scan.directories.len() + scan.files.len() >= limits.max_entries
    {
        return Err(Error::Bounds);
    }
    let directory = SourceRoot::capture(root.path.join(relative)).map_err(|_| Error::Conflict)?;
    let handle = directory
        .try_clone_directory_handle()
        .map_err(|_| Error::Conflict)?;
    let initial = observe_object(&handle, true, false).map_err(|_| Error::Conflict)?;
    files::directory_streams(&directory.path)?;
    scan.directories
        .insert(relative.to_owned(), (directory, initial));
    for entry in root
        .read_dir_relative_bounded(relative, limits.max_entries)
        .map_err(|_| Error::Conflict)?
    {
        if scan.directories.len() + scan.files.len() >= limits.max_entries {
            return Err(Error::Bounds);
        }
        let child = relative.join(entry.name);
        match entry.kind {
            SourceEntryKind::Directory => walk(root, &child, limits, scan)?,
            SourceEntryKind::File => scan.files.push(child),
            // No reparse-target discovery, implicit grants or lossy flattening.
            _ => return Err(Error::Unsupported),
        }
    }
    if observe_object(&handle, true, false).map_err(|_| Error::Conflict)? != initial {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)
}
/// Every original file is held without write/delete sharing for this capability's
/// lifetime. It cannot be reconstructed from a serialized offline assertion.
pub struct ClosedOriginal {
    application: LegacyApplicationFence,
    binding: RootBinding,
    scan: Scan,
    files: BTreeMap<PathBuf, Original>,
    limits: Limits,
}
impl ClosedOriginal {
    /// Consume the pre-exit observer only after all originally observed handles
    /// exited. Existing conflicting writers fail sharing admission; no signal,
    /// credential request, SQLite recovery or original ACL change is issued.
    pub fn acquire(mut application: LegacyApplicationFence, limits: Limits) -> Result<Self> {
        limits.validate()?;
        application.verify_closed()?;
        let binding = RootBinding::capture(&application.profile.path)?;
        let scan = scan(&application.profile, &limits)?;
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for relative in &scan.files {
            let file = files::retain(&application.profile, relative)?;
            let stamp = observe_object(&file, false, false).map_err(|_| Error::Conflict)?;
            if stamp.2 > limits.max_file_bytes {
                return Err(Error::Bounds);
            }
            total = total.checked_add(stamp.2).ok_or(Error::Bounds)?;
            if total > limits.max_total_bytes {
                return Err(Error::Bounds);
            }
            files.insert(relative.clone(), Original { file, stamp });
        }
        let mut closed = Self {
            application,
            binding,
            scan,
            files,
            limits,
        };
        closed.revalidate()?;
        Ok(closed)
    }
    pub fn binding(&self) -> &RootBinding {
        &self.binding
    }
    /// Full membership and identity fence. A new directory/file or restarted
    /// exact owner is a refusal even when every previously retained file matches.
    pub fn revalidate(&mut self) -> Result<()> {
        self.application.verify_closed()?;
        if RootBinding::capture(&self.application.profile.path)? != self.binding {
            return Err(Error::Conflict);
        }
        for (relative, original) in &self.files {
            let named = self
                .application
                .profile
                .open_optional_file_relative(relative)
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            if observe_object(&named, false, false).map_err(|_| Error::Conflict)? != original.stamp
                || observe_object(&original.file, false, false).map_err(|_| Error::Conflict)?
                    != original.stamp
            {
                return Err(Error::Conflict);
            }
            files::single_stream(&self.application.profile.path.join(relative))?;
        }
        let now = scan(&self.application.profile, &self.limits)?;
        if now.files != self.scan.files || now.directories.len() != self.scan.directories.len() {
            return Err(Error::Conflict);
        }
        for (relative, (retained, stamp)) in &self.scan.directories {
            retained.verify().map_err(|_| Error::Conflict)?;
            if now
                .directories
                .get(relative)
                .is_none_or(|(_, current)| current != stamp)
            {
                return Err(Error::Conflict);
            }
        }
        self.application.verify_closed()
    }
    /// Publish opaque originals plus a lossless Windows-wide relative locator
    /// manifest through the existing private SQLite publication journal. This
    /// receipt authorizes preservation only; native activation/import is separate.
    pub fn preserve(&mut self, destination: &Path) -> Result<SnapshotIdentity> {
        self.revalidate()?;
        let parent =
            crate::platform::Root::private(destination.parent().ok_or(Error::InvalidDestination)?)?;
        let canonical = parent
            .path
            .join(destination.file_name().ok_or(Error::InvalidDestination)?);
        let original = self.binding.locator.restore()?;
        if canonical.starts_with(&original) || original.starts_with(&canonical) {
            return Err(Error::InvalidDestination);
        }
        let stage = crate::platform::Stage::create(&canonical)?;
        parent.verify()?;
        let mut entries = Vec::new();
        for (index, (relative, original)) in self.files.iter_mut().enumerate() {
            let member = format!("{index:05}.raw");
            let mut output = stage.create_file(&member)?;
            original
                .file
                .seek(SeekFrom::Start(0))
                .map_err(|_| Error::Storage)?;
            let (bytes, sha256) = crate::platform::copy_file(
                &mut original.file,
                &mut output,
                self.limits.max_file_bytes,
            )?;
            if bytes != original.stamp.2
                || crate::platform::hash_file(&mut original.file, self.limits.max_file_bytes)?
                    != (bytes, sha256.clone())
            {
                return Err(Error::Conflict);
            }
            entries.push(Entry {
                relative: RelativeLocator::capture(relative)?,
                stamp: original.stamp,
                member,
                bytes,
                sha256,
            });
        }
        use std::os::windows::ffi::OsStrExt;
        let directories = self
            .scan
            .directories
            .iter()
            .map(|(relative, (_, stamp))| Directory {
                relative: relative.as_os_str().encode_wide().collect(),
                stamp: *stamp,
            })
            .collect();
        let manifest = Manifest {
            format: "mdreader-legacy-raw-windows".to_owned(),
            version: 1,
            id: uuid::Uuid::new_v4(),
            original: self.binding.clone(),
            directories,
            files: entries,
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|_| Error::Invalid)?;
        if bytes.len() > self.limits.max_manifest_bytes {
            return Err(Error::Bounds);
        }
        let identity = SnapshotIdentity {
            id: manifest.id,
            manifest_sha256: crate::model::hash(&bytes),
        };
        stage.write("manifest.json", &bytes)?;
        self.revalidate()?;
        let published = stage.publish()?;
        files::verify_archive(&published, &identity, &self.limits)?;
        self.revalidate()?;
        Ok(identity)
    }
    /// Reopen a published opaque archive by an explicitly expected digest. This
    /// operation never grants access to original paths recorded in its manifest.
    pub fn verify_archive(path: &Path, identity: &SnapshotIdentity, limits: &Limits) -> Result<()> {
        files::verify_archive(path, identity, limits)
    }
}
