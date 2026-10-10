use mdreader_provider::{creation::*, *};
fn public_binding() -> SourceBinding {
    let scope = SelectedScope::ScopedRoot {
        root: RecordKey::new("/").unwrap(),
        recursive: true,
    };
    SourceBinding::new(
        LibraryId::new("public-library").unwrap(),
        SourceId::new("public-source").unwrap(),
        AccountBinding::new("public-account").unwrap(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap()
}
#[test]
fn creation_reopen_preserves_single_dispatch_and_rejects_changed_intent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("public.sqlite");
    let mut c = rusqlite::Connection::open(&path).unwrap();
    let binding = public_binding();
    let id = uuid::Uuid::new_v4();
    let body = MarkdownBytes::new(b"# Public check\n".to_vec()).unwrap();
    prepare(&mut c, &binding, id, "Public check.md", &body).unwrap();
    let permit = dispatch(&mut c, &binding, id).unwrap();
    permit.validate(&binding, &body).unwrap();
    assert!(
        permit
            .validate(&binding, &MarkdownBytes::new(b"changed".to_vec()).unwrap())
            .is_err()
    );
    assert!(permit.validate(&public_binding(), &body).is_err());
    drop(permit);
    drop(c);
    let mut c = rusqlite::Connection::open(&path).unwrap();
    initialize(&mut c).unwrap();
    prepare(&mut c, &binding, id, "Public check.md", &body).unwrap();
    assert!(
        matches!(dispatch(&mut c,&binding,id),Err(ProviderError::UnknownWrite { operation_id }) if operation_id==id)
    );
    uncertain(&c, &binding, id).unwrap();
    assert!(
        matches!(prepare(&mut c,&binding,uuid::Uuid::new_v4(),"PUBLIC check.md",&body),Err(ProviderError::UnknownWrite {operation_id}) if operation_id==id)
    );
    assert_eq!(
        load(&c, &binding, id).unwrap().unwrap().state,
        journal::MutationState::Unknown
    );
    assert_eq!(
        prepare(&mut c, &binding, id, "Different.md", &body),
        Err(ProviderError::Conflict)
    );
    assert!(load(&c, &public_binding(), id).unwrap().is_none());
}
#[test]
fn accepts_exact_receipt_and_publishes_replayable_projection() {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let binding = public_binding();
    let id = uuid::Uuid::new_v4();
    let body = MarkdownBytes::new(b"# Public check\n".to_vec()).unwrap();
    prepare(&mut c, &binding, id, "Public check.md", &body).unwrap();
    dispatch(&mut c, &binding, id).unwrap();
    let mut metadata = Metadata {
        membership_ancestors: None,
        key: RecordKey::new("public-key").unwrap(),
        name: DisplayText::new("Public check.md").unwrap(),
        display_path: DisplayText::new("/Public check.md").unwrap(),
        media_type: None,
        modified_at_ms: Some(1),
        size_bytes: Some(body.as_bytes().len() as u64),
        version: DocumentVersion {
            provider_revision: OpaqueValue::new("rev1").unwrap(),
            content_hash: None,
            conditional_write_token: Some(OpaqueValue::new("token1").unwrap()),
        },
    };
    metadata.size_bytes = Some(1);
    assert_eq!(
        accept(&c, &binding, id, &metadata),
        Err(ProviderError::Protocol)
    );
    metadata.size_bytes = Some(body.as_bytes().len() as u64);
    accept(&c, &binding, id, &metadata).unwrap();
    published(&c, &binding, id, "{\"public\":true}").unwrap();
    let pending = load(&c, &binding, id).unwrap().unwrap();
    assert_eq!(pending.state, journal::MutationState::Published);
    assert_eq!(pending.projection.as_deref(), Some("{\"public\":true}"));
    assert!(dispatch(&mut c, &binding, id).is_err());
}
#[test]
fn filenames_cannot_escape_the_saved_folder() {
    for name in [
        "../bad.md",
        "a/b.md",
        "a\\b.md",
        ".md",
        " note.md",
        "a.md\n",
        "note.txt",
        "C:note.md",
    ] {
        assert!(validate_filename(name).is_err(), "{name:?}");
    }
    for name in ["A new note.md", "Résumé.markdown", "PUBLIC.MD"] {
        validate_filename(name).unwrap();
    }
}
#[test]
fn reopened_unknown_empty_create_accepts_exact_read_proof_without_granting_write_token() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("public.sqlite");
    let binding = public_binding();
    let id = uuid::Uuid::new_v4();
    let body = MarkdownBytes::new(vec![]).unwrap();
    let mut c = rusqlite::Connection::open(&path).unwrap();
    prepare(&mut c, &binding, id, "Empty.md", &body).unwrap();
    dispatch(&mut c, &binding, id).unwrap();
    drop(c);
    let mut c = rusqlite::Connection::open(&path).unwrap();
    recover_interrupted(&c, &binding).unwrap();
    let mut metadata = Metadata {
        membership_ancestors: None,
        key: RecordKey::new("public-empty").unwrap(),
        name: DisplayText::new("Empty.md").unwrap(),
        display_path: DisplayText::new("/Empty.md").unwrap(),
        media_type: None,
        modified_at_ms: Some(1),
        size_bytes: Some(0),
        version: DocumentVersion {
            provider_revision: OpaqueValue::new("v1").unwrap(),
            content_hash: None,
            conditional_write_token: None,
        },
    };
    metadata.size_bytes = Some(1);
    assert!(accept_recovered(&c, &binding, id, &metadata).is_err());
    assert_eq!(
        load(&c, &binding, id).unwrap().unwrap().state,
        journal::MutationState::Unknown
    );
    metadata.size_bytes = Some(0);
    assert!(accept_recovered(&c, &public_binding(), id, &metadata).is_err());
    accept_recovered(&c, &binding, id, &metadata).unwrap();
    published(&c, &binding, id, "{\"public\":true}").unwrap();
    assert!(dispatch(&mut c, &binding, id).is_err());
    let retained = load(&c, &binding, id).unwrap().unwrap();
    assert_eq!(retained.body, body);
    assert_eq!(
        retained.metadata.unwrap().version.conditional_write_token,
        None
    );
}
