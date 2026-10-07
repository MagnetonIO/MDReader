use mdreader_provider::*;
fn binding() -> SourceBinding {
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    SourceBinding::new(
        LibraryId::new("library").unwrap(),
        SourceId::new("source").unwrap(),
        AccountBinding::new("account").unwrap(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap()
}
#[test]
fn committed_mutation_intent_survives_reopen_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("owner.sqlite");
    let mut connection = rusqlite::Connection::open(&file).unwrap();
    let binding = binding();
    let key = RecordKey::new("selected").unwrap();
    let expected = DocumentVersion {
        provider_revision: OpaqueValue::new("provider-v1").unwrap(),
        content_hash: None,
        conditional_write_token: Some(OpaqueValue::new("http-v1").unwrap()),
    };
    let body = MarkdownBytes::new(b"# intended".to_vec()).unwrap();
    assert!(journal::prepare(&mut connection, &binding, &key, &expected, &body).is_ok());
}
fn version() -> DocumentVersion {
    DocumentVersion {
        provider_revision: OpaqueValue::new("provider-v1").unwrap(),
        content_hash: None,
        conditional_write_token: Some(OpaqueValue::new("http-v1").unwrap()),
    }
}
#[test]
fn interrupted_dispatch_is_uncertain_and_never_reissued_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("owner.sqlite");
    let mut db = rusqlite::Connection::open(&file).unwrap();
    let binding = binding();
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(b"intended".to_vec()).unwrap();
    let id = journal::prepare(&mut db, &binding, &key, &version(), &body).unwrap();
    assert!(journal::mark_published(&mut db, &binding, id).is_err());
    let permit = journal::begin_dispatch(&mut db, &binding, id).unwrap();
    assert_eq!(
        permit.expected().provider_revision,
        version().provider_revision
    );
    assert_eq!(
        permit.expected().conditional_write_token,
        version().conditional_write_token
    );
    permit.validate(&binding, &body).unwrap();
    assert!(permit
        .validate(&binding, &MarkdownBytes::new(b"other".to_vec()).unwrap())
        .is_err());
    drop(permit);
    drop(db);
    let mut db = rusqlite::Connection::open(&file).unwrap();
    assert_eq!(journal::recover_interrupted(&mut db, &binding).unwrap(), 1);
    assert_eq!(
        journal::load(&mut db, &binding, id).unwrap().state,
        journal::MutationState::Unknown
    );
    assert!(
        matches!(journal::begin_dispatch(&mut db,&binding,id),Err(ProviderError::UnknownWrite{operation_id})if operation_id==id)
    );
    let observed = DocumentBody {
        key: key.clone(),
        version: DocumentVersion {
            provider_revision: OpaqueValue::new("provider-v2").unwrap(),
            ..version()
        },
        body: body.clone(),
        observed_at_ms: 1,
    };
    journal::reconcile_read(&mut db, &binding, id, &observed).unwrap();
    assert_eq!(
        journal::load(&mut db, &binding, id).unwrap().state,
        journal::MutationState::Accepted
    );
    journal::mark_published(&mut db, &binding, id).unwrap();
    journal::mark_published(&mut db, &binding, id).unwrap();
    assert_eq!(journal::load(&mut db, &binding, id).unwrap().body, body);
    assert_eq!(
        journal::load(&mut db, &binding, id).unwrap().state,
        journal::MutationState::Published
    );
}
#[test]
fn wrong_binding_receipt_or_corrupt_intent_cannot_acknowledge() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let owner = binding();
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(b"intended".to_vec()).unwrap();
    let id = journal::prepare(&mut db, &owner, &key, &version(), &body).unwrap();
    journal::begin_dispatch(&mut db, &owner, id).unwrap();
    assert!(journal::load(&mut db, &binding(), id).is_err());
    let mut receipt = WriteReceipt {
        operation_id: id,
        key: key.clone(),
        version: DocumentVersion {
            provider_revision: OpaqueValue::new("provider-v2").unwrap(),
            ..version()
        },
        body_hash: body_hash(body.as_bytes()),
        size_bytes: body.as_bytes().len() as u64,
    };
    receipt.body_hash = [0; 32];
    assert!(journal::accept_receipt(&mut db, &owner, &receipt).is_err());
    assert_eq!(
        journal::load(&mut db, &owner, id).unwrap().state,
        journal::MutationState::Dispatched
    );
    receipt.body_hash = body_hash(body.as_bytes());
    journal::accept_receipt(&mut db, &owner, &receipt).unwrap();
    journal::accept_receipt(&mut db, &owner, &receipt).unwrap();
    db.execute(
        "UPDATE mdreader_provider_mutations SET record=record||' '",
        [],
    )
    .unwrap();
    assert!(journal::load(&mut db, &owner, id).is_err());
}
#[test]
fn conflicting_read_keeps_original_intent_and_never_blindly_retries() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let binding = binding();
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(b"original intended".to_vec()).unwrap();
    let id = journal::prepare(&mut db, &binding, &key, &version(), &body).unwrap();
    journal::begin_dispatch(&mut db, &binding, id).unwrap();
    journal::mark_unknown(&mut db, &binding, id).unwrap();
    let observed = DocumentBody {
        key,
        version: version(),
        body: MarkdownBytes::new(b"remote conflict".to_vec()).unwrap(),
        observed_at_ms: 1,
    };
    assert_eq!(
        journal::reconcile_read(&mut db, &binding, id, &observed),
        Err(ProviderError::Conflict)
    );
    assert_eq!(
        journal::load(&mut db, &binding, id).unwrap().state,
        journal::MutationState::Conflict
    );
    assert_eq!(journal::load(&mut db, &binding, id).unwrap().body, body);
    assert!(journal::begin_dispatch(&mut db, &binding, id).is_err());
}
#[test]
fn schema_guard_preserves_owner_version_and_refuses_unsafe_durability() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    db.pragma_update(None, "user_version", 42).unwrap();
    let binding = binding();
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(vec![]).unwrap();
    journal::prepare(&mut db, &binding, &key, &version(), &body).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u64>(0))
            .unwrap(),
        42
    );
    db.execute("UPDATE mdreader_provider_meta SET version=99", [])
        .unwrap();
    assert!(journal::prepare(&mut db, &binding, &key, &version(), &body).is_err());
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    db.pragma_update(None, "synchronous", 0).unwrap();
    assert!(journal::prepare(&mut db, &binding, &key, &version(), &body).is_err());
}
#[test]
fn conflicting_remote_body_is_durable_separately_from_intended_body() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("owner.sqlite");
    let mut db = rusqlite::Connection::open(&file).unwrap();
    let binding = binding();
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(b"intended version".to_vec()).unwrap();
    let id = journal::prepare(&mut db, &binding, &key, &version(), &body).unwrap();
    journal::begin_dispatch(&mut db, &binding, id).unwrap();
    let observed = DocumentBody {
        key,
        version: version(),
        body: MarkdownBytes::new(b"remote conflict version".to_vec()).unwrap(),
        observed_at_ms: 1,
    };
    assert_eq!(
        journal::reconcile_read(&mut db, &binding, id, &observed),
        Err(ProviderError::Conflict)
    );
    drop(db);
    let mut db = rusqlite::Connection::open(&file).unwrap();
    let retained = journal::conflict_body(&mut db, &binding, id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.body, observed.body);
    assert_eq!(journal::load(&mut db, &binding, id).unwrap().body, body);
}
