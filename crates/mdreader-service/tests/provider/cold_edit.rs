use super::*;
#[tokio::test]
async fn cold_exact_read_validator_edit_reopens_without_losing_discovery_progress() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owner.sqlite");
    let mut c = rusqlite::Connection::open(&path).unwrap();
    let d = descriptor_with_library(provider::library_id(&mut c).unwrap());
    provider::admit(
        &mut c,
        &d,
        &OpaqueValue::new("public-credential-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    Projection::install(&c);
    let projection = Projection::default();
    let snapshot = provider::begin_snapshot(&mut c, d.binding(), Some(&cursor("before"))).unwrap();
    let mut listed = row("v1", "/Plan.md", b"old");
    listed.version.conditional_write_token = None;
    provider::stage_page(
        &mut c,
        &snapshot,
        None,
        &MetadataPage::new(
            vec![listed.clone()],
            Some(cursor("continue")),
            1,
            Coverage::Partial {
                reason: DisplayText::new("Loading").unwrap(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    // Metadata is not a body grant. An exact native read supplies its validator.
    let old = document("v1", b"old");
    provider::cache_body(&mut c, d.binding(), &old).unwrap();
    let admitted = provider::admit_body(&mut c, d.binding(), &old, &projection).unwrap();
    assert!(provider::metadata(&mut c, d.binding(), &key()).is_err());
    let op = provider::prepare_write(
        &mut c,
        d.binding(),
        &key(),
        &old.version,
        &MarkdownBytes::new(b"desired".to_vec()).unwrap(),
    )
    .unwrap();
    let owner = Arc::new(Mutex::new(c));
    let p = Synthetic::new(d.clone(), owner.clone(), old);
    let result = provider::dispatch_write(owner.as_ref(), &p, op, &projection, &context())
        .await
        .unwrap();
    assert_ne!(result.revision, admitted.revision);
    assert_eq!(p.writes.load(Ordering::SeqCst), 1);
    let stale = owner.with_connection(|c| {
        provider::stage_page(
            c,
            &snapshot,
            Some(&cursor("continue")),
            &MetadataPage::new(
                vec![listed],
                None,
                2,
                Coverage::MetadataCompleteSelected { observed_at_ms: 2 },
            )?,
        )
    });
    assert_eq!(stale, Err(ProviderError::Conflict));
    drop(p);
    drop(owner);
    let mut reopened = rusqlite::Connection::open(&path).unwrap();
    let resumed = provider::resume(&mut reopened, d.binding())
        .unwrap()
        .unwrap();
    assert_eq!(resumed.snapshot.id(), snapshot.id());
    assert_eq!(resumed.list_cursor, Some(cursor("continue")));
    assert!(
        provider::checkpoint(&mut reopened, d.binding())
            .unwrap()
            .is_none()
    );
    let current = provider::cached(&mut reopened, d.binding(), &key())
        .unwrap()
        .unwrap();
    assert_eq!(current.document.body.as_bytes(), b"desired");
    assert_eq!(current.projection, Some(result));
    assert!(matches!(
        provider::list(&mut reopened, d.binding().library_id()).unwrap()[0].coverage,
        Coverage::NeverIndexed
    ));
    provider::stage_page(
        &mut reopened,
        &resumed.snapshot,
        Some(&cursor("continue")),
        &MetadataPage::new(
            vec![],
            None,
            2,
            Coverage::MetadataCompleteSelected { observed_at_ms: 2 },
        )
        .unwrap(),
    )
    .unwrap();
    provider::stage_changes(
        &mut reopened,
        &resumed.snapshot,
        &cursor("before"),
        &[],
        None,
        Some(&cursor("after")),
        3,
    )
    .unwrap();
    provider::publish_snapshot(&mut reopened, &resumed.snapshot, &projection).unwrap();
    assert_eq!(
        provider::metadata(&mut reopened, d.binding(), &key())
            .unwrap()
            .version,
        current.document.version
    );
    assert!(
        provider::cached(&mut reopened, d.binding(), &key())
            .unwrap()
            .is_some()
    );
    // An older admitted version and revoked grant cannot start another intent.
    assert_eq!(
        provider::prepare_write(
            &mut reopened,
            d.binding(),
            &key(),
            &version("v1", b"old"),
            &MarkdownBytes::new(b"stale".to_vec()).unwrap()
        ),
        Err(ProviderError::Conflict)
    );
    provider::disable(&mut reopened, d.binding(), &projection).unwrap();
    assert_eq!(
        provider::prepare_write(
            &mut reopened,
            d.binding(),
            &key(),
            &current.document.version,
            &MarkdownBytes::new(b"blocked".to_vec()).unwrap()
        ),
        Err(ProviderError::Unauthorized)
    );
}
#[test]
fn read_validator_refinement_cannot_replace_a_revision_or_change_exact_cached_bytes() {
    let (mut c, d) = setup();
    let snapshot = provider::begin_snapshot(&mut c, d.binding(), Some(&cursor("before"))).unwrap();
    let mut listed = row("v1", "/Plan.md", b"old");
    listed.version.conditional_write_token = None;
    provider::stage_page(
        &mut c,
        &snapshot,
        None,
        &MetadataPage::new(
            vec![listed.clone()],
            Some(cursor("next")),
            1,
            Coverage::NeverIndexed,
        )
        .unwrap(),
    )
    .unwrap();
    let mut first = document("v1", b"old");
    first.version.conditional_write_token = None;
    provider::cache_body(&mut c, d.binding(), &first).unwrap();
    let mut wrong = first.clone();
    wrong.version.conditional_write_token = Some(OpaqueValue::new("etag").unwrap());
    wrong.body = MarkdownBytes::new(b"bad".to_vec()).unwrap();
    assert_eq!(
        provider::cache_body(&mut c, d.binding(), &wrong),
        Err(ProviderError::Conflict)
    );
    wrong = document("v2", b"old");
    assert_eq!(
        provider::cache_body(&mut c, d.binding(), &wrong),
        Err(ProviderError::Conflict)
    );
    let retained = provider::cached(&mut c, d.binding(), &key())
        .unwrap()
        .unwrap();
    assert_eq!(retained.document.version, first.version);
    assert_eq!(retained.document.body, first.body);
}

#[test]
fn created_cold_row_projection_and_intent_commit_atomically_with_reopen_and_revocation_fences() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("create.sqlite");
    let mut c = rusqlite::Connection::open(&path).unwrap();
    let scope = SelectedScope::ScopedRoot {
        root: RecordKey::new("/plans").unwrap(),
        recursive: true,
    };
    let d = SourceDescriptor::new(
        SourceBinding::new(
            provider::library_id(&mut c).unwrap(),
            SourceId::new("created-source").unwrap(),
            AccountBinding::new("public-account").unwrap(),
            uuid::Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::Dropbox,
        DisplayText::new("Plans").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: true,
            assets: false,
            guarded_write: true,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    provider::admit(
        &mut c,
        &d,
        &OpaqueValue::new("public-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    Projection::install(&c);
    let projection = Projection::default();
    let original = provider::begin_snapshot(&mut c, d.binding(), Some(&cursor("before"))).unwrap();
    let body = MarkdownBytes::new(b"created".to_vec()).unwrap();
    let id = uuid::Uuid::new_v4();
    creation::prepare(&mut c, d.binding(), id, "Plan.md", &body).unwrap();
    creation::dispatch(&mut c, d.binding(), id).unwrap();
    let metadata = row("created", "/plans/Plan.md", body.as_bytes());
    creation::accept(&c, d.binding(), id, &metadata).unwrap();
    projection.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        provider::publish_created(&mut c, d.binding(), id, &projection, 1),
        Err(ProviderError::Unsupported)
    );
    let count: u64 = c
        .query_row(
            "SELECT count(*) FROM mdreader_provider_registry_rows WHERE snapshot=?1",
            [original.id().to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        creation::load(&c, d.binding(), id).unwrap().unwrap().state,
        journal::MutationState::Accepted
    );
    projection.fail.store(false, Ordering::SeqCst);
    let receipt = provider::publish_created(&mut c, d.binding(), id, &projection, 1).unwrap();
    assert_eq!(
        provider::publish_created(&mut c, d.binding(), id, &projection, 2).unwrap(),
        receipt
    );
    assert_eq!(
        creation::load(&c, d.binding(), id).unwrap().unwrap().state,
        journal::MutationState::Published
    );
    assert_eq!(
        provider::stage_page(
            &mut c,
            &original,
            None,
            &MetadataPage::new(vec![], None, 2, Coverage::NeverIndexed).unwrap()
        ),
        Err(ProviderError::Conflict)
    );
    drop(c);
    let mut c = rusqlite::Connection::open(&path).unwrap();
    let resumed = provider::resume(&mut c, d.binding()).unwrap().unwrap();
    assert_eq!(resumed.snapshot.id(), original.id());
    assert!(resumed.list_cursor.is_none());
    assert!(provider::checkpoint(&mut c, d.binding()).unwrap().is_none());
    let cached = provider::cached(&mut c, d.binding(), &key())
        .unwrap()
        .unwrap();
    assert_eq!(cached.projection, Some(receipt));
    assert_eq!(cached.document.body, body);
    // Revocation forbids local repair even with an accepted durable receipt.
    let second = uuid::Uuid::new_v4();
    creation::prepare(&mut c, d.binding(), second, "Plan.md", &body).unwrap();
    creation::dispatch(&mut c, d.binding(), second).unwrap();
    creation::accept(&c, d.binding(), second, &metadata).unwrap();
    provider::disable(&mut c, d.binding(), &projection).unwrap();
    assert_eq!(
        provider::publish_created(&mut c, d.binding(), second, &projection, 3),
        Err(ProviderError::Unauthorized)
    );
}

#[tokio::test]
async fn explicit_current_read_admits_new_head_without_dispatching_stale_prepared_save() {
    let (mut c, d) = setup();
    Projection::install(&c);
    let snapshot = provider::begin_snapshot(&mut c, d.binding(), Some(&cursor("before"))).unwrap();
    provider::stage_page(
        &mut c,
        &snapshot,
        None,
        &MetadataPage::new(
            vec![row("v1", "/Plan.md", b"old")],
            Some(cursor("next")),
            1,
            Coverage::NeverIndexed,
        )
        .unwrap(),
    )
    .unwrap();
    let old = document("v1", b"old");
    provider::cache_body(&mut c, d.binding(), &old).unwrap();
    let operation = provider::prepare_write(
        &mut c,
        d.binding(),
        &key(),
        &old.version,
        &MarkdownBytes::new(b"draft".to_vec()).unwrap(),
    )
    .unwrap();
    let owner = Arc::new(Mutex::new(c));
    let p = Synthetic::new(d.clone(), owner.clone(), document("v2", b"new"));
    let projection = Projection::default();
    let receipt = provider::fetch_and_admit(owner.as_ref(), &p, &key(), &projection, &context())
        .await
        .unwrap();
    let current = owner
        .with_connection(|c| provider::cached(c, d.binding(), &key()))
        .unwrap()
        .unwrap();
    assert_eq!(current.document.version, version("v2", b"new"));
    assert_eq!(current.document.body.as_bytes(), b"new");
    assert_eq!(current.projection, Some(receipt));
    assert_eq!(
        owner
            .with_connection(|c| Ok(provider::resume(c, d.binding())?.unwrap().list_cursor))
            .unwrap(),
        Some(cursor("next"))
    );
    assert_eq!(
        provider::dispatch_write(owner.as_ref(), &p, operation, &projection, &context()).await,
        Err(ProviderError::Conflict)
    );
    assert_eq!(p.writes.load(Ordering::SeqCst), 0);
    let pending = owner
        .with_connection(|c| journal::load(c, d.binding(), operation))
        .unwrap();
    assert_eq!(pending.expected, old.version);
    assert_eq!(pending.state, journal::MutationState::Prepared);
    assert_eq!(pending.body.as_bytes(), b"draft");
}
