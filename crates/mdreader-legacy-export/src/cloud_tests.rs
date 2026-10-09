use crate::*;
use rusqlite::params;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn offline_cloud_export_preserves_cached_and_accepted_bodies_and_unknown_raw_rows() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temp.path().join("cloud.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE mdreader_cloud_body_cache(source_id BLOB NOT NULL,source_version_id BLOB NOT NULL,record_key BLOB NOT NULL,revision BLOB NOT NULL,body BLOB NOT NULL,body_hash BLOB NOT NULL,size_bytes INTEGER NOT NULL CHECK(size_bytes>=0),last_access_ms INTEGER NOT NULL,PRIMARY KEY(source_id,source_version_id,record_key,revision));
CREATE TABLE mdreader_cloud_write_repair(operation_id BLOB PRIMARY KEY,source_id BLOB NOT NULL,record_key BLOB NOT NULL,accepted_revision BLOB NOT NULL,body_hash BLOB NOT NULL,receipt_json BLOB NOT NULL,state TEXT NOT NULL CHECK(state IN ('provider-accepted','published')),created_at_ms INTEGER NOT NULL);
CREATE TABLE future_authored_rows(value BLOB); INSERT INTO future_authored_rows VALUES(x'00ff');").unwrap();
    let cached = "\u{feff}# Cached\r\n\r\nExact original.\r\n";
    for (key, hash) in [
        (
            b"valid".as_slice(),
            *blake3::hash(cached.as_bytes()).as_bytes(),
        ),
        (b"invalid".as_slice(), [0; 32]),
    ] {
        db.execute(
            "INSERT INTO mdreader_cloud_body_cache VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                b"source",
                b"version",
                key,
                b"opaque-revision",
                cached.as_bytes(),
                hash.as_slice(),
                cached.len(),
                i64::MAX
            ],
        )
        .unwrap();
    }
    let accepted = "# Accepted but unpublished\n\nUseful user edit.\n";
    for (operation, version) in [(b"known".as_slice(), 1), (b"future".as_slice(), 2)] {
        let receipt = serde_json::to_vec(&serde_json::json!({"version":version,"expected_revision":"opaque-old","opened_body_hash":vec![0u8;32],"content":accepted,"modified_at_ms":17,"size_bytes":accepted.len()})).unwrap();
        db.execute("INSERT INTO mdreader_cloud_write_repair VALUES(?1,?2,?3,?4,?5,?6,'provider-accepted',18)", params![operation,b"source",b"edited-record",b"accepted-revision",blake3::hash(accepted.as_bytes()).as_bytes().as_slice(),receipt]).unwrap();
    }
    drop(db);
    let before = fs::read(&path).unwrap();
    let destination = temp.path().join("archive");
    let limits = Limits::default();
    export(
        &[Input {
            role: Role::CloudCatalog,
            path: path.clone(),
        }],
        &destination,
        &limits,
    )
    .unwrap();
    verify(&destination, &limits).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read(destination.join("0000.raw")).unwrap(), before);
    let lines = fs::read_to_string(destination.join("0000.records.jsonl")).unwrap();
    let values: Vec<serde_json::Value> = lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(values[0]["raw_only_tables"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("future_authored_rows")));
    assert_eq!(
        values
            .iter()
            .filter(|row| row["kind"] == "cloud_unavailable")
            .count(),
        2
    );
    let bodies: Vec<_> = lines
        .lines()
        .filter_map(|line| cloud::decode_record(line.as_bytes(), &limits).unwrap())
        .collect();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0].markdown.as_bytes(), cached.as_bytes());
    assert_eq!(bodies[0].revision_hex, hex(b"opaque-revision"));
    assert_eq!(bodies[0].recorded_at_ms, i64::MAX.to_string());
    assert_eq!(bodies[1].markdown.as_bytes(), accepted.as_bytes());
    assert_eq!(
        bodies[1].acceptance_state.as_deref(),
        Some("provider-accepted")
    );
    assert_eq!(
        bodies[1].operation_id_hex.as_deref(),
        Some(hex(b"known").as_str())
    );
    assert_eq!(bodies[1].expected_revision.as_deref(), Some("opaque-old"));
}
