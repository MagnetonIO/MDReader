use crate::{platform::Root, Disposition, Inventory, InventorySelection, Limits};
use rusqlite::Connection;
use std::{fs, path::Path, process::Command, time::Duration};

const CHILD: &str = "MDREADER_PROFILE_LOCK_CHILD";

pub(super) fn competing_writer() -> bool {
    let Some(path) = std::env::var_os(CHILD) else {
        return false;
    };
    let connection = Connection::open(path).unwrap();
    connection.busy_timeout(Duration::ZERO).unwrap();
    assert!(matches!(connection.execute_batch("BEGIN EXCLUSIVE"),
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseBusy));
    true
}

pub(super) fn assert_locked(test: &str, path: &Path) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test])
        .env(CHILD, path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn database(root: &Root) -> Connection {
    let connection = Connection::open(root.path.join("library.db")).unwrap();
    connection.execute_batch("CREATE TABLE durable(value INTEGER); INSERT INTO durable VALUES(1); BEGIN; SELECT * FROM durable;").unwrap();
    connection
}

#[test]
fn inventory_preserves_live_database_locks_through_drop() {
    if competing_writer() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = Root::capture(&temp.path().canonicalize().unwrap()).unwrap();
    let connection = database(&root);
    let inventory = Inventory::capture(
        &root.path,
        &[InventorySelection {
            relative: "library.db".into(),
            disposition: Disposition::Sqlite,
        }],
        &Limits::default(),
    )
    .unwrap();
    inventory.revalidate().unwrap();
    drop(inventory);
    assert_locked(
        "live_database_tests::inventory_preserves_live_database_locks_through_drop",
        &root.path.join("library.db"),
    );
    connection.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn snapshot_validation_preserves_live_database_locks() {
    if competing_writer() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = Root::capture(&temp.path().canonicalize().unwrap()).unwrap();
    let connection = database(&root);
    let expected =
        crate::platform::stamp(&fs::File::open(root.path.join("library.db")).unwrap()).unwrap();
    // Establish a new read lock after the fixture's stamp descriptor is closed.
    connection
        .execute_batch("ROLLBACK; BEGIN; SELECT * FROM durable;")
        .unwrap();
    super::snapshot::backup::check_database(&connection, &root, Path::new("library.db"), &expected)
        .unwrap();
    assert_locked(
        "live_database_tests::snapshot_validation_preserves_live_database_locks",
        &root.path.join("library.db"),
    );
    connection.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn snapshot_validation_rejects_a_replaced_live_handle() {
    let temp = tempfile::tempdir().unwrap();
    let root = Root::capture(&temp.path().canonicalize().unwrap()).unwrap();
    let connection = database(&root);
    let named = root.path.join("library.db");
    fs::rename(&named, root.path.join("old.db")).unwrap();
    fs::File::create(&named).unwrap();
    let expected = crate::platform::stamp(&fs::File::open(&named).unwrap()).unwrap();
    assert!(
        matches!(
            super::snapshot::backup::check_database(
                &connection,
                &root,
                Path::new("library.db"),
                &expected
            ),
            Err(crate::Error::Conflict)
        ),
        "a new file at the old address cannot attest the retained SQLite handle"
    );
}
