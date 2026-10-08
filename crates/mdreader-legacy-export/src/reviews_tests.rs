use super::*;
use crate::{export, hex, Input, Role};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt};

fn fixture(root: &Path, version: u8) -> (std::path::PathBuf, ArchiveIdentity) {
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let sqlite = root.join("proposals.db");
    let c = rusqlite::Connection::open(&sqlite).unwrap();
    c.execute_batch("CREATE TABLE proposal(fact_id TEXT PRIMARY KEY,kind TEXT,schema_name TEXT,extractor TEXT,rule TEXT,qualified TEXT,source_path TEXT,start_line INTEGER,end_line INTEGER,content_hash TEXT,endpoint_src TEXT,endpoint_dst TEXT,state TEXT,reviewer TEXT,reviewed_hash TEXT,payload TEXT); PRAGMA user_version=2;").unwrap();
    let content_hash = format!("blake3:{}", "a".repeat(64));
    let reviewed_hash = format!("blake3:{}", "b".repeat(64));
    for (index, state) in ["proposed", "approved", "rejected", "FUTURE", "approved"]
        .into_iter()
        .enumerate()
    {
        let id = uuid::Uuid::new_v4().to_string();
        let source = "../opaque/deleted.md";
        let schema = if index == 4 {
            "generic_admin"
        } else {
            "decision_overlay"
        };
        let payload = serde_json::json!({"id":id,"entity":"decision_overlay.Decision","value":{"decision_title":"Keep rejected history","future":9007199254740993_u64},"provenance":{"source":source,"range":[2,5],"content_hash":content_hash,"schema_version":"1","extractor":"mdreader.decision@1","confidence":0.95,"approved_by":null}}).to_string();
        c.execute("INSERT INTO proposal VALUES(?1,'object',?2,'mdreader.decision@1','D1','decision_overlay.Decision',?3,2,5,?4,NULL,NULL,?5,?6,?7,?8)", params![id,schema,source,content_hash,state,if state=="proposed" {None} else {Some("Reviewer")},if state=="rejected" {Some(reviewed_hash.as_str())} else if state=="approved" {Some(content_hash.as_str())} else {None},payload]).unwrap();
    }
    let id = uuid::Uuid::new_v4().to_string();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    let payload=serde_json::json!({"id":id,"arrow":"architecture_overlay.exposes","source":a,"target":b,"provenance":{"source":"api.md","range":[10,12],"content_hash":content_hash,"schema_version":"1","extractor":"mdreader.architecture@1","confidence":0.85,"approved_by":null}}).to_string();
    c.execute("INSERT INTO proposal VALUES(?1,'morphism','architecture_overlay','mdreader.architecture@1','A3','architecture_overlay.exposes','api.md',10,12,?2,?3,?4,'approved','Reviewer',?2,?5)",params![id,content_hash,a,b,payload]).unwrap();
    c.execute("INSERT INTO proposal SELECT ?1,kind,schema_name,extractor,rule,qualified,CAST(x'ff' AS TEXT),start_line,end_line,content_hash,endpoint_src,endpoint_dst,state,reviewer,reviewed_hash,payload FROM proposal WHERE rowid=1",[uuid::Uuid::new_v4().to_string()]).unwrap();
    if version == 1 {
        c.execute_batch("ALTER TABLE proposal DROP COLUMN reviewed_hash; PRAGMA user_version=1;")
            .unwrap();
    }
    drop(c);
    let archive = root.join("export");
    let m = export(
        &[Input {
            role: Role::Proposals,
            path: sqlite,
        }],
        &archive,
        &Limits::default(),
    )
    .unwrap();
    let expected = ArchiveIdentity {
        export_id: m.export_id,
        manifest_sha256: hex(&Sha256::digest(
            fs::read(archive.join("manifest.json")).unwrap(),
        )),
    };
    (archive, expected)
}

