use super::*;
#[test]
fn local_draft_read_survives_disabled_and_replaced_grant_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("public.sqlite");
    let mut c = Connection::open(&path).unwrap();
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    let d = SourceDescriptor::new(
        SourceBinding::new(
            library_id(&mut c).unwrap(),
            SourceId::new("source").unwrap(),
            AccountBinding::new("public-account").unwrap(),
            Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Public cloud source").unwrap(),
        scope.clone(),
        ProviderCapabilities {
            read: true,
            list: true,
            changes: false,
            guarded_write: true,
            assets: false,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    admit(
        &mut c,
        &d,
        &OpaqueValue::new("public-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    let body = MarkdownBytes::new(b"# durable intended text".to_vec()).unwrap();
    let key = RecordKey::new("selected").unwrap();
    let expected = DocumentVersion {
        provider_revision: OpaqueValue::new("v1").unwrap(),
        content_hash: None,
        conditional_write_token: Some(OpaqueValue::new("private-token").unwrap()),
    };
    let id = journal::prepare(&mut c, d.binding(), &key, &expected, &body).unwrap();
    disable(&mut c, d.binding(), &UnsupportedProjection).unwrap();
    let changes = c.total_changes();
    let listed = page(&c, 0, 32).unwrap();
    assert_eq!(listed.drafts[0].source_name.as_str(), "Public cloud source");
    let revision = listed.drafts[0].revision.clone();
    let recovered = read(&c, id, &revision).unwrap();
    assert_eq!(recovered.markdown, "# durable intended text");
    assert_eq!(c.total_changes(), changes);
    let fresh = SourceDescriptor::new(
        SourceBinding::new(
            d.binding().library_id().clone(),
            d.binding().source_id().clone(),
            AccountBinding::new("new-account").unwrap(),
            Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        d.kind(),
        DisplayText::new("New connection").unwrap(),
        scope,
        d.capabilities(),
        d.health(),
        Coverage::NeverIndexed,
    )
    .unwrap();
    reconnect(
        &mut c,
        &fresh,
        &OpaqueValue::new("new-public-reference").unwrap(),
        [8; 32],
    )
    .unwrap();
    drop(c);
    let c = Connection::open(&path).unwrap();
    let changes = c.total_changes();
    let recovered = read(&c, id, &revision).unwrap();
    assert_eq!(
        recovered.draft.source_name.as_str(),
        "Previous cloud connection"
    );
    assert_eq!(recovered.draft.title.as_str(), "Recovered cloud draft.md");
    assert_eq!(recovered.markdown, "# durable intended text");
    assert_eq!(
        journal::inspect(&c, d.binding(), id)
            .unwrap()
            .unwrap()
            .state,
        journal::MutationState::Prepared
    );
    assert_eq!(c.total_changes(), changes);
    let transport = serde_json::to_string(&recovered).unwrap();
    assert!(!transport.contains("private-token"));
    assert!(!transport.contains("public-account"));
    assert!(!transport.contains("selected"));
}
