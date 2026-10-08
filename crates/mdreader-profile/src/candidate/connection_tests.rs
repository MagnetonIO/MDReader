use super::*;
use std::{fs, os::unix::fs::PermissionsExt, process::Command, time::Duration};

fn profile(path: &Path) -> crate::platform::Root {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    crate::platform::Root::private(&path.canonicalize().unwrap()).unwrap()
}

#[test]
fn candidate_verification_preserves_process_sqlite_locks() {
    const CHILD: &str = "MDREADER_CANDIDATE_LOCK_CHILD";
    if let Some(path) = std::env::var_os(CHILD) {
        let connection = Connection::open(Path::new(&path).join("library.sqlite")).unwrap();
        connection.busy_timeout(Duration::ZERO).unwrap();
        assert!(matches!(connection.execute_batch("BEGIN IMMEDIATE"),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::DatabaseBusy));
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = profile(temp.path());
    let mut connection = Connection::open(root.path.join("library.sqlite")).unwrap();
    connection
        .execute_batch("CREATE TABLE retained(value INTEGER)")
        .unwrap();
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    for _ in 0..3 {
        check_connection(&tx, &root.binding).unwrap();
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "candidate::connection_tests::candidate_verification_preserves_process_sqlite_locks",
        ])
        .env(CHILD, &root.path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    tx.rollback().unwrap();
}

#[test]
fn candidate_verification_rejects_replaced_database_and_root() {
    let temp = tempfile::tempdir().unwrap();
    let candidate = temp.path().join("candidate");
    fs::create_dir(&candidate).unwrap();
    let root = profile(&candidate);
    let named = root.path.join("library.sqlite");
    let connection = Connection::open(&named).unwrap();
    connection
        .execute_batch("CREATE TABLE retained(value INTEGER)")
        .unwrap();
    check_connection(&connection, &root.binding).unwrap();
    fs::rename(&named, root.path.join("retained.sqlite")).unwrap();
    fs::File::create(&named).unwrap();
    assert!(
        matches!(
            check_connection(&connection, &root.binding),
            Err(Error::Conflict)
        ),
        "named replacement must not attest the old SQLite handle"
    );

    drop(connection);
    fs::rename(&candidate, temp.path().join("old-root")).unwrap();
    fs::create_dir(&candidate).unwrap();
    let replacement = profile(&candidate);
    let connection = Connection::open(replacement.path.join("library.sqlite")).unwrap();
    assert!(
        matches!(
            check_connection(&connection, &root.binding),
            Err(Error::Conflict)
        ),
        "a new owner at the same address cannot adopt the prepared root binding"
    );
    // Only the exact candidate library main handle is eligible, not its index.
    let index = Connection::open(replacement.path.join("index.sqlite")).unwrap();
    assert!(matches!(
        check_connection(&index, &replacement.binding),
        Err(Error::Conflict)
    ));
}
