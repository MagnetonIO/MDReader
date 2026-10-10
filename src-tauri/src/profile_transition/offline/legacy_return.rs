//! Private durable provenance for returning to a prepared legacy original.
//! Not a lock or a grant: actual application/daemon/SQLite closure and complete
//! original evidence are reacquired/revalidated before any rollback or launch.
use super::{
    legacy_launch::{LaunchRecord, LegacyLaunch},
    legacy_source::LegacySource,
    *,
};
use mdreader_core::{source_root::file_stamp::single_link_stamp, SourceRoot, SourceRootIdentity};
use mdreader_legacy_export::{self as legacy, reviews::ArchiveIdentity};
use mdreader_profile::{RootBinding, RootLocator};
use std::{
    io::{Read, Write},
    os::unix::fs::MetadataExt,
};
const MARKER: &str = ".mdreader-legacy-return-v1.json";
const NEXT: &str = ".mdreader-legacy-return-next-v1.json";
const MAX_BYTES: u64 = 131072;
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    ReturnIssued,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    phase: Phase,
    original: RootBinding,
    candidate: RootBinding,
    archive: ArchiveIdentity,
    launch: LaunchRecord,
}
fn binding(root: &SourceRoot) -> Result<RootBinding> {
    root.verify().map_err(|_| Error::Conflict)?;
    let SourceRootIdentity::Unix { device, inode } = root.identity else {
        return Err(Error::Unsupported);
    };
    Ok(RootBinding {
        windows_file_id: None,
        locator: RootLocator::capture(&root.path)?,
        device,
        inode,
    })
}
fn restore(root: &RootBinding) -> Result<SourceRoot> {
    root.validate()?;
    SourceRoot::restore(
        root.locator.restore()?,
        SourceRootIdentity::Unix {
            device: root.device,
            inode: root.inode,
        },
    )
    .map_err(|_| Error::Conflict)
}
fn private(root: &SourceRoot) -> Result<u32> {
    let metadata = root
        .try_clone_directory_handle()
        .map_err(|_| Error::Conflict)?
        .metadata()
        .map_err(|_| Error::Storage)?;
    if metadata.mode() & 0o077 != 0 {
        return Err(Error::Invalid);
    }
    Ok(metadata.uid())
}
fn read(root: &SourceRoot) -> Result<Option<Record>> {
    let uid = private(root)?;
    if root
        .open_optional_file_relative(Path::new(NEXT))
        .map_err(|_| Error::Conflict)?
        .is_some()
    {
        return Err(Error::ReconciliationRequired);
    }
    let Some(mut file) = root
        .open_optional_file_relative(Path::new(MARKER))
        .map_err(|_| Error::Conflict)?
    else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(|_| Error::Storage)?;
    if metadata.uid() != uid || metadata.mode() & 0o077 != 0 || metadata.len() > MAX_BYTES {
        return Err(Error::Integrity);
    }
    let before = single_link_stamp(&file).map_err(|_| Error::Conflict)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::Bounds);
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
    let named = root
        .open_optional_file_relative(Path::new(MARKER))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    if before != single_link_stamp(&file).map_err(|_| Error::Conflict)?
        || before != single_link_stamp(&named).map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    if record.version != 1 || record.candidate != binding(root)? {
        return Err(Error::Conflict);
    }
    let profile = legacy::migration::verify(&root.path, &Default::default())
        .map_err(super::archive_import::failure)?;
    if profile.version != 2 || profile.archive != record.archive {
        return Err(Error::Conflict);
    }
    Ok(Some(record))
}
fn publish(root: &SourceRoot, name: &str, record: &Record) -> Result<()> {
    private(root)?;
    let bytes = serde_json::to_vec(record).map_err(|_| Error::Storage)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::Bounds);
    }
    let mut file = root
        .create_new_relative(Path::new(name))
        .map_err(|_| Error::Storage)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::Storage)?;
    root.try_clone_directory_handle()
        .map_err(|_| Error::Storage)?
        .sync_all()
        .map_err(|_| Error::Storage)?;
    root.verify().map_err(|_| Error::Conflict)
}
/// Caller has retained/verified original application exit and real owner locks.
pub(super) fn write(
    original: &SourceRoot,
    launch: &LegacyLaunch,
    candidate: &Path,
    archive: &Path,
) -> Result<()> {
    let candidate = SourceRoot::capture(candidate.to_owned()).map_err(|_| Error::Conflict)?;
    let identity = legacy::migration::archive::identity(archive, &Default::default())
        .map_err(super::archive_import::failure)?;
    let copied = legacy::migration::verify(&candidate.path, &Default::default())
        .map_err(super::archive_import::failure)?;
    if copied.archive != identity {
        return Err(Error::Conflict);
    }
    let record = Record {
        version: 1,
        phase: Phase::Prepared,
        original: binding(original)?,
        candidate: binding(&candidate)?,
        archive: identity,
        launch: launch.record()?,
    };
    if let Some(old) = read(&candidate)? {
        if serde_json::to_vec(&old).map_err(|_| Error::Integrity)?
            != serde_json::to_vec(&record).map_err(|_| Error::Integrity)?
        {
            return Err(Error::Conflict);
        }
        return Ok(());
    }
    publish(&candidate, MARKER, &record)
}
/// Rehydrate identities, not owner guards. Those are acquired on explicit return.
fn provenance(candidate: &SourceRoot) -> Result<Option<(SourceRoot, LegacyLaunch)>> {
    let Some(record) = read(candidate)? else {
        return Ok(None);
    };
    if record.phase != Phase::Prepared {
        return Err(Error::ReconciliationRequired);
    }
    let root = restore(&record.original)?;
    let launch = LegacyLaunch::restore(record.launch)?;
    super::legacy_snapshot::verify_originals(&root, &candidate.path.join("legacy-archive"))?;
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(Some((root, launch)))
}
pub(super) fn resume(candidate: &Path) -> Result<Option<LegacySource>> {
    let candidate = SourceRoot::capture(candidate.to_owned()).map_err(|_| Error::Conflict)?;
    provenance(&candidate)?
        .map(|(root, launch)| LegacySource::restore(root, launch))
        .transpose()
}
/// Durable no-relaunch fence before clearing selection/launch dispatch. Unknown
/// results across a process restart must not cause a second original launch.
pub(super) fn return_issued(candidate: &Path) -> Result<()> {
    let candidate = SourceRoot::capture(candidate.to_owned()).map_err(|_| Error::Conflict)?;
    let Some(mut record) = read(&candidate)? else {
        return Err(Error::Unsupported);
    };
    if record.phase != Phase::Prepared {
        return Err(Error::ReconciliationRequired);
    }
    record.phase = Phase::ReturnIssued;
    publish(&candidate, NEXT, &record)?;
    candidate
        .rename_relative(Path::new(NEXT), Path::new(MARKER))
        .map_err(|_| Error::Storage)?;
    candidate
        .try_clone_directory_handle()
        .map_err(|_| Error::Storage)?
        .sync_all()
        .map_err(|_| Error::Storage)?;
    candidate.verify().map_err(|_| Error::Conflict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdreader_core::{device, ReaderStore};
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn prepared_return_survives_restart_and_refuses_changed_original_binary_or_candidate() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = parent.join("original");
        std::fs::create_dir(&original).unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = SourceRoot::capture(original).unwrap();
        original
            .create_new_relative(Path::new(".mdreader-json-owner.lock"))
            .unwrap();
        let mut owner = ReaderStore::open(&original.path.join("reader-v1")).unwrap();
        let saved = device::import(
            &mut owner,
            &original.path.join("reader-v1/device-markdown-v1"),
            "retained.md".into(),
            "# Retained original\nUnchanged bytes.\n".into(),
        )
        .unwrap();
        // A truly quiescent fixture retains committed WAL pages and SHM without
        // a live SQLite connection. Recovery may touch only private copies.
        owner.with_library_connection(|db| db.execute_batch(
            "PRAGMA wal_autocheckpoint=0; CREATE TABLE retained_wal_row(value TEXT); INSERT INTO retained_wal_row VALUES('WAL-only');"
        )).unwrap();
        let frozen = parent.join("frozen-db");
        std::fs::create_dir(&frozen).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            std::fs::copy(
                original
                    .path
                    .join(format!("reader-v1/library.sqlite{suffix}")),
                frozen.join(format!("library.sqlite{suffix}")),
            )
            .unwrap();
        }
        drop(owner);
        for suffix in ["", "-wal", "-shm"] {
            std::fs::copy(
                frozen.join(format!("library.sqlite{suffix}")),
                original
                    .path
                    .join(format!("reader-v1/library.sqlite{suffix}")),
            )
            .unwrap();
        }
        let mut closed = super::super::owner::ClosedOwner::acquire(&original.path).unwrap();
        let archive =
            super::super::legacy_snapshot::capture(&original, || closed.revalidate()).unwrap();
        let mut import = super::super::archive_import::ArchiveImport::preview(&archive).unwrap();
        import
            .prepare(
                &parent,
                &[Selection {
                    document_id: saved.id.clone(),
                    revision: saved.revision.clone(),
                }],
            )
            .unwrap();
        let candidate = import.selected_profile().unwrap().to_owned();
        let bundle = parent.join("Original.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            b"original synthetic bundle",
        )
        .unwrap();
        let binary = bundle.join("Contents/MacOS/Original");
        std::fs::write(&binary, b"original executable bytes").unwrap();
        let launch = LegacyLaunch::capture(bundle, binary.clone()).unwrap();
        super::super::legacy_snapshot::verify_originals(&original, &archive).unwrap();
        write(&original, &launch, &candidate, &archive).unwrap();
        drop(import);
        drop(launch);
        drop(closed);
        // This used to fail: closing the readonly original WAL connection
        // changed SHM ctime even with no original writer or body changes.
        super::super::legacy_snapshot::verify_originals(&original, &archive).unwrap();
        // Actual prepared candidate and private provenance reload with no old
        // in-memory Pending or application object. This restores identities only.
        let resumed = super::super::archive_import::ArchiveImport::resume(&candidate).unwrap();
        let selected = SourceRoot::capture(resumed.selected_profile().unwrap().to_owned()).unwrap();
        let (restored, launch) = provenance(&selected).unwrap().unwrap();
        assert_eq!(restored, original);
        launch.verify().unwrap();
        let device_path = original
            .path
            .join("reader-v1/device-markdown-v1")
            .join(&saved.id)
            .join("document.md");
        let bytes = std::fs::read(&device_path).unwrap();
        std::fs::write(&device_path, b"changed original body").unwrap();
        assert!(matches!(provenance(&selected), Err(Error::Conflict)));
        // Restoring bytes cannot restore the retained file stamps, so preserve
        // that original untouched for the remaining independent checks.
        std::fs::write(&device_path, &bytes).unwrap();
        let retained = read(&selected).unwrap().unwrap();
        std::fs::write(&binary, b"replacement executable").unwrap();
        assert!(matches!(
            LegacyLaunch::restore(retained.launch),
            Err(Error::Conflict)
        ));
        let moved = parent.join("moved-candidate");
        std::fs::rename(&candidate, &moved).unwrap();
        std::fs::create_dir(&candidate).unwrap();
        assert!(read(&selected).is_err());
        assert!(moved.join(MARKER).exists());
    }
}
