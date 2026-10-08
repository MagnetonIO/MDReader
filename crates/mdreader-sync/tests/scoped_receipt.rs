use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mdreader_sync::*;
use serde_json::json;
#[test]
fn new_transport_refuses_weak_and_cross_operation_upload_receipts() {
    let temp = tempfile::TempDir::new().unwrap();
    let mut db = rusqlite::Connection::open(temp.path().join("synthetic.sqlite")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL")
        .unwrap();
    migrate(&mut db).unwrap();
    let scope = SyncScope::new(
        "a".repeat(64),
        uuid::Uuid::new_v4().to_string(),
        URL_SAFE_NO_PAD.encode([1; 32]),
    )
    .unwrap();
    let mut store = SyncStore::attach(&mut db, scope.clone()).unwrap();
    let key = LibraryKey::generate().unwrap();
    let context = EncryptionContext {
        library_id: scope.library_id().into(),
        item_id: uuid::Uuid::new_v4().to_string(),
        revision_id: uuid::Uuid::new_v4().to_string(),
        key_id: key.key_id().into(),
    };
    let op = uuid::Uuid::new_v4().to_string();
    let entry = store
        .seal_enqueue(&op, &[], &key, &context, b"synthetic")
        .unwrap();
    let weak =
        json!({"blob":entry.operation.blob,"bytes":entry.envelope_utf8.as_ref().unwrap().len()});
    assert!(store
        .mark_uploaded_scoped(&scope, &op, 201, &serde_json::to_vec(&weak).unwrap())
        .is_err());
    assert_eq!(
        store.entry(&op).unwrap().unwrap().state,
        OutboxState::Pending
    );
    let mut strong = json!({"v":2,"account":scope.account_digest(),"libraryId":scope.library_id(),"device":scope.device_id(),"operationId":uuid::Uuid::new_v4().to_string(),"blob":entry.operation.blob,"bytes":entry.envelope_utf8.as_ref().unwrap().len()});
    assert!(store
        .mark_uploaded_scoped(&scope, &op, 201, &serde_json::to_vec(&strong).unwrap())
        .is_err());
    strong["operationId"] = json!(op);
    for (field, value) in [
        ("v", json!(1)),
        ("account", json!("b".repeat(64))),
        ("libraryId", json!(uuid::Uuid::new_v4().to_string())),
        ("device", json!(URL_SAFE_NO_PAD.encode([2; 32]))),
        ("blob", json!("b".repeat(64))),
        ("bytes", json!(1)),
        ("unknown", json!(true)),
    ] {
        let mut bad = strong.clone();
        bad[field] = value;
        assert!(
            store
                .mark_uploaded_scoped(&scope, &op, 201, &serde_json::to_vec(&bad).unwrap())
                .is_err(),
            "{field}"
        );
        assert_eq!(
            store.entry(&op).unwrap().unwrap().state,
            OutboxState::Pending
        );
    }
    store
        .mark_uploaded_scoped(&scope, &op, 201, &serde_json::to_vec(&strong).unwrap())
        .unwrap();
    assert_eq!(
        store.entry(&op).unwrap().unwrap().state,
        OutboxState::Uploaded
    );
}
