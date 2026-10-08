use mdreader_sync::*;
use rusqlite::Connection;
use serde_json::json;

#[test]
fn final_authority_failure_rolls_back_owner_receipt_and_cursor_across_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.sqlite");
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE owner_receipts(value TEXT NOT NULL);").unwrap();
    migrate(&mut db).unwrap();
    let context = EncryptionContext {
        library_id: "11111111-1111-4111-8111-111111111111".into(),
        item_id: "22222222-2222-4222-8222-222222222222".into(),
        revision_id: "33333333-3333-4333-8333-333333333333".into(),
        key_id: "44444444-4444-4444-8444-444444444444".into(),
    };
    let scope = SyncScope::new("a".repeat(64), context.library_id.clone(), "A".repeat(43)).unwrap();
    let key = LibraryKey::from_bytes(context.key_id.clone(), zeroize::Zeroizing::new(vec![0; 32]))
        .unwrap();
    let prepared = encrypt(&key, &context, b"synthetic exact bytes").unwrap();
    let page = serde_json::to_vec(&json!({
        "changes": [{"sequence":1,"revisionId":context.revision_id,"itemId":context.item_id,
            "blob":prepared.blob_id(),"deleted":false,"parents":[],"device":scope.device_id()}],
        "cursor":1,"highWater":1,"hasMore":false,"checkpoint":0
    }))
    .unwrap();
    let mut store = SyncStore::attach(&mut db, scope.clone()).unwrap();
    store
        .stage_changes(&scope, 200, 0, &page, &[prepared])
        .unwrap();
    let denied: Result<_> = store.mark_applied_guarded(
        1,
        |tx| {
            tx.execute("INSERT INTO owner_receipts VALUES('provisional')", [])?;
            Ok(())
        },
        |tx| {
            let pending: u64 =
                tx.query_row("SELECT count(*) FROM owner_receipts", [], |row| row.get(0))?;
            assert_eq!(pending, 1, "final fence must see provisional owner writes");
            Err(SyncError::Scope)
        },
    );
    assert!(matches!(denied, Err(SyncError::Scope)));
    assert_eq!(store.cursors().unwrap().applied, 0);
    drop(store);
    drop(db);
    let mut db = Connection::open(&path).unwrap();
    let count: u64 = db
        .query_row("SELECT count(*) FROM owner_receipts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let mut store = SyncStore::attach(&mut db, scope.clone()).unwrap();
    let committed: Result<_> = store.mark_applied_guarded(
        1,
        |tx| {
            tx.execute("INSERT INTO owner_receipts VALUES('committed')", [])?;
            Ok(())
        },
        |_| Ok(()),
    );
    assert_eq!(committed.unwrap().applied, 1);
    // Even an already-applied replay must not bypass the current authority fence.
    let replay: Result<_> = store.mark_applied_guarded(
        1,
        |_| panic!("replay must not republish"),
        |_| Err(SyncError::Scope),
    );
    assert!(matches!(replay, Err(SyncError::Scope)));
    assert_eq!(store.cursors().unwrap().applied, 1);
    assert_eq!(store.cursors().unwrap().server_acknowledged, 0);
}
