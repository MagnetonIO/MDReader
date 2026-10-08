use super::*;
use crate::ReaderStore;
use std::fs;

#[test]
fn writer_readonly_and_reopen_keep_original_object_and_reject_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let owner = ReaderStore::open(directory.path()).unwrap();
    let identity = owner.library_identity().unwrap();
    identity.verify_at(directory.path()).unwrap();
    owner
        .store_identity()
        .unwrap()
        .verify_at(directory.path())
        .unwrap();
    // The actual index connection must retain the original authority handle,
    // not merely a valid main index filename/library certificate.
    owner
        .index
        .execute_batch(
            "DETACH DATABASE reader_authority; ATTACH DATABASE ':memory:' AS reader_authority",
        )
        .unwrap();
    assert!(matches!(owner.store_identity(), Err(ReaderError::Conflict)));
    owner
        .index
        .execute_batch("DETACH DATABASE reader_authority")
        .unwrap();
    super::super::projection::attach_authority(
        &owner.index,
        &directory.path().join("library.sqlite"),
    )
    .unwrap();
    owner.store_identity().unwrap();
    let readonly = ReaderStore::open_readonly(directory.path()).unwrap();
    readonly.library_identity().unwrap().verify().unwrap();
    readonly
        .store_identity()
        .unwrap()
        .verify_at(directory.path())
        .unwrap();
    let closed = readonly.library_identity().unwrap();
    drop(readonly);
    assert!(matches!(closed.verify(), Err(ReaderError::Unavailable)));
    let other = tempfile::tempdir().unwrap();
    assert!(matches!(
        identity.verify_at(other.path()),
        Err(ReaderError::Conflict)
    ));
    drop(owner);
    assert!(matches!(identity.verify(), Err(ReaderError::Unavailable)));
    let reopened = ReaderStore::open(directory.path()).unwrap();
    reopened.library_identity().unwrap().verify().unwrap();
    let identity = reopened.library_identity().unwrap();

    let named = directory.path().join("library.sqlite");
    fs::rename(&named, directory.path().join("original.sqlite")).unwrap();
    fs::File::create(&named).unwrap();
    assert!(matches!(identity.verify(), Err(ReaderError::Conflict)));
    assert!(matches!(
        reopened.library_identity(),
        Err(ReaderError::Conflict)
    ));
    // Existing aliases must fail before the SQLite open/migrate callback runs.
    fs::remove_file(&named).unwrap();
    std::os::unix::fs::symlink("original.sqlite", &named).unwrap();
    assert!(matches!(
        open_library(directory.path(), true, |_| {
            panic!("symlink must be rejected before opening SQLite")
        }),
        Err(ReaderError::Conflict)
    ));
    fs::remove_file(&named).unwrap();
    fs::hard_link(directory.path().join("original.sqlite"), &named).unwrap();
    assert!(matches!(
        open_library(directory.path(), true, |_| {
            panic!("hardlink must be rejected before opening SQLite")
        }),
        Err(ReaderError::Conflict)
    ));
    // The retained original is not overwritten or reopened as the replacement.
    assert!(
        directory
            .path()
            .join("original.sqlite")
            .metadata()
            .unwrap()
            .len()
            > 0
    );
}

#[test]
fn sqlite_open_time_aba_is_rejected_even_when_original_path_is_restored() {
    let directory = tempfile::tempdir().unwrap();
    let named = directory.path().join("library.sqlite");
    fs::File::create(&named).unwrap();
    let original = directory.path().join("original.sqlite");
    let alternate = directory.path().join("alternate.sqlite");
    let result = open_library(directory.path(), false, |path| {
        // The retained pre-open object is A; SQLite opens B. Restoring A before
        // the post-open stat defeats a path/stat-only proof.
        fs::rename(path, &original).unwrap();
        let connection = Connection::open(path)?;
        connection.execute_batch("CREATE TABLE alternate(value INTEGER)")?;
        fs::rename(path, &alternate).unwrap();
        fs::rename(&original, path).unwrap();
        Ok(connection)
    });
    assert!(matches!(result, Err(ReaderError::Conflict)));
    assert_eq!(fs::metadata(named).unwrap().len(), 0);
    assert!(fs::metadata(alternate).unwrap().len() > 0);
}

#[test]
fn certification_does_not_release_process_sqlite_locks() {
    const CHILD_PROFILE: &str = "MDREADER_IDENTITY_LOCK_CHILD";
    if let Some(directory) = std::env::var_os(CHILD_PROFILE) {
        let connection = Connection::open(Path::new(&directory).join("library.sqlite")).unwrap();
        connection.busy_timeout(std::time::Duration::ZERO).unwrap();
        let result = connection.execute_batch("BEGIN IMMEDIATE");
        assert!(
            matches!(result, Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseBusy)
        );
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    // Test the exact core certification helper with rollback journaling so
    // locks belong to the main inode. ReaderStore's attached readonly authority
    // connection prevents changing its default WAL mode to DELETE in-place.
    let (mut connection, live) = open_library(directory.path(), true, |path| {
        let connection = Connection::open(path)?;
        connection.execute_batch("CREATE TABLE lock_fixture(value INTEGER)")?;
        Ok(connection)
    })
    .unwrap();
    let live = live.unwrap();
    let identity = certificate(&live);
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    for _ in 0..3 {
        identity.verify_at(directory.path()).unwrap();
    }
    let readonly = Connection::open_with_flags(
        directory.path().join("library.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    verify_connection(&readonly, Some(&live)).unwrap();
    drop(readonly);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::identity::tests::certification_does_not_release_process_sqlite_locks",
        ])
        .env(CHILD_PROFILE, directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    transaction.rollback().unwrap();
}
