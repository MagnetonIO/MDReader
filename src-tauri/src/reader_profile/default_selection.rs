//! Durable native default selection, separate from every original profile.
//! A missing selection can bootstrap; a damaged selected root must fail closed.
use mdreader_core::{SourceRoot, SourceRootIdentity};
use mdreader_legacy_export::{self as legacy, reviews::ArchiveIdentity};
use mdreader_profile::{Error, Result, RootBinding, RootLocator};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    root: RootBinding,
    archive: ArchiveIdentity,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    version: u32,
    generation: u64,
    target: Option<Target>,
}
pub(super) struct DefaultSelection {
    control: SourceRoot,
    expected: Mutex<Selection>,
}
fn binding(root: &SourceRoot) -> Result<RootBinding> {
    binding_from(&root.path, &root.identity)
}
fn binding_from(path: &Path, identity: &SourceRootIdentity) -> Result<RootBinding> {
    let SourceRootIdentity::Unix { device, inode } = *identity else {
        return Err(Error::Unsupported);
    };
    Ok(RootBinding {
        windows_file_id: None,
        locator: RootLocator::capture(path)?,
        device,
        inode,
    })
}
fn restore(target: &Target) -> Result<SourceRoot> {
    let root = SourceRoot::restore(
        target.root.locator.restore()?,
        SourceRootIdentity::Unix {
            device: target.root.device,
            inode: target.root.inode,
        },
    )
    .map_err(|_| Error::Conflict)?;
    let profile =
        legacy::migration::verify(&root.path, &Default::default()).map_err(|_| Error::Integrity)?;
    if profile.version != 2 || profile.archive != target.archive {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(root)
}
fn read(control: &SourceRoot) -> Result<Selection> {
    control.verify().map_err(|_| Error::Conflict)?;
    let mut file = control
        .open_optional_file_relative(Path::new("selection.json"))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Integrity)?;
    let stamp = mdreader_core::source_root::file_stamp::single_link_stamp(&file)
        .map_err(|_| Error::Conflict)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > 65536 {
        return Err(Error::Bounds);
    }
    let selection: Selection = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
    if selection.version != 1 || selection.generation == 0 {
        return Err(Error::Unsupported);
    }
    let named = control
        .open_optional_file_relative(Path::new("selection.json"))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    if stamp
        != mdreader_core::source_root::file_stamp::single_link_stamp(&file)
            .map_err(|_| Error::Conflict)?
        || stamp
            != mdreader_core::source_root::file_stamp::single_link_stamp(&named)
                .map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    control.verify().map_err(|_| Error::Conflict)?;
    Ok(selection)
}
impl DefaultSelection {
    pub(super) fn open(standard: &Path) -> Result<Self> {
        let parent = standard.parent().ok_or(Error::Invalid)?;
        super::create_profile_directory(parent).map_err(|_| Error::Storage)?;
        let parent = SourceRoot::capture(parent.canonicalize().map_err(|_| Error::Storage)?)
            .map_err(|_| Error::Conflict)?;
        let name = standard
            .file_name()
            .ok_or(Error::Invalid)?
            .to_str()
            .ok_or(Error::Unsupported)?;
        let control = parent
            .create_or_capture_directory_relative(Path::new(&format!("{name}.reader-selection-v1")))
            .map_err(|_| Error::Conflict)?;
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&control.path)
            .map_err(|_| Error::Storage)?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(Error::Invalid);
        }
        let lock = control
            .open_lock_relative(Path::new("selection.lock"))
            .map_err(|_| Error::Conflict)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::NotQuiescent)?;
        let marker = Path::new("initialized-v1");
        if let Some(file) = control
            .open_optional_file_relative(marker)
            .map_err(|_| Error::Conflict)?
        {
            let mut bytes = Vec::new();
            file.take(65)
                .read_to_end(&mut bytes)
                .map_err(|_| Error::Storage)?;
            if bytes != b"mdreader-default-selection-v1" {
                return Err(Error::Integrity);
            }
        } else {
            if control
                .read_dir_relative_bounded(Path::new(""), 8)
                .map_err(|_| Error::Conflict)?
                .iter()
                .any(|entry| entry.name != "selection.lock")
            {
                return Err(Error::Integrity);
            }
            let mut marker = control
                .create_new_relative(marker)
                .map_err(|_| Error::Storage)?;
            marker
                .write_all(b"mdreader-default-selection-v1")
                .and_then(|()| marker.sync_all())
                .map_err(|_| Error::Storage)?;
            let initial = Selection {
                version: 1,
                generation: 1,
                target: None,
            };
            let mut file = control
                .create_new_relative(Path::new("selection.json"))
                .map_err(|_| Error::Storage)?;
            file.write_all(&serde_json::to_vec(&initial).map_err(|_| Error::Storage)?)
                .and_then(|()| file.sync_all())
                .map_err(|_| Error::Storage)?;
            control
                .try_clone_directory_handle()
                .map_err(|_| Error::Storage)?
                .sync_all()
                .map_err(|_| Error::Storage)?;
        }
        let expected = read(&control)?;
        drop(lock);
        Ok(Self {
            control,
            expected: Mutex::new(expected),
        })
    }
    pub(super) fn selected(&self) -> Result<Option<SourceRoot>> {
        let target = {
            let expected = self
                .expected
                .lock()
                .map_err(|_| Error::ReconciliationRequired)?;
            if read(&self.control)? != *expected {
                return Err(Error::Conflict);
            }
            expected.target.clone()
        };
        let Some(target) = target else {
            return Ok(None);
        };
        let root = restore(&target)?;
        // The selection is path-based: persist the device/inode the path names
        // now (e.g. after macOS renumbered the volume) through the normal
        // publish path. A failed write leaves the path-based selection usable.
        let observed = binding_from(
            &root.path,
            &root.observed_identity().map_err(|_| Error::Conflict)?,
        )?;
        if observed != target.root {
            let _ = self.publish(
                Some(Target {
                    root: observed,
                    archive: target.archive.clone(),
                }),
                None,
            );
        }
        Ok(Some(root))
    }
    pub(super) fn bootstrap(&self) -> Result<SourceRoot> {
        self.control
            .create_or_capture_directory_relative(Path::new("bootstrap"))
            .map_err(|_| Error::Conflict)
    }
    pub(super) fn return_controller(&self) -> Result<SourceRoot> {
        // Bound private controller roots; never reuse a live GUI's JSON owner.
        self.control
            .read_dir_relative_bounded(Path::new(""), 64)
            .map_err(|_| Error::Bounds)?;
        self.control
            .create_or_capture_directory_relative(Path::new(&format!(
                "return-{}",
                uuid::Uuid::new_v4()
            )))
            .map_err(|_| Error::Conflict)
    }
    pub(super) fn select(&self, path: &Path) -> Result<()> {
        let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
        let profile =
            legacy::migration::verify(path, &Default::default()).map_err(|_| Error::Integrity)?;
        if profile.version != 2 {
            return Err(Error::Unsupported);
        }
        let target = Target {
            root: binding(&root)?,
            archive: profile.archive,
        };
        restore(&target)?;
        self.publish(Some(target), None)
    }
    pub(super) fn clear(&self, path: &Path) -> Result<()> {
        let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
        self.publish(None, Some(binding(&root)?))
    }
    fn publish(&self, target: Option<Target>, clearing: Option<RootBinding>) -> Result<()> {
        let mut expected = self
            .expected
            .lock()
            .map_err(|_| Error::ReconciliationRequired)?;
        let lock = self
            .control
            .open_lock_relative(Path::new("selection.lock"))
            .map_err(|_| Error::Conflict)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::NotQuiescent)?;
        if read(&self.control)? != *expected {
            return Err(Error::Conflict);
        }
        // Path-based: clearing names the selected folder by its location.
        if clearing.as_ref().is_some_and(|root| {
            expected.target.as_ref().is_none_or(|target| {
                target.root.locator.restore().ok() != root.locator.restore().ok()
            })
        }) {
            return Err(Error::Conflict);
        }
        if let Some(target) = &target {
            restore(target)?;
        }
        let next = Selection {
            version: 1,
            generation: expected.generation.checked_add(1).ok_or(Error::Bounds)?,
            target,
        };
        let name = PathBuf::from(format!("selection-{}.json", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec(&next).map_err(|_| Error::Storage)?;
        let mut file = self
            .control
            .create_new_relative(&name)
            .map_err(|_| Error::Storage)?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Storage)?;
        self.control.verify().map_err(|_| Error::Conflict)?;
        if read(&self.control)? != *expected {
            return Err(Error::Conflict);
        }
        self.control
            .rename_relative(&name, Path::new("selection.json"))
            .map_err(|_| Error::Storage)?;
        self.control
            .try_clone_directory_handle()
            .map_err(|_| Error::Storage)?
            .sync_all()
            .map_err(|_| Error::Storage)?;
        *expected = next;
        if read(&self.control)? != *expected {
            return Err(Error::ReconciliationRequired);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_renumbered_device_restores_the_selection_and_persists_the_observed_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let standard = parent.join("standard-original");
        let slot = DefaultSelection::open(&standard).unwrap();
        let settings = parent.join("old-settings.json");
        std::fs::write(&settings, br#"{"settings":{"theme":"dark"}}"#).unwrap();
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o600)).unwrap();
        let archive = parent.join("archive");
        legacy::export(
            &[legacy::Input {
                role: legacy::Role::Settings,
                path: settings,
            }],
            &archive,
            &Default::default(),
        )
        .unwrap();
        let identity = legacy::migration::archive::identity(&archive, &Default::default()).unwrap();
        let candidate = parent.join("candidate");
        legacy::migration::archive::prepare(
            &archive,
            &identity,
            &candidate,
            &Default::default(),
            |_, _, root| {
                mdreader_core::ReaderStore::open(&root.join("reader-v1"))
                    .map_err(|_| legacy::Error::Integrity)?;
                Ok(0)
            },
        )
        .unwrap();
        slot.select(&candidate).unwrap();
        drop(slot);
        // Rewrite selection.json as if macOS renumbered st_dev after a reboot.
        let control = parent.join("standard-original.reader-selection-v1");
        let file = control.join("selection.json");
        let mut selection: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let device = selection["target"]["root"]["device"].as_u64().unwrap();
        let inode = selection["target"]["root"]["inode"].as_u64().unwrap();
        selection["target"]["root"]["device"] = (device + 1).into();
        selection["target"]["root"]["inode"] = (inode + 5).into();
        std::fs::write(&file, serde_json::to_vec(&selection).unwrap()).unwrap();

        let reopened = DefaultSelection::open(&standard).unwrap();
        assert_eq!(reopened.selected().unwrap().unwrap().path, candidate);
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(persisted["target"]["root"]["device"].as_u64(), Some(device));
        assert_eq!(persisted["target"]["root"]["inode"].as_u64(), Some(inode));
        // And it survives another reopen unchanged.
        let again = DefaultSelection::open(&standard).unwrap();
        assert_eq!(again.selected().unwrap().unwrap().path, candidate);
    }

    #[test]
    fn accepted_default_survives_reopen_and_refuses_stale_replaced_or_missing_selection() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let standard = parent.join("standard-original");
        let slot = DefaultSelection::open(&standard).unwrap();
        let stale = DefaultSelection::open(&standard).unwrap();
        assert!(slot.selected().unwrap().is_none());
        assert!(!standard.exists());
        assert_ne!(slot.bootstrap().unwrap().path, standard);
        let settings = parent.join("old-settings.json");
        std::fs::write(&settings, br#"{"settings":{"theme":"dark"}}"#).unwrap();
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o600)).unwrap();
        let original = std::fs::read(&settings).unwrap();
        let archive = parent.join("archive");
        legacy::export(
            &[legacy::Input {
                role: legacy::Role::Settings,
                path: settings.clone(),
            }],
            &archive,
            &Default::default(),
        )
        .unwrap();
        let identity = legacy::migration::archive::identity(&archive, &Default::default()).unwrap();
        let candidate = parent.join("candidate");
        legacy::migration::archive::prepare(
            &archive,
            &identity,
            &candidate,
            &Default::default(),
            |_, _, root| {
                mdreader_core::ReaderStore::open(&root.join("reader-v1"))
                    .map_err(|_| legacy::Error::Integrity)?;
                Ok(0)
            },
        )
        .unwrap();
        slot.select(&candidate).unwrap();
        assert_eq!(slot.selected().unwrap().unwrap().path, candidate);
        assert!(matches!(stale.publish(None, None), Err(Error::Conflict)));
        let reopened = DefaultSelection::open(&standard).unwrap();
        assert_eq!(reopened.selected().unwrap().unwrap().path, candidate);
        let moved = parent.join("moved-candidate");
        std::fs::rename(&candidate, &moved).unwrap();
        std::fs::create_dir(&candidate).unwrap();
        assert!(reopened.selected().is_err());
        // Startup degrades to the standard library instead of failing.
        let mut info = super::super::ProfileStartup {
            migration_required: false,
            using_imported_library: false,
            return_controller: false,
            selected_library_unavailable: false,
        };
        assert!(super::super::or_standard(reopened.selected(), &mut info).is_none());
        assert!(info.selected_library_unavailable);
        std::fs::remove_dir(&candidate).unwrap();
        std::fs::rename(&moved, &candidate).unwrap();
        reopened.clear(&candidate).unwrap();
        assert!(DefaultSelection::open(&standard)
            .unwrap()
            .selected()
            .unwrap()
            .is_none());
        std::fs::remove_file(reopened.control.path.join("selection.json")).unwrap();
        assert!(DefaultSelection::open(&standard).is_err());
        assert!(!standard.exists());
        assert_eq!(std::fs::read(settings).unwrap(), original);
    }
}
