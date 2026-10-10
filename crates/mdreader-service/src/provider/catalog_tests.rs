use super::*;

fn recent_fixture() -> (Connection, SourceDescriptor) {
    let mut c = Connection::open_in_memory().unwrap();
    let scope = SelectedScope::exact(
        ["old-a", "new-z", "tie-a", "tie-b", "unknown", "latest"]
            .into_iter()
            .map(|s| RecordKey::new(s).unwrap())
            .collect(),
    )
    .unwrap();
    let d = SourceDescriptor::new(
        SourceBinding::new(
            library_id(&mut c).unwrap(),
            SourceId::new("recent-source").unwrap(),
            AccountBinding::new("synthetic-account").unwrap(),
            Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Recent notes").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: false,
            assets: false,
            guarded_write: false,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    admit(
        &mut c,
        &d,
        &OpaqueValue::new("synthetic-reference").unwrap(),
        [1; 32],
    )
    .unwrap();
    (c, d)
}
fn recent_row(key: &str, modified: Option<i64>) -> Metadata {
    Metadata {
        key: RecordKey::new(key).unwrap(),
        name: DisplayText::new(format!("{key}.md")).unwrap(),
        display_path: DisplayText::new(format!("/{key}.md")).unwrap(),
        media_type: None,
        modified_at_ms: modified,
        size_bytes: Some(6),
        version: DocumentVersion {
            provider_revision: OpaqueValue::new("v1").unwrap(),
            content_hash: None,
            conditional_write_token: None,
        },
    }
}
fn staged(
    c: &mut Connection,
    d: &SourceDescriptor,
    rows: Vec<Metadata>,
    complete: bool,
) -> Snapshot {
    let s = begin_snapshot(c, d.binding(), None).unwrap();
    stage_page(
        c,
        &s,
        None,
        &MetadataPage::new(
            rows,
            (!complete).then(|| ProviderCursor::new(b"next".to_vec()).unwrap()),
            1,
            if complete {
                Coverage::MetadataCompleteSelected { observed_at_ms: 1 }
            } else {
                Coverage::Partial {
                    reason: DisplayText::new("Loading").unwrap(),
                }
            },
        )
        .unwrap(),
    )
    .unwrap();
    s
}

#[test]
fn cold_rows_are_readable_recent_first_with_stable_ties_and_fenced_continuation() {
    let (mut c, d) = recent_fixture();
    let s = staged(
        &mut c,
        &d,
        vec![
            recent_row("old-a", Some(1)),
            recent_row("new-z", Some(30)),
            recent_row("tie-b", Some(20)),
            recent_row("tie-a", Some(20)),
            recent_row("unknown", None),
        ],
        false,
    );
    let source = d.binding().source_id();
    let first = catalog_page(&mut c, source, None, None, 2).unwrap();
    assert_eq!(
        first
            .documents
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["new-z.md", "tie-a.md"]
    );
    assert!(first
        .documents
        .iter()
        .all(|d| d.observation == "partial_provider_metadata"));
    let next = catalog_page(
        &mut c,
        source,
        first.next_after.as_deref(),
        Some(&first.snapshot),
        2,
    )
    .unwrap();
    assert_eq!(
        next.documents
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["tie-b.md", "old-a.md"]
    );
    let last = catalog_page(
        &mut c,
        source,
        next.next_after.as_deref(),
        Some(&next.snapshot),
        2,
    )
    .unwrap();
    assert_eq!(last.documents[0].name.as_str(), "unknown.md");
    assert!(last.next_after.is_none());
    let body = DocumentBody {
        key: RecordKey::new("new-z").unwrap(),
        version: recent_row("new-z", Some(30)).version,
        body: MarkdownBytes::new(b"public".to_vec()).unwrap(),
        observed_at_ms: 2,
    };
    cache_body(&mut c, d.binding(), &body).unwrap();
    assert!(cached(&mut c, d.binding(), &body.key).unwrap().is_some());
    // Partial observation does not bypass the published-metadata write gate.
    assert!(matches!(
        metadata(&mut c, d.binding(), &body.key),
        Err(ProviderError::Unavailable)
    ));
    let consumed = ProviderCursor::new(b"next".to_vec()).unwrap();
    stage_page(
        &mut c,
        &s,
        Some(&consumed),
        &MetadataPage::new(
            vec![recent_row("latest", Some(50))],
            None,
            2,
            Coverage::MetadataCompleteSelected { observed_at_ms: 2 },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        catalog_page(
            &mut c,
            source,
            first.next_after.as_deref(),
            Some(&first.snapshot),
            2
        ),
        Err(ProviderError::Conflict)
    ));
    assert_eq!(
        catalog_page(&mut c, source, None, None, 10)
            .unwrap()
            .documents[0]
            .name
            .as_str(),
        "latest.md"
    );
    discard_staging(&mut c, d.binding()).unwrap();
    assert!(matches!(
        cached(&mut c, d.binding(), &body.key),
        Err(ProviderError::Unavailable)
    ));
    assert!(catalog_page(&mut c, source, None, None, 10)
        .unwrap()
        .documents
        .is_empty());
}

#[test]
fn refresh_keeps_completed_snapshot_until_replacement_and_revocation_still_refuses() {
    let (mut c, d) = recent_fixture();
    let old = staged(&mut c, &d, vec![recent_row("old-a", Some(1))], true);
    publish_snapshot(&mut c, &old, &UnsupportedProjection).unwrap();
    let next = staged(&mut c, &d, vec![recent_row("new-z", Some(30))], true);
    assert_eq!(
        catalog_page(&mut c, d.binding().source_id(), None, None, 10)
            .unwrap()
            .documents[0]
            .name
            .as_str(),
        "old-a.md"
    );
    publish_snapshot(&mut c, &next, &UnsupportedProjection).unwrap();
    assert_eq!(
        catalog_page(&mut c, d.binding().source_id(), None, None, 10)
            .unwrap()
            .documents[0]
            .name
            .as_str(),
        "new-z.md"
    );
    disable(&mut c, d.binding(), &UnsupportedProjection).unwrap();
    assert!(matches!(
        catalog_page(&mut c, d.binding().source_id(), None, None, 10),
        Err(ProviderError::Unauthorized)
    ));
}
#[test]
fn final_fence_rejects_metadata_publication_after_rows_were_observed() {
    let mut c = Connection::open_in_memory().unwrap();
    let library = library_id(&mut c).unwrap();
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    let descriptor = SourceDescriptor::new(
        SourceBinding::new(
            library,
            SourceId::new("selected-source").unwrap(),
            AccountBinding::new("private-account").unwrap(),
            Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Selected").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: false,
            assets: false,
            guarded_write: false,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    admit(
        &mut c,
        &descriptor,
        &OpaqueValue::new("private-reference").unwrap(),
        [1; 32],
    )
    .unwrap();
    let publish = |c: &mut Connection| {
        let snapshot = begin_snapshot(c, descriptor.binding(), None)?;
        stage_page(
            c,
            &snapshot,
            None,
            &MetadataPage::new(
                vec![],
                None,
                1,
                Coverage::MetadataCompleteSelected { observed_at_ms: 1 },
            )?,
        )?;
        publish_snapshot(c, &snapshot, &UnsupportedProjection)
    };
    publish(&mut c).unwrap();
    let result = catalog_page_checked(
        &mut c,
        descriptor.binding().source_id(),
        None,
        None,
        100,
        publish,
    );
    assert!(matches!(result, Err(ProviderError::Conflict)));
    assert!(
        catalog_page(&mut c, descriptor.binding().source_id(), None, None, 100)
            .unwrap()
            .documents
            .is_empty()
    );
}
