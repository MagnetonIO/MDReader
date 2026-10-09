//! Opaque original preservation. Windows sharing denial owns the bytes, while
//! the application fence owns observed GUI/service closure. No SQLite original
//! is opened and no legacy grant is interpreted or restored.
use super::*;
use crate::{Limits, RelativeLocator, RootBinding, SnapshotIdentity};
use mdreader_capture::WindowsObjectStamp;
use mdreader_core::source_root::SourceEntryKind;
use std::collections::BTreeMap;
pub(super) mod files;
mod frozen;
mod recovery;
pub use recovery::RecoveredDatabase;

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
    pub fn acquire(application: LegacyApplicationFence, limits: Limits) -> Result<Self> {
        Self::acquire_retaining(&mut Some(application), limits)
    }
    /// Native Retry keeps the exact pre-exit observer on every failed admission.
    /// The slot is consumed only after full file/membership/closure verification.
    pub fn acquire_retaining(
        retained: &mut Option<LegacyApplicationFence>,
        limits: Limits,
    ) -> Result<Self> {
        limits.validate()?;
        let application = retained.as_mut().ok_or(Error::Conflict)?;
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
        verify_parts(application, &binding, &scan, &files, &limits)?;
        Ok(Self {
            application: retained.take().ok_or(Error::Conflict)?,
            binding,
            scan,
            files,
            limits,
        })
    }
    /// Metadata for a later explicit return launch, never a serialized closure proof.
    pub fn launch_record(&mut self) -> Result<super::LaunchRecord> {
        self.revalidate()?;
        self.application.launch_record()
    }
    pub fn path(&self) -> &Path {
        &self.application.profile.path
    }
    pub fn binding(&self) -> &RootBinding {
        &self.binding
    }
    /// Full membership and identity fence. A new directory/file or restarted
    /// exact owner is a refusal even when every previously retained file matches.
    pub fn revalidate(&mut self) -> Result<()> {
        verify_parts(
            &mut self.application,
            &self.binding,
            &self.scan,
            &self.files,
            &self.limits,
        )
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
        let inventory = self.inventory()?;
        let limits = self.limits.clone();
        let verified = crate::stage(self, inventory, &canonical, &limits)?;
        parent.verify()?;
        self.revalidate()?;
        Ok(verified.identity().clone())
    }
    /// Reopen a published opaque archive by an explicitly expected digest. This
    /// operation never grants access to original paths recorded in its manifest.
    pub fn verify_archive(path: &Path, identity: &SnapshotIdentity, limits: &Limits) -> Result<()> {
        crate::verify(path, identity, limits).map(|_| ())
    }
}

fn verify_parts(
    application: &mut LegacyApplicationFence,
    binding: &RootBinding,
    observed: &Scan,
    files: &BTreeMap<PathBuf, Original>,
    limits: &Limits,
) -> Result<()> {
    application.verify_closed()?;
    if RootBinding::capture(&application.profile.path)? != *binding {
        return Err(Error::Conflict);
    }
    for (relative, original) in files {
        let named = application
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
        files::single_stream(&application.profile.path.join(relative))?;
    }
    let now = scan(&application.profile, limits)?;
    if now.files != observed.files || now.directories.len() != observed.directories.len() {
        return Err(Error::Conflict);
    }
    for (relative, (retained, stamp)) in &observed.directories {
        retained.verify().map_err(|_| Error::Conflict)?;
        if now
            .directories
            .get(relative)
            .is_none_or(|(_, current)| current != stamp)
        {
            return Err(Error::Conflict);
        }
    }
    application.verify_closed()
}
