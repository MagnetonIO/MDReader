use mdreader_provider::*;
fn binding(library: &str) -> SourceBinding {
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    SourceBinding::new(
        LibraryId::new(library).unwrap(),
        SourceId::new("source").unwrap(),
        AccountBinding::new("account").unwrap(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap()
}
fn version() -> DocumentVersion {
    DocumentVersion {
        provider_revision: OpaqueValue::new("v1").unwrap(),
        content_hash: None,
        conditional_write_token: Some(
            OpaqueValue::new("private-validator-never-returned").unwrap(),
        ),
    }
}
#[test]
fn readonly_recovery_reopens_exact_drafts_scopes_library_and_does_not_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("owned.sqlite");
    let mut c = rusqlite::Connection::open(&path).unwrap();
    let b = binding("library");
    let text = MarkdownBytes::new(b"# intended\r\npublic \xF0\x9F\x93\x9D".to_vec()).unwrap();
    let id = journal::prepare(
        &mut c,
        &b,
        &RecordKey::new("selected").unwrap(),
        &version(),
        &text,
    )
    .unwrap();
    let p = journal::begin_dispatch(&mut c, &b, id).unwrap();
    drop(p);
    journal::mark_unknown(&mut c, &b, id).unwrap();
    let foreign = binding("foreign-library");
    let foreign_id = journal::prepare(
        &mut c,
        &foreign,
        &RecordKey::new("selected").unwrap(),
        &version(),
        &text,
    )
    .unwrap();
    drop(c);
    let c = rusqlite::Connection::open(&path).unwrap();
    let changes = c.total_changes();
    let page = journal::draft_page(&c, b.library_id(), 0, 1).unwrap();
    assert_eq!(page.drafts.len(), 1);
    assert_eq!(page.next_offset, None);
    assert_eq!(page.drafts[0].operation_id, id);
    assert_eq!(page.drafts[0].state, journal::MutationState::Unknown);
    let (_, read) =
        journal::read_draft(&c, b.library_id(), id, body_hash(text.as_bytes())).unwrap();
    assert_eq!(read, text);
    assert!(matches!(
        journal::read_draft(&c, b.library_id(), foreign_id, body_hash(text.as_bytes())),
        Err(ProviderError::Unauthorized)
    ));
    assert!(matches!(
        journal::read_draft(&c, b.library_id(), id, [0; 32]),
        Err(ProviderError::Conflict)
    ));
    assert_eq!(c.total_changes(), changes);
    assert_eq!(
        journal::inspect(&c, &b, id).unwrap().unwrap().state,
        journal::MutationState::Unknown
    );
}
#[test]
fn prior_schema_and_empty_reads_remain_unchanged_and_body_corruption_is_refused() {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let b = binding("library");
    assert!(
        journal::draft_page(&c, b.library_id(), 0, 32)
            .unwrap()
            .drafts
            .is_empty()
    );
    let tables: u32 = c
        .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tables, 0);
    let text = MarkdownBytes::new(b"draft".to_vec()).unwrap();
    let id = journal::prepare(
        &mut c,
        &b,
        &RecordKey::new("selected").unwrap(),
        &version(),
        &text,
    )
    .unwrap();
    c.execute_batch(
        "DROP TABLE mdreader_provider_conflicts;UPDATE mdreader_provider_meta SET version=1;",
    )
    .unwrap();
    let changes = c.total_changes();
    assert_eq!(
        journal::draft_page(&c, b.library_id(), 0, 32)
            .unwrap()
            .drafts[0]
            .operation_id,
        id
    );
    assert_eq!(
        journal::read_draft(&c, b.library_id(), id, body_hash(text.as_bytes()))
            .unwrap()
            .1,
        text
    );
    assert_eq!(c.total_changes(), changes);
    assert_eq!(
        c.query_row("SELECT version FROM mdreader_provider_meta", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        1
    );
    c.execute(
        "UPDATE mdreader_provider_mutations SET body=?1",
        [b"other".as_slice()],
    )
    .unwrap();
    assert!(matches!(
        journal::read_draft(&c, b.library_id(), id, body_hash(text.as_bytes())),
        Err(ProviderError::Persistence)
    ));
    c.execute_batch("CREATE TEMP TABLE mdreader_provider_mutations(value);")
        .unwrap();
    assert!(matches!(
        journal::draft_page(&c, b.library_id(), 0, 32),
        Err(ProviderError::Persistence)
    ));
}
#[test]
fn bounded_pages_never_load_all_draft_bodies_and_exact_read_checks_cap_and_digest() {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let b = binding("library");
    let key = RecordKey::new("selected").unwrap();
    let text = MarkdownBytes::new(vec![b'a'; MAX_BODY_BYTES]).unwrap();
    let id = journal::prepare(&mut c, &b, &key, &version(), &text).unwrap();
    for _ in 0..33 {
        journal::prepare(
            &mut c,
            &b,
            &key,
            &version(),
            &MarkdownBytes::new(b"tiny".to_vec()).unwrap(),
        )
        .unwrap();
    }
    let first = journal::draft_page(&c, b.library_id(), 0, 32).unwrap();
    assert_eq!(first.drafts.len(), 32);
    assert_eq!(first.next_offset, Some(32));
    let next = journal::draft_page(&c, b.library_id(), 32, 32).unwrap();
    assert_eq!(next.drafts.len(), 2);
    assert_eq!(next.next_offset, None);
    assert_eq!(next.drafts[1].byte_length, MAX_BODY_BYTES);
    assert_eq!(
        journal::read_draft(&c, b.library_id(), id, body_hash(text.as_bytes()))
            .unwrap()
            .1,
        text
    );
    assert!(matches!(
        journal::draft_page(&c, b.library_id(), 0, 33),
        Err(ProviderError::Invalid)
    ));
    c.execute(
        "UPDATE mdreader_provider_mutations SET body=zeroblob(?1) WHERE operation_id=?2",
        rusqlite::params![MAX_BODY_BYTES + 1, id.to_string()],
    )
    .unwrap();
    assert!(matches!(
        journal::read_draft(&c, b.library_id(), id, body_hash(text.as_bytes())),
        Err(ProviderError::TooLarge)
    ));
}
