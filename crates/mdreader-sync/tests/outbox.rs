use mdreader_sync::*;
use rusqlite::Connection;
use tempfile::TempDir;
#[test]
fn real_sqlite_sync_migration_preserves_reader_owner_schema_and_reopens() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("synthetic-library.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA user_version=27; CREATE TABLE owner_records(value TEXT); INSERT INTO owner_records VALUES('keep');").unwrap();
    migrate(&mut connection).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        27
    );
    drop(connection);
    let mut reopened = Connection::open(&path).unwrap();
    migrate(&mut reopened).unwrap();
    assert_eq!(
        reopened
            .query_row("SELECT value FROM owner_records", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        reopened
            .query_row(
                "SELECT version FROM mdreader_sync_meta WHERE singleton=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::json;
fn context() -> EncryptionContext {
    serde_json::from_value(
        serde_json::from_str::<serde_json::Value>(include_str!("../fixtures/a256gcm-v1.json"))
            .unwrap()["context"]
            .clone(),
    )
    .unwrap()
}
fn scope(account: char) -> SyncScope {
    SyncScope::new(
        account.to_string().repeat(64),
        context().library_id,
        URL_SAFE_NO_PAD.encode([17; 32]),
    )
    .unwrap()
}
fn op() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn connection(temp: &TempDir) -> Connection {
    let mut db = Connection::open(temp.path().join("library.sqlite")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .unwrap();
    migrate(&mut db).unwrap();
    db
}
#[test]
fn attaching_requires_owner_durable_journal_and_synchronous_settings() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    for settings in [
        "PRAGMA journal_mode=OFF; PRAGMA synchronous=FULL;",
        "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=FULL;",
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
    ] {
        db.execute_batch(settings).unwrap();
        assert!(
            matches!(
                SyncStore::attach(&mut db, scope('a')),
                Err(SyncError::NotReady)
            ),
            "{settings}"
        );
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .unwrap();
    SyncStore::attach(&mut db, scope('a')).unwrap();
}
fn key() -> LibraryKey {
    LibraryKey::from_bytes(context().key_id, zeroize::Zeroizing::new(vec![7; 32])).unwrap()
}
fn upload(entry: &OutboxEntry) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"blob":entry.operation.blob,"bytes":entry.envelope_utf8.as_ref().unwrap().len()}),
    )
    .unwrap()
}
fn receipt(entry: &OutboxEntry, sequence: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({"operationId":entry.operation.operation_id,"revisionId":entry.operation.revision_id,"sequence":sequence,"conflict":false,"heads":[entry.operation.revision_id]})).unwrap()
}
#[test]
fn encrypted_outbox_reopens_unacked_replay_and_only_verified_receipts_ack() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    let authority = scope('a');
    let operation = op();
    let entry = {
        let mut store = SyncStore::attach(&mut db, authority.clone()).unwrap();
        store
            .seal_enqueue(
                &operation,
                &[],
                &key(),
                &context(),
                b"PRIVATE PLAINTEXT MUST NOT ENTER SQLITE",
            )
            .unwrap()
    };
    assert_eq!(entry.state, OutboxState::Pending);
    assert!(!entry
        .envelope_utf8
        .as_ref()
        .unwrap()
        .windows(17)
        .any(|s| s == b"PRIVATE PLAINTEXT "));
    drop(db);
    let mut db = Connection::open(temp.path().join("library.sqlite")).unwrap();
    let mut store = SyncStore::attach(&mut db, authority.clone()).unwrap();
    assert_eq!(store.pending(10).unwrap().len(), 1);
    assert_eq!(
        store
            .seal_enqueue(
                &operation,
                &[],
                &key(),
                &context(),
                b"PRIVATE PLAINTEXT MUST NOT ENTER SQLITE"
            )
            .unwrap()
            .operation,
        entry.operation
    );
    assert!(store
        .commit_receipt(&authority, 201, &receipt(&entry, 1))
        .is_err());
    assert!(store
        .mark_uploaded(&scope('b'), &operation, 201, &upload(&entry))
        .is_err());
    assert!(store
        .mark_uploaded(&authority, &operation, 503, &upload(&entry))
        .is_err());
    assert_eq!(
        store.entry(&operation).unwrap().unwrap().state,
        OutboxState::Pending
    );
    store
        .mark_uploaded(&authority, &operation, 201, &upload(&entry))
        .unwrap();
    let mut wrong: serde_json::Value = serde_json::from_slice(&receipt(&entry, 1)).unwrap();
    wrong["revisionId"] = op().into();
    assert!(store
        .commit_receipt(&authority, 201, &serde_json::to_vec(&wrong).unwrap())
        .is_err());
    store
        .commit_receipt(&authority, 201, &receipt(&entry, 1))
        .unwrap();
    store
        .commit_receipt(&authority, 200, &receipt(&entry, 1))
        .unwrap();
    assert!(store.pending(10).unwrap().is_empty());
    assert_eq!(
        store.entry(&operation).unwrap().unwrap().state,
        OutboxState::Acknowledged
    );
    assert!(store
        .commit_receipt(&authority, 200, &receipt(&entry, 2))
        .is_err());
    assert!(store
        .seal_enqueue(&operation, &[], &key(), &context(), b"changed payload")
        .is_err());
}
#[test]
fn account_library_and_device_scope_are_revalidated_for_every_transition() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    let authority = scope('a');
    let operation = op();
    let entry = SyncStore::attach(&mut db, authority.clone())
        .unwrap()
        .seal_enqueue(&operation, &[], &key(), &context(), b"selected content")
        .unwrap();
    for other in [
        scope('b'),
        SyncScope::new("a".repeat(64), op(), authority.device_id().into()).unwrap(),
        SyncScope::new(
            "a".repeat(64),
            authority.library_id().into(),
            URL_SAFE_NO_PAD.encode([18; 32]),
        )
        .unwrap(),
    ] {
        let mut store = SyncStore::attach(&mut db, other).unwrap();
        assert!(store.pending(10).unwrap().is_empty());
        assert!(store.entry(&operation).unwrap().is_none());
        assert!(store
            .mark_uploaded(&authority, &operation, 201, &upload(&entry))
            .is_err());
        assert_eq!(store.cursors().unwrap(), CursorState::default());
    }
    assert_eq!(
        SyncStore::attach(&mut db, authority)
            .unwrap()
            .pending(10)
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn nonce_reservations_survive_reopen_and_retries_do_not_reencrypt() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    let authority = scope('a');
    let library_key = key();
    let operation = op();
    let a = SyncStore::attach(&mut db, authority.clone())
        .unwrap()
        .seal_enqueue(&operation, &[], &library_key, &context(), b"once")
        .unwrap();
    let b = SyncStore::attach(&mut db, authority.clone())
        .unwrap()
        .seal_enqueue(&operation, &[], &library_key, &context(), b"once")
        .unwrap();
    assert_eq!(a.envelope_utf8, b.envelope_utf8);
    assert_eq!(
        db.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    drop(db);
    let mut db = Connection::open(temp.path().join("library.sqlite")).unwrap();
    let mut next = context();
    next.revision_id = op();
    let c = SyncStore::attach(&mut db, authority)
        .unwrap()
        .seal_enqueue(&op(), &[], &library_key, &next, b"twice")
        .unwrap();
    let nonce = |entry: &OutboxEntry| {
        serde_json::from_slice::<serde_json::Value>(entry.envelope_utf8.as_ref().unwrap()).unwrap()
            ["nonce"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_ne!(nonce(&a), nonce(&c));
    assert_eq!(
        db.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn future_migrations_and_quota_fail_without_overwriting_or_discarding() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    db.execute("UPDATE mdreader_sync_meta SET version=2", [])
        .unwrap();
    assert!(matches!(migrate(&mut db), Err(SyncError::Schema)));
    db.execute("UPDATE mdreader_sync_meta SET version=1", [])
        .unwrap();
    let limits = SyncLimits {
        max_outbox_entries: 1,
        ..SyncLimits::default()
    };
    let mut store = SyncStore::with_limits(&mut db, scope('a'), limits).unwrap();
    store
        .seal_enqueue(&op(), &[], &key(), &context(), b"retained")
        .unwrap();
    let mut next = context();
    next.revision_id = op();
    assert!(matches!(
        store.seal_enqueue(&op(), &[], &key(), &next, b"rejected"),
        Err(SyncError::Bounds)
    ));
    assert_eq!(store.pending(10).unwrap().len(), 1);
}

fn page(changes: Vec<serde_json::Value>, cursor: u64, high: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({"changes":changes,"cursor":cursor,"highWater":high,"hasMore":cursor<high,"checkpoint":0})).unwrap()
}
fn change(
    revision: &PreparedRevision,
    sequence: u64,
    parents: Vec<String>,
    deleted: bool,
) -> serde_json::Value {
    json!({"sequence":sequence,"revisionId":revision.context().revision_id,"itemId":revision.context().item_id,"blob":if deleted{None}else{Some(revision.blob_id())},"deleted":deleted,"parents":parents,"device":scope('a').device_id()})
}
#[test]
fn encrypted_changes_preserve_branches_tombstones_and_stage_apply_ack_boundaries() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    let authority = scope('a');
    let k = key();
    db.execute_batch("CREATE TABLE reader_application(value TEXT);")
        .unwrap();
    let initial = encrypt(&k, &context(), b"initial body").unwrap();
    let mut branch_ctx = context();
    branch_ctx.revision_id = op();
    let branch = encrypt(&k, &branch_ctx, b"offline branch").unwrap();
    let mut delete_ctx = context();
    delete_ctx.revision_id = op();
    let deletion = encrypt(&k, &delete_ctx, b"unused tombstone helper").unwrap();
    let first = page(vec![change(&initial, 1, vec![], false)], 1, 3);
    let mut store = SyncStore::attach(&mut db, authority.clone()).unwrap();
    assert!(store
        .stage_changes(&authority, 200, 0, &first, &[])
        .is_err());
    assert_eq!(store.cursors().unwrap(), CursorState::default());
    store
        .stage_changes(&authority, 200, 0, &first, std::slice::from_ref(&initial))
        .unwrap();
    assert_eq!(
        store.cursors().unwrap(),
        CursorState {
            received: 1,
            applied: 0,
            server_acknowledged: 0
        }
    );
    assert!(store
        .confirm_cursor_ack(&authority, 200, 1, br#"{"cursor":1}"#)
        .is_err());
    let rest = page(
        vec![
            change(
                &deletion,
                2,
                vec![initial.context().revision_id.clone()],
                true,
            ),
            // A device reconnecting after deletion cannot silently resurrect it.
            change(
                &branch,
                3,
                vec![initial.context().revision_id.clone()],
                false,
            ),
        ],
        3,
        3,
    );
    store
        .stage_changes(&authority, 200, 1, &rest, std::slice::from_ref(&branch))
        .unwrap();
    let heads = store.heads(&context().item_id).unwrap();
    assert_eq!(heads.len(), 2);
    assert_eq!(store.staged(0, 100).unwrap().len(), 3);
    assert!(heads.iter().any(|h| h.change.deleted));
    assert!(heads
        .iter()
        .any(|h| h.change.revision_id == branch_ctx.revision_id));
    assert!(store.mark_applied(4, |_| Ok(())).is_err());
    assert!(store
        .mark_applied(3, |tx| {
            tx.execute("INSERT INTO reader_application VALUES('rollback')", [])?;
            Err(SyncError::NotReady)
        })
        .is_err());
    assert_eq!(store.cursors().unwrap().applied, 0);
    store
        .mark_applied(3, |tx| {
            tx.execute(
                "INSERT INTO reader_application VALUES('durable application')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    store
        .confirm_cursor_ack(&authority, 200, 3, br#"{"cursor":3}"#)
        .unwrap();
    assert_eq!(
        store.cursors().unwrap(),
        CursorState {
            received: 3,
            applied: 3,
            server_acknowledged: 3
        }
    );
    // Replaying an old page does not resurrect a consumed head.
    store
        .stage_changes(&authority, 200, 0, &first, &[])
        .unwrap();
    assert_eq!(store.heads(&context().item_id).unwrap().len(), 2);
    // Only an explicit revision naming both heads resolves this conflict.
    let mut merged_context = context();
    merged_context.revision_id = op();
    let merged = encrypt(&k, &merged_context, b"explicit resolution").unwrap();
    let resolution = page(
        vec![change(
            &merged,
            4,
            vec![delete_ctx.revision_id, branch_ctx.revision_id],
            false,
        )],
        4,
        4,
    );
    store
        .stage_changes(&authority, 200, 3, &resolution, &[merged])
        .unwrap();
    assert_eq!(store.heads(&context().item_id).unwrap().len(), 1);
    assert_eq!(store.staged(0, 100).unwrap().len(), 4);
    assert!(store
        .staged(0, 100)
        .unwrap()
        .iter()
        .any(|revision| revision.change.deleted));
    drop(store);
    assert_eq!(
        db.query_row("SELECT count(*) FROM reader_application", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    drop(db);
    let mut db = Connection::open(temp.path().join("library.sqlite")).unwrap();
    let store = SyncStore::attach(&mut db, authority).unwrap();
    assert_eq!(store.cursors().unwrap().server_acknowledged, 3);
    assert_eq!(store.heads(&context().item_id).unwrap().len(), 1);
    assert_eq!(store.staged(0, 100).unwrap().len(), 4);
}
#[test]
fn tampered_change_pages_missing_blobs_wrong_context_and_scope_never_advance_cursor() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    let authority = scope('a');
    let k = key();
    let initial = encrypt(&k, &context(), b"body").unwrap();
    let valid = page(vec![change(&initial, 1, vec![], false)], 1, 1);
    let mut wrong_ctx = context();
    wrong_ctx.item_id = op();
    let wrong = encrypt(&k, &wrong_ctx, b"body").unwrap();
    let mut store = SyncStore::attach(&mut db, authority.clone()).unwrap();
    assert!(store
        .stage_changes(&scope('b'), 200, 0, &valid, std::slice::from_ref(&initial))
        .is_err());
    assert!(store
        .stage_changes(&authority, 200, 0, &valid, &[wrong])
        .is_err());
    assert!(store
        .stage_changes(&authority, 200, 1, &valid, std::slice::from_ref(&initial))
        .is_err());
    for mutation in ["gap", "cursor", "highWater", "checkpoint", "blobMissing"] {
        let mut value: serde_json::Value = serde_json::from_slice(&valid).unwrap();
        match mutation {
            "gap" => value["changes"][0]["sequence"] = 2.into(),
            "cursor" => value["cursor"] = 2.into(),
            "highWater" => value["highWater"] = 0.into(),
            "checkpoint" => value["checkpoint"] = 1.into(),
            _ => {
                value["changes"][0].as_object_mut().unwrap().remove("blob");
            }
        }
        assert!(
            store
                .stage_changes(
                    &authority,
                    200,
                    0,
                    &serde_json::to_vec(&value).unwrap(),
                    std::slice::from_ref(&initial)
                )
                .is_err(),
            "{mutation}"
        );
    }
    assert_eq!(store.cursors().unwrap(), CursorState::default());
    assert!(store.heads(&context().item_id).unwrap().is_empty());
}

#[test]
fn required_null_blob_fields_and_orphan_schema_fail_closed() {
    let missing = json!({"operationId":op(),"itemId":context().item_id,"revisionId":op(),"parents":[],"deleted":true});
    assert!(serde_json::from_value::<CommitOperation>(missing).is_err());
    let temp = TempDir::new().unwrap();
    let mut db = Connection::open(temp.path().join("orphan.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE mdreader_sync_outbox(value TEXT); INSERT INTO mdreader_sync_outbox VALUES('must survive');").unwrap();
    assert!(matches!(migrate(&mut db), Err(SyncError::Schema)));
    assert_eq!(
        db.query_row("SELECT value FROM mdreader_sync_outbox", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "must survive"
    );
}
#[test]
fn conflict_receipts_tombstone_outbox_and_nonce_budget_preserve_prior_data() {
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    let authority = scope('a');
    let limits = SyncLimits {
        max_nonce_reservations: 1,
        ..SyncLimits::default()
    };
    let mut store = SyncStore::with_limits(&mut db, authority.clone(), limits).unwrap();
    let operation = op();
    let a = store
        .seal_enqueue(&operation, &[], &key(), &context(), b"first")
        .unwrap();
    let mut ctx = context();
    ctx.revision_id = op();
    assert!(matches!(
        store.seal_enqueue(&op(), &[], &key(), &ctx, b"budget failure"),
        Err(SyncError::Bounds)
    ));
    assert_eq!(store.pending(10).unwrap().len(), 1);
    store
        .mark_uploaded(&authority, &operation, 201, &upload(&a))
        .unwrap();
    let competing = op();
    let conflict = json!({"operationId":operation,"revisionId":a.operation.revision_id,"sequence":1,"conflict":true,"heads":[a.operation.revision_id,competing]});
    store
        .commit_receipt(&authority, 201, &serde_json::to_vec(&conflict).unwrap())
        .unwrap();
    assert_eq!(
        store
            .entry(&operation)
            .unwrap()
            .unwrap()
            .receipt
            .unwrap()
            .heads
            .len(),
        2
    );
    let tombstone = store
        .enqueue_tombstone(
            &op(),
            &ctx.item_id,
            &ctx.revision_id,
            std::slice::from_ref(&a.operation.revision_id),
        )
        .unwrap();
    assert_eq!(tombstone.state, OutboxState::Uploaded);
    assert!(tombstone.envelope_utf8.is_none());
    store
        .commit_receipt(&authority, 201, &receipt(&tombstone, 2))
        .unwrap();
    assert!(store.pending(10).unwrap().is_empty());
}

#[test]
fn crash_application_worker() {
    use std::io::Write;
    let Some(base) = std::env::var_os("MDREADER_SYNC_SYNTHETIC_CRASH") else {
        return;
    };
    let mut db = Connection::open(std::path::PathBuf::from(base).join("library.sqlite")).unwrap();
    let mut store = SyncStore::attach(&mut db, scope('a')).unwrap();
    store
        .mark_applied(1, |tx| {
            tx.execute("INSERT INTO reader_application VALUES('uncommitted')", [])?;
            println!("application-uncommitted");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        })
        .unwrap();
}
#[test]
fn killed_owner_rolls_back_application_and_applied_cursor_together() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    let temp = TempDir::new().unwrap();
    let mut db = connection(&temp);
    let authority = scope('a');
    db.execute_batch("CREATE TABLE reader_application(value TEXT);")
        .unwrap();
    let body = encrypt(&key(), &context(), b"durable ciphertext").unwrap();
    SyncStore::attach(&mut db, authority.clone())
        .unwrap()
        .stage_changes(
            &authority,
            200,
            0,
            &page(vec![change(&body, 1, vec![], false)], 1, 1),
            &[body],
        )
        .unwrap();
    drop(db);
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_application_worker", "--nocapture"])
        .env("MDREADER_SYNC_SYNTHETIC_CRASH", temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(worker.stdout.take().unwrap());
    let mut ready = false;
    for _ in 0..16 {
        let mut line = String::new();
        if output.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line.contains("application-uncommitted") {
            ready = true;
            break;
        }
    }
    assert!(ready);
    worker.kill().unwrap();
    assert!(!worker.wait().unwrap().success());
    let mut db = Connection::open(temp.path().join("library.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM reader_application", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    let mut store = SyncStore::attach(&mut db, authority).unwrap();
    assert_eq!(
        store.cursors().unwrap(),
        CursorState {
            received: 1,
            applied: 0,
            server_acknowledged: 0
        }
    );
    assert_eq!(store.staged(0, 10).unwrap().len(), 1);
    store
        .mark_applied(1, |tx| {
            tx.execute("INSERT INTO reader_application VALUES('committed')", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.cursors().unwrap().applied, 1);
}
