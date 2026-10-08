use super::*;
use std::fs;

fn fixture(root: &Path) -> Vec<Input> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let sqlite = root.join("proposals.db");
    let c = rusqlite::Connection::open(&sqlite).unwrap();
    c.execute_batch("CREATE TABLE proposal(fact_id TEXT PRIMARY KEY,kind TEXT,schema_name TEXT,extractor TEXT,rule TEXT,qualified TEXT,source_path TEXT,start_line INTEGER,end_line INTEGER,content_hash TEXT,endpoint_src TEXT,endpoint_dst TEXT,state TEXT,reviewer TEXT,reviewed_hash TEXT,payload TEXT);
INSERT INTO proposal VALUES('stable','object','decision','extract@1','D1','Decision','missing.md',2,3,'changed',NULL,NULL,'rejected','Reviewer','reviewed','{\"unknown\":true}');
CREATE TABLE future_history(i INTEGER,t TEXT,b BLOB,r REAL);
INSERT INTO future_history VALUES(9223372036854775807,'private authored history',x'00ff',1.25);
PRAGMA user_version=2;").unwrap();
    c.execute_batch("ALTER TABLE proposal ADD COLUMN future_integer INTEGER; ALTER TABLE proposal ADD COLUMN future_blob BLOB; ALTER TABLE proposal ADD COLUMN future_real REAL;
UPDATE proposal SET future_integer=9223372036854775807,future_blob=x'00ff',future_real=1.25;
INSERT INTO proposal(fact_id,state,reviewed_hash,source_path) VALUES('approved','approved','accepted-revision','deleted.md');
INSERT INTO proposal(fact_id,state,source_path) VALUES('pending','proposed','deleted.md');").unwrap();
    drop(c);
    let settings = root.join("settings.json");
    fs::write(&settings,b"{ \"settings\": {\"theme\":\"dark\",\"reviewer\":\"Reviewer\",\"future\":9223372036854775807} }\n").unwrap();
    vec![
        Input {
            role: Role::Proposals,
            path: sqlite,
        },
        Input {
            role: Role::Settings,
            path: settings,
        },
    ]
}

#[test]
fn lossless_roundtrip_keeps_unknown_history_and_review_tombstone() {
    let temp = tempfile::tempdir().unwrap();
    let inputs = fixture(temp.path());
    let before: Vec<_> = inputs.iter().map(|i| fs::read(&i.path).unwrap()).collect();
    let destination = temp.path().join("archive");
    export(&inputs, &destination, &Limits::default()).unwrap();
    verify(&destination, &Limits::default()).unwrap();
    assert_eq!(fs::read(destination.join("0000.raw")).unwrap(), before[0]);
    assert_eq!(fs::read(destination.join("0001.raw")).unwrap(), before[1]);
    assert_eq!(
        inputs
            .iter()
            .map(|i| fs::read(&i.path).unwrap())
            .collect::<Vec<_>>(),
        before
    );
    let archived = rusqlite::Connection::open_with_flags(
        destination.join("0000.raw"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let unknown: (i64, Vec<u8>) = archived
        .query_row("SELECT i,b FROM future_history", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(unknown, (i64::MAX, vec![0, 255]));
    let records: Vec<serde_json::Value> =
        fs::read_to_string(destination.join("0000.records.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    assert_eq!(records.len(), 3);
    let record = &records[0];
    assert_eq!(record["kind"], "proposal");
    assert_eq!(record["fields"]["state"]["value"], "72656a6563746564");
    assert_eq!(
        record["fields"]["reviewed_hash"]["value"],
        "7265766965776564"
    );
    assert_eq!(
        record["fields"]["future_integer"]["value"],
        i64::MAX.to_string()
    );
    assert_eq!(record["fields"]["future_blob"]["value"], "00ff");
    assert_eq!(
        record["fields"]["future_real"]["value"],
        format!("{:016x}", 1.25_f64.to_bits())
    );
    assert_eq!(records[1]["fields"]["state"]["value"], hex(b"approved"));
    assert_eq!(records[2]["fields"]["state"]["value"], hex(b"proposed"));
    let manifest = verify(&destination, &Limits::default()).unwrap();
    assert_eq!(
        manifest.artifacts[0]
            .sqlite
            .as_ref()
            .unwrap()
            .tables
            .iter()
            .find(|t| t.name == "future_history")
            .unwrap()
            .rows,
        Some(1)
    );
    assert_eq!(
        manifest.artifacts[1].locator.restore().unwrap(),
        inputs[1].path
    );
    drop(archived);
    let c = rusqlite::Connection::open(&inputs[0].path).unwrap();
    c.execute_batch("ALTER TABLE proposal DROP COLUMN reviewed_hash; PRAGMA user_version=1;")
        .unwrap();
    drop(c);
    let v1 = temp.path().join("v1-archive");
    export(&inputs[..1], &v1, &Limits::default()).unwrap();
    let first: serde_json::Value = serde_json::from_str(
        fs::read_to_string(v1.join("0000.records.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert!(first["fields"].get("reviewed_hash").is_none());
}

#[test]
fn tamper_missing_members_and_existing_destination_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let inputs = fixture(temp.path());
    let destination = temp.path().join("archive");
    export(&inputs, &destination, &Limits::default()).unwrap();
    assert!(export(&inputs, &destination, &Limits::default()).is_err());
    fs::write(destination.join("0001.raw"), b"changed").unwrap();
    assert!(verify(&destination, &Limits::default()).is_err());
    let destination = temp.path().join("second");
    export(&inputs, &destination, &Limits::default()).unwrap();
    fs::remove_file(destination.join("0000.records.jsonl")).unwrap();
    assert!(verify(&destination, &Limits::default()).is_err());
}

#[test]
fn unavailable_virtual_module_is_preserved_without_claiming_query_coverage() {
    let temp = tempfile::tempdir().unwrap();
    let inputs = fixture(temp.path());
    let path = &inputs[0].path;
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch("CREATE VIRTUAL TABLE future_search USING fts5(body); INSERT INTO future_search VALUES('authored future data');").unwrap();
    drop(c);
    // Model a module absent from the standalone exporter, without loading one.
    let mut before = fs::read(path).unwrap();
    let offset = before.windows(10).position(|w| w == b"USING fts5").unwrap();
    before[offset + 6..offset + 10].copy_from_slice(b"zzzz");
    fs::write(path, &before).unwrap();
    let destination = temp.path().join("future-module");
    let manifest = export(
        &[Input {
            role: Role::Catalog,
            path: path.clone(),
        }],
        &destination,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(fs::read(destination.join("0000.raw")).unwrap(), before);
    assert!(matches!(
        manifest.artifacts[0].conversion,
        Conversion::Unavailable { .. }
    ));
    assert!(manifest.artifacts[0].records.is_none());
    let info = manifest.artifacts[0].sqlite.as_ref().unwrap();
    // SQLite's physical integrity_check can succeed without loading a virtual
    // module; that does not establish logical row coverage for the module.
    assert!(matches!(info.integrity, SqliteIntegrity::Checked));
    assert!(info
        .tables
        .iter()
        .any(|t| t.name == "future_search" && t.rows.is_none()));
    verify(&destination, &Limits::default()).unwrap();
}

#[test]
fn explicit_json_versions_and_raw_unknown_data_are_not_sanitized() {
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let library = temp.path().join("library.json");
    let session = temp.path().join("session.json");
    let native_locator_path = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"settings-\xff.json".to_vec()));
    assert_eq!(
        Locator::capture(&native_locator_path)
            .unwrap()
            .restore()
            .unwrap(),
        native_locator_path
    );
    let settings = temp.path().join("settings.json");
    fs::write(
        &library,
        b"{\"version\":9,\"sources\":[],\"unknown\":{\"future\":9007199254740993}}\n",
    )
    .unwrap();
    fs::write(
        &session,
        b"{\"session\":{\"version\":1,\"root\":null,\"tabs\":[],\"recent\":[],\"unknown\":true}}\n",
    )
    .unwrap();
    fs::write(&settings, b"{bad but preserved").unwrap();
    let inputs = vec![
        Input {
            role: Role::Library,
            path: library.clone(),
        },
        Input {
            role: Role::Session,
            path: session,
        },
        Input {
            role: Role::Settings,
            path: settings.clone(),
        },
    ];
    let archive = temp.path().join("known");
    let manifest = export(&inputs, &archive, &Limits::default()).unwrap();
    assert!(matches!(
        manifest.artifacts[2].conversion,
        Conversion::Unavailable {
            reason: Unavailable::MalformedKnownRecord
        }
    ));
    assert_eq!(manifest.artifacts[2].locator.restore().unwrap(), settings);
    assert_eq!(
        fs::read(archive.join("0002.raw")).unwrap(),
        b"{bad but preserved"
    );
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(archive.join("0000.records.jsonl")).unwrap()).unwrap();
    assert_eq!(record["kind"], "library");
    assert_eq!(
        record["value"]["unknown"]["future"].as_u64(),
        Some(9007199254740993)
    );
    fs::write(&library, b"{\"version\":99,\"sources\":[],\"future\":true}").unwrap();
    let manifest = export(&inputs, &temp.path().join("future"), &Limits::default()).unwrap();
    assert!(matches!(
        manifest.artifacts[0].conversion,
        Conversion::Unavailable {
            reason: Unavailable::UnknownVersion
        }
    ));
    assert!(manifest.artifacts[0].records.is_none());
}

#[test]
fn sidecars_symlinks_reader_profile_and_bounds_fail_before_completion() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let inputs = fixture(temp.path());
    let destination = temp.path().join("archive");
    fs::write(temp.path().join("proposals.db-wal"), b"pending").unwrap();
    assert!(matches!(
        export(&inputs, &destination, &Limits::default()),
        Err(Error::Invalid)
    ));
    fs::remove_file(temp.path().join("proposals.db-wal")).unwrap();
    let limits = Limits {
        max_artifact_bytes: 8,
        ..Limits::default()
    };
    assert!(matches!(
        export(&inputs, &destination, &limits),
        Err(Error::Bounds)
    ));
    assert!(!destination.exists());
    let link = temp.path().join("linked.db");
    symlink(&inputs[0].path, &link).unwrap();
    assert!(export(
        &[Input {
            role: Role::Proposals,
            path: link
        }],
        &destination,
        &Limits::default()
    )
    .is_err());
    let reader = temp.path().join("reader-v1");
    fs::create_dir(&reader).unwrap();
    fs::write(reader.join("library.sqlite"), b"new state").unwrap();
    assert!(matches!(
        export(
            &[Input {
                role: Role::Catalog,
                path: reader.join("library.sqlite")
            }],
            &destination,
            &Limits::default()
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(
        fs::read(reader.join("library.sqlite")).unwrap(),
        b"new state"
    );
    let spec = SnapshotSpec {
        version: 1,
        offline_snapshots: false,
        inputs: SnapshotSpec::from_inputs(&inputs).unwrap().inputs,
    };
    assert!(spec.selected_inputs(&Limits::default()).is_err());
}

#[test]
fn source_replacement_during_export_is_not_acknowledged() {
    let temp = tempfile::tempdir().unwrap();
    let inputs = fixture(temp.path());
    let destination = temp.path().join("archive");
    let mut changed = false;
    let result = export_inner(&inputs, &destination, &Limits::default(), || {
        if !changed {
            changed = true;
            let replacement = temp.path().join("replacement.db");
            fs::copy(&inputs[0].path, &replacement).unwrap();
            fs::rename(&replacement, &inputs[0].path).unwrap();
        }
        Ok(())
    });
    assert!(matches!(result, Err(Error::Conflict)));
    assert!(!destination.exists());
}

#[test]
#[ignore]
fn offline_export_child() {
    let root = std::path::PathBuf::from(std::env::var_os("MDREADER_LEGACY_FIXTURE").unwrap());
    let inputs = vec![Input {
        role: Role::Proposals,
        path: root.join("proposals.db"),
    }];
    export_inner(&inputs, &root.join("archive"), &Limits::default(), || {
        fs::write(root.join("ready"), b"raw-synced").unwrap();
        loop {
            std::thread::park();
        }
    })
    .unwrap();
}

#[test]
fn killed_export_stays_private_and_retry_publishes_without_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let inputs = fixture(temp.path());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "tests::offline_export_child"])
        .env("MDREADER_LEGACY_FIXTURE", temp.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !temp.path().join("ready").exists() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("synthetic child failed to reach durable checkpoint")
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let destination = temp.path().join("archive");
    assert!(!destination.exists());
    let staging = fs::read_dir(temp.path())
        .unwrap()
        .map(|x| x.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".mdreader-export-")
        })
        .unwrap();
    assert!(staging.join("0000.raw").exists());
    assert!(verify(&staging, &Limits::default()).is_err());
    export(&inputs, &destination, &Limits::default()).unwrap();
    verify(&destination, &Limits::default()).unwrap();
    assert!(export(&inputs, &destination, &Limits::default()).is_err());
}