#[test]
fn verified_archive_stages_exact_reviews_without_grants_graph_or_history_claims() {
    let temp = tempfile::tempdir().unwrap();
    let (archive, identity) = fixture(temp.path(), 2);
    let destination = temp.path().join("reviews");
    let receipt = stage_reviews(&archive, &identity, &destination, &Limits::default()).unwrap();
    assert_eq!(receipt.records, 7);
    assert!(!receipt.replayed);
    let read = ReviewArchive::open(&destination, &identity, &Limits::default()).unwrap();
    let first = read.list(0, 2).unwrap();
    assert_eq!(first.records.len(), 2);
    assert_eq!(first.next_offset, Some(2));
    assert_eq!(first.records[1].state, ArchivedState::Approved);
    assert_eq!(first.records[1].reviewer.as_deref(), Some("Reviewer"));
    assert_eq!(
        first.records[1].recorded_hash_comparison,
        RecordedHashComparison::Equal
    );
    for r in &first.records {
        assert_eq!(r.graph_membership, GraphMembership::Unavailable);
        assert_eq!(r.chronology, ReviewChronology::Unavailable);
        assert_eq!(r.current_evidence, CurrentEvidence::SourceBindingRequired);
        assert!(matches!(r.assertion, Assertion::DocumentSpan(_)));
    }
    let rest = read.list(2, 100).unwrap();
    assert_eq!(rest.records[0].state, ArchivedState::Rejected);
    assert_eq!(
        rest.records[0].recorded_hash_comparison,
        RecordedHashComparison::Differ
    );
    assert_eq!(rest.records[1].state, ArchivedState::Unknown);
    assert!(matches!(
        rest.records[1].assertion,
        Assertion::Unavailable {
            reason: UnavailableAssertion::UnknownReviewState
        }
    ));
    assert!(matches!(
        rest.records[2].assertion,
        Assertion::Unavailable { .. }
    ));
    assert!(
        matches!(&rest.records[3].assertion, Assertion::DocumentSpan(span) if span.fact_kind=="morphism" && span.endpoint_src.is_some() && span.endpoint_dst.is_some())
    );
    assert!(matches!(
        rest.records[4].assertion,
        Assertion::Unavailable {
            reason: UnavailableAssertion::MalformedAssertion
        }
    ));
    let non_utf8: serde_json::Value = serde_json::from_slice(
        &read
            .raw_record(&rest.records[4].record_id)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(non_utf8["fields"]["source_path"]["value"], "ff");
    let raw = read
        .raw_record(&first.records[1].record_id)
        .unwrap()
        .unwrap();
    let original = fs::read_to_string(archive.join("0000.records.jsonl")).unwrap();
    assert_eq!(raw, original.lines().nth(1).unwrap().as_bytes());
    assert_eq!(first.records[1].raw.row_sha256, hex(&Sha256::digest(&raw)));
    assert_eq!(
        read.get(&first.records[1].record_id)
            .unwrap()
            .unwrap()
            .state,
        ArchivedState::Approved
    );
    drop(read);
    assert!(
        stage_reviews(&archive, &identity, &destination, &Limits::default())
            .unwrap()
            .replayed
    );
}

#[test]
fn typed_raw_disagreement_and_wrong_selected_identity_never_publish() {
    let temp = tempfile::tempdir().unwrap();
    let (archive, mut identity) = fixture(temp.path(), 2);
    let wrong = ArchiveIdentity {
        export_id: uuid::Uuid::new_v4(),
        manifest_sha256: identity.manifest_sha256.clone(),
    };
    let destination = temp.path().join("reviews");
    assert!(stage_reviews(&archive, &wrong, &destination, &Limits::default()).is_err());
    let records_path = archive.join("0000.records.jsonl");
    let records = fs::read_to_string(&records_path).unwrap();
    let changed = records.replacen(&hex(b"proposed"), &hex(b"approved"), 1);
    assert_ne!(records, changed);
    fs::write(&records_path, &changed).unwrap();
    let mut manifest: crate::Manifest =
        serde_json::from_slice(&fs::read(archive.join("manifest.json")).unwrap()).unwrap();
    let member = manifest
        .members
        .iter_mut()
        .find(|m| m.name == "0000.records.jsonl")
        .unwrap();
    member.bytes = changed.len() as u64;
    member.sha256 = hex(&Sha256::digest(changed.as_bytes()));
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(archive.join("manifest.json"), &manifest_bytes).unwrap();
    identity.manifest_sha256 = hex(&Sha256::digest(&manifest_bytes));
    crate::verify(&archive, &Limits::default()).unwrap();
    assert!(matches!(
        stage_reviews(&archive, &identity, &destination, &Limits::default()),
        Err(Error::Integrity)
    ));
    assert!(!destination.exists());
}

#[test]
fn interrupted_import_retries_without_overwrite_or_reader_profile_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let (archive, identity) = fixture(temp.path(), 2);
    let before = fs::read(archive.join("0000.raw")).unwrap();
    let destination = temp.path().join("reviews");
    assert!(matches!(
        super::store::stage_inner(
            &archive,
            &identity,
            &destination,
            &Limits::default(),
            || Err(Error::Interrupted)
        ),
        Err(Error::Interrupted)
    ));
    assert!(!destination.exists());
    stage_reviews(&archive, &identity, &destination, &Limits::default()).unwrap();
    let completed = fs::read(destination.join("reviews.sqlite")).unwrap();
    let limited = Limits {
        max_rows: 2,
        ..Limits::default()
    };
    assert!(stage_reviews(&archive, &identity, &temp.path().join("bounded"), &limited).is_err());
    assert!(!temp.path().join("bounded").exists());
    let protected = temp.path().join("reader-v1");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("library.sqlite"), b"new owner state").unwrap();
    assert!(stage_reviews(
        &archive,
        &identity,
        &protected.join("reviews"),
        &Limits::default()
    )
    .is_err());
    assert_eq!(
        fs::read(protected.join("library.sqlite")).unwrap(),
        b"new owner state"
    );
    assert_eq!(
        fs::read(destination.join("reviews.sqlite")).unwrap(),
        completed
    );
    assert_eq!(fs::read(archive.join("0000.raw")).unwrap(), before);
}

#[test]
fn v1_missing_review_hash_stays_unknown_and_stage_tamper_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let (archive, identity) = fixture(temp.path(), 1);
    let destination = temp.path().join("reviews");
    stage_reviews(&archive, &identity, &destination, &Limits::default()).unwrap();
    let read = ReviewArchive::open(&destination, &identity, &Limits::default()).unwrap();
    let all = read.list(0, 100).unwrap();
    assert_eq!(all.records[2].state, ArchivedState::Rejected);
    assert!(all.records[2].reviewed_content_hash.is_none());
    assert_eq!(
        all.records[2].recorded_hash_comparison,
        RecordedHashComparison::Unknown
    );
    drop(read);
    fs::write(destination.join("reviews.sqlite"), b"tampered").unwrap();
    assert!(ReviewArchive::open(&destination, &identity, &Limits::default()).is_err());
    assert!(stage_reviews(&archive, &identity, &destination, &Limits::default()).is_err());
}
