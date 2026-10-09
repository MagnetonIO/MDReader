//! Actual closed Windows writer locks and original WAL/JSON bytes, not a
//! fabricated frozen owner. Only the isolated crash-child owns SQLite writes.
use super::*;
use crate::profile_transition::persistence::{JsonKind, JsonOwner};
use mdreader_capture::{observe_object, PrivateDirectory};
use std::{io::Read, process::Command};

fn evidence(
    root: &SourceRoot,
    names: &[&str],
) -> Vec<(String, mdreader_capture::WindowsObjectStamp, Vec<u8>)> {
    names
        .iter()
        .map(|name| {
            let mut file = root
                .open_optional_file_relative(Path::new(name))
                .unwrap()
                .unwrap();
            let stamp = observe_object(&file, false, true).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            (name.to_string(), stamp, bytes)
        })
        .collect()
}

#[test]
fn closed_windows_reader_exports_wal_and_canonical_json_without_touching_originals() {
    const CHILD: &str = "MDREADER_TEST_CLOSED_WINDOWS_WAL";
    if let Some(path) = std::env::var_os(CHILD) {
        let db = Connection::open(PathBuf::from(path).join("reader-v1/library.sqlite")).unwrap();
        db.pragma_update(None, "journal_mode", "WAL").unwrap();
        db.execute_batch("CREATE TABLE preserved_test_body(body BLOB NOT NULL); INSERT INTO preserved_test_body VALUES(X'efbbbf23204f726967696e616c0d0a');").unwrap();
        // Intentionally bypass Connection::drop: leaves a real committed WAL
        // and original SHM after the actual child writer process has exited.
        std::process::exit(72);
    }
    let temporary = tempfile::tempdir().unwrap();
    let selected = temporary.path().canonicalize().unwrap();
    let original = selected.join("original");
    let private = PrivateDirectory::create(&original).unwrap();
    let reader = private.create_child(Path::new("reader-v1")).unwrap();
    drop(
        reader
            .create_database_file(Path::new("library.sqlite"))
            .unwrap(),
    );
    drop(
        reader
            .create_database_file(Path::new("index.sqlite"))
            .unwrap(),
    );
    drop(mdreader_core::ReaderStore::open(&original.join("reader-v1")).unwrap());
    // SQLite-created sidecars inherit ACLs. This fixture explicitly creates
    // private absent sidecars before the crashed writer, rather than relaxing
    // the production private-file admission or repairing an existing file.
    for name in ["library.sqlite-wal", "library.sqlite-shm"] {
        drop(reader.create_database_file(Path::new(name)).unwrap());
    }
    let root = SourceRoot::capture(original.clone()).unwrap();
    let json = JsonOwner::new(root.clone()).unwrap();
    let identity = json.load("main").unwrap().identity;
    json.write(
        "main",
        &identity,
        JsonKind::Settings,
        1,
        r#"{"theme":"dark"}"#,
    )
    .unwrap();
    let expected_json = json
        .read_current("main", &identity)
        .unwrap()
        .settings_json
        .unwrap();
    // A live renderer is never treated as closed, regardless of daemon state.
    assert!(matches!(
        ClosedOwner::acquire(&original),
        Err(Error::NotQuiescent)
    ));
    drop(json);
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact").arg("profile_transition::offline::owner::windows_tests::closed_windows_reader_exports_wal_and_canonical_json_without_touching_originals")
        .arg("--nocapture").env(CHILD, &original).status().unwrap();
    assert_eq!(status.code(), Some(72));
    let names = [
        "reader-v1/library.sqlite",
        "reader-v1/library.sqlite-wal",
        "reader-v1/library.sqlite-shm",
        "reader-json-v1.sqlite",
        "reader-json-v1.sqlite-journal",
    ];
    let before = evidence(&root, &names);
    let output = selected.join("exports");
    PrivateDirectory::create(&output).unwrap();
    let mut owner = ClosedOwner::acquire(&original).unwrap();
    let limits = mdreader_profile::Limits::default();
    let choices = super::super::super::backup_inventory::selections(&original, &limits).unwrap();
    let inventory = mdreader_profile::Inventory::capture(&original, &choices, &limits).unwrap();
    let snapshot = mdreader_profile::stage(
        &mut super::super::super::BorrowedFreeze { frozen: &mut owner },
        inventory,
        &output.join("copy"),
        &limits,
    )
    .unwrap();
    for (relative, expected) in [
        ("reader-json-v1.sqlite", expected_json.as_bytes()),
        (
            "reader-v1/library.sqlite",
            b"\xef\xbb\xbf# Original\r\n".as_slice(),
        ),
    ] {
        let entry = snapshot
            .manifest()
            .entries
            .iter()
            .find(|entry| entry.relative.restore().unwrap() == Path::new(relative))
            .unwrap();
        let copied = Connection::open_with_flags(
            output.join("copy").join(entry.member.as_ref().unwrap()),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let bytes: Vec<u8> = if relative.starts_with("reader-json") {
            copied
                .query_row(
                    "SELECT body FROM reader_json WHERE name='settings.json'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        } else {
            copied
                .query_row("SELECT body FROM preserved_test_body", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(bytes, expected);
    }
    let identity = snapshot.identity().clone();
    drop(snapshot);
    drop(owner);
    assert_eq!(evidence(&root, &names), before);
    mdreader_profile::verify(&output.join("copy"), &identity, &limits).unwrap();
}
