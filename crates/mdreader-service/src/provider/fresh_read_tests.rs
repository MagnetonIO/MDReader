use super::super::super::{journal, UnsupportedProjection};
use super::*;

fn cursor(value: &str) -> ProviderCursor {
    ProviderCursor::new(value.as_bytes().to_vec()).unwrap()
}
fn row(key: &str, revision: &str, bytes: &[u8]) -> Metadata {
    Metadata {
        key: RecordKey::new(key).unwrap(),
        name: DisplayText::new(format!("{key}.md")).unwrap(),
        display_path: DisplayText::new(format!("/root/{key}.md")).unwrap(),
        membership_ancestors: Some(vec![RecordKey::new("actual-root").unwrap()]),
        media_type: None,
        modified_at_ms: Some(1),
        size_bytes: Some(bytes.len() as u64),
        version: DocumentVersion {
            provider_revision: OpaqueValue::new(revision).unwrap(),
            content_hash: Some(
                ContentHash::new(HashAlgorithm::Sha256, body_hash(bytes).to_vec()).unwrap(),
            ),
            conditional_write_token: Some(OpaqueValue::new(format!("etag-{revision}")).unwrap()),
        },
    }
}
fn body(row: &Metadata, bytes: &[u8]) -> DocumentBody {
    DocumentBody {
        key: row.key.clone(),
        version: row.version.clone(),
        body: MarkdownBytes::new(bytes.to_vec()).unwrap(),
        observed_at_ms: 2,
    }
}
fn setup(c: &mut Connection) -> SourceDescriptor {
    let scope = SelectedScope::DriveFolder {
        root: RecordKey::new("root").unwrap(),
        recursive: true,
    };
    let d = SourceDescriptor::new(
        SourceBinding::new(
            library_id(c).unwrap(),
            SourceId::new("source").unwrap(),
            AccountBinding::new("account").unwrap(),
            Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Public fixture").unwrap(),
        scope,
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
        c,
        &d,
        &OpaqueValue::new("public-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    d
}
fn seed(c: &mut Connection, d: &SourceDescriptor, complete: bool) -> Snapshot {
    let s = begin_snapshot(c, d.binding(), None).unwrap();
    stage_page(
        c,
        &s,
        None,
        &MetadataPage::new(
            vec![row("one", "v1", b"old"), row("two", "v1", b"two")],
            (!complete).then(|| cursor("next")),
            1,
            if complete {
                Coverage::MetadataCompleteSelected { observed_at_ms: 1 }
            } else {
                Coverage::NeverIndexed
            },
        )
        .unwrap(),
    )
    .unwrap();
    s
}
fn current(c: &Connection, d: &SourceDescriptor) -> Metadata {
    metadata_observation::metadata(c, d.binding(), &RecordKey::new("one").unwrap()).unwrap()
}
fn retained(c: &mut Connection, d: &SourceDescriptor) -> Vec<u8> {
    cached(c, d.binding(), &RecordKey::new("one").unwrap())
        .unwrap()
        .unwrap()
        .document
        .body
        .as_bytes()
        .to_vec()
}

#[test]
fn current_head_rebases_cold_progress_without_rewriting_prepared_intent() {
    let mut c = Connection::open_in_memory().unwrap();
    let d = setup(&mut c);
    let snapshot = seed(&mut c, &d, false);
    let old = current(&c, &d);
    cache_body(&mut c, d.binding(), &body(&old, b"old")).unwrap();
    let operation = prepare_write(
        &mut c,
        d.binding(),
        &old.key,
        &old.version,
        &MarkdownBytes::new(b"draft".to_vec()).unwrap(),
    )
    .unwrap();
    let head = capture(&mut c, d.binding(), &old.key).unwrap();
    let fresh = row("one", "v2", b"new");
    cache(&mut c, head, &fresh, &body(&fresh, b"new")).unwrap();
    assert_eq!(
        serde_json::to_value(current(&c, &d)).unwrap(),
        serde_json::to_value(&fresh).unwrap()
    );
    assert_eq!(retained(&mut c, &d), b"new");
    let pending = journal::load(&mut c, d.binding(), operation).unwrap();
    assert_eq!(pending.expected, old.version);
    assert_eq!(pending.body.as_bytes(), b"draft");
    assert_eq!(pending.state, journal::MutationState::Prepared);
    assert_eq!(
        prepare_write(&mut c, d.binding(), &old.key, &old.version, &pending.body),
        Err(ProviderError::Conflict)
    );
    let resumed = resume(&mut c, d.binding()).unwrap().unwrap();
    assert_eq!(resumed.snapshot.id(), snapshot.id());
    assert_eq!(resumed.list_cursor, Some(cursor("next")));
    let page = MetadataPage::new(
        vec![],
        None,
        3,
        Coverage::MetadataCompleteSelected { observed_at_ms: 3 },
    )
    .unwrap();
    assert_eq!(
        stage_page(&mut c, &snapshot, Some(&cursor("next")), &page),
        Err(ProviderError::Conflict)
    );
    stage_page(&mut c, &resumed.snapshot, Some(&cursor("next")), &page).unwrap();
}

#[test]
fn validator_refresh_and_withdrawal_preserve_exact_cached_bytes() {
    let mut c = Connection::open_in_memory().unwrap();
    let d = setup(&mut c);
    seed(&mut c, &d, false);
    let old = current(&c, &d);
    cache_body(&mut c, d.binding(), &body(&old, b"old")).unwrap();
    let mut fresh = old.clone();
    fresh.version.conditional_write_token = Some(OpaqueValue::new("new-etag").unwrap());
    let head = capture(&mut c, d.binding(), &old.key).unwrap();
    cache(&mut c, head, &fresh, &body(&fresh, b"old")).unwrap();
    assert_eq!(
        serde_json::to_value(current(&c, &d)).unwrap(),
        serde_json::to_value(&fresh).unwrap()
    );
    fresh.version.conditional_write_token = None;
    let head = capture(&mut c, d.binding(), &old.key).unwrap();
    cache(&mut c, head, &fresh, &body(&fresh, b"old")).unwrap();
    assert_eq!(current(&c, &d).version.conditional_write_token, None);
    assert_eq!(retained(&mut c, &d), b"old");
    // Even a provider without a checksum cannot change bytes at the same head.
    fresh.version.content_hash = None;
    let head = capture(&mut c, d.binding(), &old.key).unwrap();
    cache(&mut c, head, &fresh, &body(&fresh, b"old")).unwrap();
    let head = capture(&mut c, d.binding(), &old.key).unwrap();
    let mut wrong = fresh.clone();
    wrong.version.conditional_write_token = Some(OpaqueValue::new("another-etag").unwrap());
    assert_eq!(
        cache(&mut c, head, &wrong, &body(&wrong, b"bad")),
        Err(ProviderError::Conflict)
    );
    assert_eq!(
        serde_json::to_value(current(&c, &d)).unwrap(),
        serde_json::to_value(&fresh).unwrap()
    );
    assert_eq!(retained(&mut c, &d), b"old");
}

#[test]
fn captured_row_epoch_progress_and_authority_races_refuse_publication() {
    for race in ["row", "epoch", "progress", "disabled", "outside", "body"] {
        let mut c = Connection::open_in_memory().unwrap();
        let d = setup(&mut c);
        let snapshot = seed(&mut c, &d, false);
        let old = current(&c, &d);
        cache_body(&mut c, d.binding(), &body(&old, b"old")).unwrap();
        let head = capture(&mut c, d.binding(), &old.key).unwrap();
        let mut fresh = row("one", "v2", b"new");
        match race {
            "row" => {
                let tx = c.transaction().unwrap();
                exact_row(&tx, &snapshot, &row("one", "competing", b"old")).unwrap();
                tx.commit().unwrap();
            }
            "epoch" => {
                c.execute(
                    "UPDATE mdreader_provider_registry_state SET epoch=epoch+1",
                    [],
                )
                .unwrap();
            }
            "progress" => {
                stage_page(
                    &mut c,
                    &snapshot,
                    Some(&cursor("next")),
                    &MetadataPage::new(vec![], Some(cursor("later")), 2, Coverage::NeverIndexed)
                        .unwrap(),
                )
                .unwrap();
            }
            "disabled" => {
                disable(&mut c, d.binding(), &UnsupportedProjection).unwrap();
            }
            "outside" => {
                fresh.display_path = DisplayText::new("/outside/one.md").unwrap();
            }
            "body" => {
                fresh.size_bytes = Some(4);
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                cache(&mut c, head, &fresh, &body(&fresh, b"new")),
                Err(ProviderError::Conflict | ProviderError::Unauthorized)
            ),
            "{race}"
        );
        let stored: Vec<u8> = c
            .query_row(
                "SELECT body FROM mdreader_provider_registry_cache",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, b"old", "{race}");
        if race == "row" {
            assert_eq!(
                current(&c, &d).version.provider_revision.as_str(),
                "competing"
            );
        }
        if race == "progress" {
            assert_eq!(
                resume(&mut c, d.binding()).unwrap().unwrap().list_cursor,
                Some(cursor("later"))
            );
        }
    }
}

#[test]
fn published_current_head_preserves_hidden_rebuild_and_reopens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("public.sqlite");
    let mut c = Connection::open(&path).unwrap();
    let d = setup(&mut c);
    let published = seed(&mut c, &d, true);
    publish_snapshot(&mut c, &published, &UnsupportedProjection).unwrap();
    let pending = begin_snapshot(&mut c, d.binding(), None).unwrap();
    let other = row("two", "rebuild", b"two");
    stage_page(
        &mut c,
        &pending,
        None,
        &MetadataPage::new(
            vec![other.clone()],
            Some(cursor("rebuild-next")),
            4,
            Coverage::NeverIndexed,
        )
        .unwrap(),
    )
    .unwrap();
    let old = current(&c, &d);
    let head = capture(&mut c, d.binding(), &old.key).unwrap();
    let fresh = row("one", "v2", b"new");
    cache(&mut c, head, &fresh, &body(&fresh, b"new")).unwrap();
    assert_eq!(
        serde_json::to_value(metadata_in(&c, &pending.id().to_string(), &other.key).unwrap())
            .unwrap(),
        serde_json::to_value(Some(&other)).unwrap()
    );
    assert_eq!(
        serde_json::to_value(metadata_in(&c, &pending.id().to_string(), &fresh.key).unwrap())
            .unwrap(),
        serde_json::to_value(Some(&fresh)).unwrap()
    );
    assert_eq!(
        stage_page(
            &mut c,
            &pending,
            Some(&cursor("rebuild-next")),
            &MetadataPage::new(
                vec![],
                None,
                5,
                Coverage::MetadataCompleteSelected { observed_at_ms: 5 }
            )
            .unwrap()
        ),
        Err(ProviderError::Conflict)
    );
    drop(c);
    let mut c = Connection::open(&path).unwrap();
    assert_eq!(
        serde_json::to_value(current(&c, &d)).unwrap(),
        serde_json::to_value(&fresh).unwrap()
    );
    assert_eq!(retained(&mut c, &d), b"new");
    let resumed = resume(&mut c, d.binding()).unwrap().unwrap();
    assert_eq!(resumed.snapshot.id(), pending.id());
    assert_eq!(resumed.list_cursor, Some(cursor("rebuild-next")));
    assert_eq!(
        serde_json::to_value(
            metadata_in(&c, &resumed.snapshot.id().to_string(), &other.key).unwrap()
        )
        .unwrap(),
        serde_json::to_value(Some(&other)).unwrap()
    );
    stage_page(
        &mut c,
        &resumed.snapshot,
        Some(&cursor("rebuild-next")),
        &MetadataPage::new(
            vec![],
            None,
            5,
            Coverage::MetadataCompleteSelected { observed_at_ms: 5 },
        )
        .unwrap(),
    )
    .unwrap();
}
