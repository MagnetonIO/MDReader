#![cfg(unix)]
use mdreader_legacy_export::{export, Input, Limits, Role};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Output},
};

fn run(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mdreader-legacy-export"))
        .args(args)
        .output()
        .unwrap()
}
fn json(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 4 * 1024 * 1024 + 4096);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn review_commands_stage_replay_and_read_only_the_explicit_selected_identity() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let input = root.path().join("offline proposals.db");
    let c = rusqlite::Connection::open(&input).unwrap();
    c.execute_batch("CREATE TABLE proposal(fact_id TEXT PRIMARY KEY,kind TEXT,schema_name TEXT,extractor TEXT,rule TEXT,qualified TEXT,source_path TEXT,start_line INTEGER,end_line INTEGER,content_hash TEXT,endpoint_src TEXT,endpoint_dst TEXT,state TEXT,reviewer TEXT,reviewed_hash TEXT,payload TEXT); INSERT INTO proposal VALUES('synthetic-preserved-id','object','generic_admin','legacy@1','U1','Unsupported','missing.md',2,3,'prior',NULL,NULL,'rejected','synthetic-reviewer','reviewed','{}'); PRAGMA user_version=2;").unwrap();
    drop(c);
    let archive = root.path().join("archive");
    let manifest = export(
        &[Input {
            role: Role::Proposals,
            path: input,
        }],
        &archive,
        &Limits::default(),
    )
    .unwrap();
    let uuid = manifest.export_id.to_string();
    let hash = Sha256::digest(fs::read(archive.join("manifest.json")).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let stage = root.path().join("reviews");
    let staged = json(run(&[
        "stage-reviews".as_ref(),
        archive.as_os_str(),
        uuid.as_ref(),
        hash.as_ref(),
        stage.as_os_str(),
    ]));
    assert_eq!(staged["scope"], "archival_only");
    assert_eq!(staged["receipt"]["records"], 1);
    assert_eq!(staged["receipt"]["replayed"], false);
    assert!(serde_json::to_vec(&staged).unwrap().len() < 4096);
    assert!(!staged.to_string().contains("synthetic-reviewer"));
    let replay = json(run(&[
        "stage-reviews".as_ref(),
        archive.as_os_str(),
        uuid.as_ref(),
        hash.as_ref(),
        stage.as_os_str(),
    ]));
    assert_eq!(replay["receipt"]["replayed"], true);
    let before = fs::read(stage.join("reviews.sqlite")).unwrap();
    let listed = json(run(&[
        "list-reviews".as_ref(),
        stage.as_os_str(),
        uuid.as_ref(),
        hash.as_ref(),
        "0".as_ref(),
        "1".as_ref(),
    ]));
    assert_eq!(listed["currentDocumentEvidence"], "unavailable");
    assert_eq!(listed["sourceBinding"], "not_performed");
    let record = &listed["page"]["records"][0];
    assert_eq!(record["state"], "rejected");
    assert_eq!(record["assertion"]["kind"], "unavailable");
    assert_eq!(record["graphMembership"], "unavailable");
    assert!(record.get("raw_json").is_none());
    assert_eq!(record["raw"]["archive"]["exportId"], uuid);
    let record_id = record["recordId"].as_str().unwrap();
    let got = json(run(&[
        "get-review".as_ref(),
        stage.as_os_str(),
        uuid.as_ref(),
        hash.as_ref(),
        record_id.as_ref(),
    ]));
    assert_eq!(got["record"], *record);
    let missing = "0".repeat(64);
    let empty = json(run(&[
        "get-review".as_ref(),
        stage.as_os_str(),
        uuid.as_ref(),
        hash.as_ref(),
        missing.as_ref(),
    ]));
    assert!(empty["record"].is_null());
    let wrong = uuid::Uuid::new_v4().to_string();
    for command in ["list-reviews", "get-review"] {
        let mut args = vec![
            OsStr::new(command),
            stage.as_os_str(),
            wrong.as_ref(),
            hash.as_ref(),
        ];
        if command == "list-reviews" {
            args.extend([OsStr::new("0"), OsStr::new("1")]);
        } else {
            args.push(record_id.as_ref());
        }
        let refused = run(&args);
        assert!(!refused.status.success());
        assert!(refused.stdout.is_empty());
        let error = String::from_utf8_lossy(&refused.stderr);
        assert!(!error.contains(&wrong));
        assert!(!error.contains("synthetic-reviewer"));
        assert!(!error.contains(root.path().to_string_lossy().as_ref()));
    }
    let bad_limit = run(&[
        "list-reviews".as_ref(),
        stage.as_os_str(),
        uuid.as_ref(),
        hash.as_ref(),
        "0".as_ref(),
        "101".as_ref(),
    ]);
    assert!(!bad_limit.status.success());
    assert!(bad_limit.stdout.is_empty());
    assert_eq!(fs::read(stage.join("reviews.sqlite")).unwrap(), before);
    assert_eq!(fs::read_dir(stage).unwrap().count(), 2);
}
