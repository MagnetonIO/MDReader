//! Public synthetic Windows service with its actual writer and retained TCP lease.
use super::*;
use mdreader_profile::{database_backup, FreezeOwner, FrozenProfile, RootBinding};
use std::path::{Path, PathBuf};

struct BorrowedService {
    binding: RootBinding,
    attempt: Attempt,
}
fn held(status: Status) -> mdreader_profile::Result<()> {
    if status.phase == Phase::Held && status.lease.is_some() && status.remaining_ms > 0 {
        Ok(())
    } else {
        Err(mdreader_profile::Error::NotQuiescent)
    }
}
impl FreezeOwner for BorrowedService {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> mdreader_profile::Result<T>,
    ) -> mdreader_profile::Result<T> {
        if expected != &self.binding {
            return Err(mdreader_profile::Error::Conflict);
        }
        held(
            self.attempt
                .begin()
                .map_err(|_| mdreader_profile::Error::NotQuiescent)?,
        )?;
        self.revalidate()?;
        action(self)
    }
}
impl FrozenProfile for BorrowedService {
    fn binding(&self) -> &RootBinding {
        &self.binding
    }
    fn revalidate(&mut self) -> mdreader_profile::Result<()> {
        held(
            self.attempt
                .revalidate()
                .map_err(|_| mdreader_profile::Error::Conflict)?,
        )?;
        if RootBinding::capture_private(&self.binding.locator.restore()?)? != self.binding {
            return Err(mdreader_profile::Error::Conflict);
        }
        Ok(())
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> mdreader_profile::Result<()>,
    ) -> mdreader_profile::Result<()> {
        Err(mdreader_profile::Error::Unsupported)
    }
    fn backup_database(
        &mut self,
        target: &mut database_backup::DatabaseBackupTarget,
    ) -> mdreader_profile::Result<database_backup::BackupReceipt> {
        self.revalidate()?;
        let status = self
            .attempt
            .backup(target.request())
            .map_err(|_| mdreader_profile::Error::Conflict)?;
        let BackupStatus::Completed { receipt } = status else {
            return Err(mdreader_profile::Error::ReconciliationRequired);
        };
        target.accept(&receipt)?;
        self.revalidate()?;
        Ok(*receipt)
    }
}
fn fixture_inventory(root: &Path) -> mdreader_profile::Inventory {
    use mdreader_profile::{Disposition, Exclusion, InventorySelection};
    let mut pending = vec![PathBuf::new()];
    let mut selected = vec![];
    while let Some(relative) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&relative)).unwrap() {
            let entry = entry.unwrap();
            let name = relative.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                pending.push(name);
            } else {
                selected.push(InventorySelection {
                    disposition: if name == Path::new("library.sqlite") {
                        Disposition::Sqlite
                    } else {
                        Disposition::Excluded(Exclusion::RuntimeEndpointOrLock)
                    },
                    relative: name,
                });
            }
            assert!(pending.len() + selected.len() <= 4096);
        }
    }
    mdreader_profile::Inventory::capture(root, &selected, &Default::default()).unwrap()
}
#[test]
fn windows_remote_backup_borrows_exact_frozen_service_owner_and_reopens_private_snapshot() {
    let server = Server::new();
    server._owner.lock().unwrap().with_library_connection(|db| {
        db.execute_batch("CREATE TABLE public_preservation_fixture(body BLOB NOT NULL); INSERT INTO public_preservation_fixture VALUES(x'0001ff');")
    }).unwrap();
    let client = server.client();
    let mut owner = BorrowedService {
        binding: RootBinding::capture_private(&server.path).unwrap(),
        attempt: client
            .attempt(Uuid::new_v4(), Uuid::new_v4(), 30_000)
            .unwrap(),
    };
    held(owner.attempt.begin().unwrap()).unwrap();
    let inventory = fixture_inventory(&server.path);
    let output = tempfile::tempdir().unwrap();
    let private = output.path().join("private backup output");
    mdreader_capture::PrivateDirectory::create(&private).unwrap();
    let destination = private.join("copy");
    let copied =
        mdreader_profile::stage(&mut owner, inventory, &destination, &Default::default()).unwrap();
    let entry = copied
        .manifest()
        .entries
        .iter()
        .find(|entry| entry.relative.restore().unwrap() == Path::new("library.sqlite"))
        .unwrap();
    let database = rusqlite::Connection::open_with_flags(
        destination.join(entry.member.as_ref().unwrap()),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let body: Vec<u8> = database
        .query_row("SELECT body FROM public_preservation_fixture", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(body, [0, 1, 255]);
    assert!(server.runtime.execute(Request::ListSources).is_err());
    assert_eq!(owner.attempt.release().unwrap().phase, Phase::Released);
    assert!(server.runtime.execute(Request::ListSources).is_ok());
}
