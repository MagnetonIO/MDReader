//! Native-retained closure admission for the known installed legacy profile.
//! An application exit observer is retained before asking the user to close it.
use super::{legacy_applications::LegacyApplications, owner::ClosedOwner, *};
use mdreader_core::SourceRoot;
use std::{collections::BTreeMap, fs::File};

pub(super) struct LegacySource {
    root: SourceRoot,
    applications: LegacyApplications,
    modern: Option<ClosedOwner>,
    handoff: super::shutdown_handoff::ShutdownHandoff,
    locks: BTreeMap<&'static str, File>,
}
impl LegacySource {
    pub(super) fn capture(root: SourceRoot) -> Result<Self> {
        // Recognition requires both a known legacy store and the exact installed
        // profile/application observer. Arbitrary folders gain no closure claim.
        if !recognized(&root)? {
            return Err(Error::Unsupported);
        }
        let applications = LegacyApplications::capture(&root.path)?.ok_or(Error::Unsupported)?;
        Ok(Self {
            root,
            applications,
            modern: None,
            handoff: Default::default(),
            locks: BTreeMap::new(),
        })
    }
    pub(super) fn handoff_issued(&self) -> bool {
        self.handoff.issued()
    }
    pub(super) fn handoff_failure(&self) -> Option<&'static str> {
        self.handoff.failure_kind()
    }
    pub(super) fn close(&mut self) -> Result<()> {
        self.applications.verify_closed()?;
        self.root.verify().map_err(|_| Error::Conflict)?;
        // Mixed profiles can also have the current cooperating owner. Retain
        // its real lifetime locks and authenticated daemon shutdown capability.
        if self.modern.is_none()
            && self
                .root
                .path
                .join("reader-v1/library.sqlite")
                .try_exists()
                .map_err(|_| Error::Storage)?
        {
            self.modern = Some(ClosedOwner::acquire_retaining(
                &self.root.path,
                &mut self.handoff,
            )?);
        }
        for name in ["library.lock", "cloud-accounts.lock"] {
            // The modern owner already retains library.lock on another descriptor.
            if self.locks.contains_key(name) || (name == "library.lock" && self.modern.is_some()) {
                continue;
            }
            if let Some(file) = self
                .root
                .open_optional_file_relative(Path::new(name))
                .map_err(|_| Error::Conflict)?
            {
                fs2::FileExt::try_lock_exclusive(&file).map_err(|_| Error::NotQuiescent)?;
                self.locks.insert(name, file);
            }
        }
        self.verify()
    }
    pub(super) fn verify(&mut self) -> Result<()> {
        self.applications.verify_closed()?;
        self.root.verify().map_err(|_| Error::Conflict)?;
        use std::os::unix::fs::MetadataExt;
        for (name, retained) in &self.locks {
            let named = self
                .root
                .open_optional_file_relative(Path::new(name))
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            let old = retained.metadata().map_err(|_| Error::Storage)?;
            let now = named.metadata().map_err(|_| Error::Storage)?;
            if now.nlink() != 1 || (old.dev(), old.ino()) != (now.dev(), now.ino()) {
                return Err(Error::Conflict);
            }
        }
        if let Some(modern) = &mut self.modern {
            modern.revalidate()?;
        }
        Ok(())
    }
    pub(super) fn snapshot(&mut self) -> Result<super::archive_import::ArchiveImport> {
        self.close()?;
        let root = self.root.clone();
        let archive = super::legacy_snapshot::capture(&root, || self.verify())?;
        self.verify()?;
        super::archive_import::ArchiveImport::preview(&archive)
    }
    pub(super) fn launch_identity(&mut self) -> Result<super::legacy_launch::LegacyLaunch> {
        self.verify()?;
        self.applications.launch_identity()
    }
    pub(super) fn original_unchanged(&mut self, archive: &Path) -> Result<()> {
        self.verify()?;
        super::legacy_snapshot::verify_originals(&self.root, archive)?;
        self.verify()
    }
}

/// Recognize only fixed legacy store locations. This is format detection, not
/// proof of owner closure or permission to read/import any document bodies.
pub(crate) fn recognized(root: &SourceRoot) -> Result<bool> {
    use mdreader_core::source_root::SourceEntryKind;
    root.verify().map_err(|_| Error::Conflict)?;
    if root
        .open_optional_file_relative(Path::new("cloud-library.catdb"))
        .map_err(|_| Error::Conflict)?
        .is_some()
    {
        return Ok(true);
    }
    let entries = root
        .read_dir_relative_bounded(Path::new(""), 4096)
        .map_err(|_| Error::Conflict)?;
    let Some(workspaces) = entries.iter().find(|entry| entry.name == "workspaces") else {
        return Ok(false);
    };
    if workspaces.kind != SourceEntryKind::Directory {
        return Err(Error::Conflict);
    }
    let children = root
        .read_dir_relative_bounded(Path::new("workspaces"), 1000)
        .map_err(|_| Error::Conflict)?;
    for child in children {
        if child.kind != SourceEntryKind::Directory {
            continue;
        }
        let parent = Path::new("workspaces").join(child.name);
        for name in ["catdb.db", "proposals.db"] {
            if root
                .open_optional_file_relative(&parent.join(name))
                .map_err(|_| Error::Conflict)?
                .is_some()
            {
                root.verify().map_err(|_| Error::Conflict)?;
                return Ok(true);
            }
        }
    }
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(false)
}
