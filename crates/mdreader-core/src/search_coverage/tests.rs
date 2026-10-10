use super::*;
use crate::{EMBEDDING_DIM, SourceRoot};

fn local() -> (tempfile::TempDir, ReaderStore, String) {
    let t = tempfile::tempdir().unwrap();
    let notes = t.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("one.md"), "# One\n\nquartz report").unwrap();
    std::fs::write(notes.join("two.markdown"), "# Two\n\nother document").unwrap();
    std::fs::write(notes.join("ignored.txt"), "not Markdown").unwrap();
    let mut store = ReaderStore::open(&t.path().join("profile")).unwrap();
    let id = store
        .add_local_source("Synthetic", SourceRoot::capture(notes).unwrap())
        .unwrap()
        .id;
    (t, store, id)
}

#[test]
fn queued_and_failed_refresh_preserve_last_index_observation_and_unknown_total() {
    let (t, mut store, id) = local();
    let sources = vec![id.clone()];
    let before = store.search_coverage(&sources, &[]).unwrap();
    assert_eq!(before.sources[0].indexed_documents, 0);
    assert_eq!(before.sources[0].snapshot.index, None);
    store.queue_index(&id).unwrap();
    assert_eq!(
        store.search_coverage(&sources, &[]).unwrap().sources[0].job_phase,
        SearchIndexPhase::Queued
    );
    store.index_source(&id).unwrap();
    store.record_index_job(&id, "done", None).unwrap();
    let indexed = store.search_coverage(&sources, &[]).unwrap();
    assert_eq!(indexed.sources[0].indexed_documents, 2);
    assert!(indexed.sources[0].snapshot.observed_at_ms.is_some());
    assert_eq!(indexed.sources[0].eligible_documents, None);
    store.queue_index(&id).unwrap();
    std::fs::write(t.path().join("notes/broken.md"), [0xff]).unwrap();
    assert!(store.index_source(&id).is_err());
    store
        .record_index_job(&id, "error", Some("private synthetic detail"))
        .unwrap();
    let failed = store.search_coverage(&sources, &[]).unwrap();
    assert_eq!(failed.sources[0].indexed_documents, 2);
    assert_eq!(failed.sources[0].snapshot, indexed.sources[0].snapshot);
    assert_eq!(failed.sources[0].job_phase, SearchIndexPhase::Error);
    assert!(
        !serde_json::to_string(&failed)
            .unwrap()
            .contains("private synthetic detail")
    );
    drop(store);
    let reopened = ReaderStore::open_readonly(&t.path().join("profile")).unwrap();
    assert_eq!(reopened.search_coverage(&sources, &[]).unwrap(), failed);
}

#[test]
fn partial_vectors_and_exact_current_receipts_are_counted_independently() {
    let (t, mut store, id) = local();
    store.index_source(&id).unwrap();
    let one = store.read_document(&id, "one.md", None).unwrap();
    let (_, passages) = store.embedding_inputs(&id, "one.md").unwrap();
    let vectors = vec![vec![0.01; EMBEDDING_DIM]; passages.len()];
    store
        .install_embeddings(&id, "one.md", &one.revision, &vectors)
        .unwrap();
    let full = store
        .search_coverage(std::slice::from_ref(&id), &[])
        .unwrap();
    assert_eq!(full.sources[0].indexed_documents, 2);
    assert!(full.sources[0].embedded_chunks > 0);
    assert!(full.sources[0].embedded_chunks < full.sources[0].indexed_chunks);
    std::fs::write(
        t.path().join("notes/one.md"),
        "# Current\n\nchanged revision",
    )
    .unwrap();
    let current = store.read_document(&id, "one.md", None).unwrap();
    let exact = vec![SearchDocumentReceipt {
        source_id: id.clone(),
        path: "one.md".into(),
        revision: current.revision,
    }];
    let stale = store
        .search_coverage(std::slice::from_ref(&id), &exact)
        .unwrap();
    assert_eq!(stale.sources[0].eligible_documents, Some(1));
    assert_eq!(stale.sources[0].indexed_documents, 0);
    assert_eq!(stale.sources[0].indexed_chunks, 0);
    assert_eq!(stale.sources[0].embedded_chunks, 0);
    store.index_source(&id).unwrap();
    let refreshed = store
        .search_coverage(std::slice::from_ref(&id), &exact)
        .unwrap();
    assert_eq!(refreshed.sources[0].indexed_documents, 1);
    assert_eq!(refreshed.sources[0].embedded_chunks, 0);
    assert_ne!(refreshed.scope_token, stale.scope_token);
    store.set_source_enabled(&id, false).unwrap();
    assert!(store.search_coverage(&[id], &exact).is_err());
}

#[test]
fn coverage_refuses_ungranted_receipts_and_unbounded_scope() {
    let (_t, store, id) = local();
    assert!(store.search_coverage(&[], &[]).is_err());
    assert!(
        store
            .search_coverage(&vec![id.clone(); MAX_SEARCH_SOURCES + 1], &[])
            .is_err()
    );
    let exact = SearchDocumentReceipt {
        source_id: "ungranted".into(),
        path: "document.md".into(),
        revision: "1".repeat(64),
    };
    assert!(store.search_coverage(&[id], &[exact]).is_err());
}

#[test]
fn device_receipt_counts_only_its_current_document_even_if_other_rows_are_indexed() {
    let t = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(&t.path().join("profile")).unwrap();
    let roots = t.path().join("documents");
    let document = crate::device::import(
        &mut store,
        &roots,
        "selected.md".into(),
        "# Selected\n\nquartz".into(),
    )
    .unwrap();
    #[cfg(unix)]
    std::fs::write(
        roots.join(&document.id).join("unselected.md"),
        "# Other\n\nother file",
    )
    .unwrap();
    store.index_source(&document.source_id).unwrap();
    let receipt = SearchDocumentReceipt {
        source_id: document.source_id.clone(),
        path: "document.md".into(),
        revision: document.revision.clone(),
    };
    let observed = store
        .search_coverage(std::slice::from_ref(&document.source_id), &[receipt])
        .unwrap();
    assert_eq!(observed.sources[0].indexed_documents, 1);
    assert_eq!(observed.sources[0].eligible_documents, Some(1));
    assert_eq!(observed.sources[0].embedded_chunks, 0);
    #[cfg(unix)]
    assert_eq!(
        store.index_status(&[document.source_id]).unwrap().documents,
        2
    );
}

#[cfg(any(unix, windows))]
#[test]
fn provider_current_admission_invalidates_old_index_and_revoke_refuses_coverage() {
    use mdreader_provider::*;
    let t = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(&t.path().join("profile")).unwrap();
    let binding = SourceBinding::new(
        LibraryId::new("synthetic-library").unwrap(),
        SourceId::new("selected-provider").unwrap(),
        AccountBinding::new("synthetic-account").unwrap(),
        uuid::Uuid::new_v4(),
        "1".repeat(64),
    )
    .unwrap();
    let projection =
        crate::ProviderProjection::open(&t.path().join("cache"), binding.library_id().clone())
            .unwrap();
    let mut metadata = Metadata {
        key: RecordKey::new("record").unwrap(),
        name: DisplayText::new("one.md").unwrap(),
        display_path: DisplayText::new("one.md").unwrap(),
        media_type: None,
        modified_at_ms: Some(1),
        size_bytes: None,
        version: DocumentVersion {
            provider_revision: OpaqueValue::new("v1").unwrap(),
            content_hash: None,
            conditional_write_token: None,
        },
    };
    let admit = |store: &mut ReaderStore, metadata: &Metadata, text: &str| {
        store.with_library_connection(|db| {
            let tx = db.transaction().unwrap();
            let body = DocumentBody {
                key: metadata.key.clone(),
                version: metadata.version.clone(),
                body: MarkdownBytes::new(text.as_bytes().to_vec()).unwrap(),
                observed_at_ms: 1,
            };
            let receipt = projection.admit(&tx, &binding, metadata, &body).unwrap();
            tx.commit().unwrap();
            receipt
        })
    };
    let first = admit(&mut store, &metadata, "# One\n\nquartz");
    let sources = vec![first.reader_source_id.clone()];
    let before = store.search_coverage(&sources, &[]).unwrap();
    assert_eq!(before.sources[0].admitted_bodies, Some(1));
    assert_eq!(before.sources[0].indexed_documents, 0);
    store.index_source(&sources[0]).unwrap();
    assert_eq!(
        store.search_coverage(&sources, &[]).unwrap().sources[0].indexed_documents,
        1
    );
    metadata.version.provider_revision = OpaqueValue::new("v2").unwrap();
    admit(&mut store, &metadata, "# New\n\nnew revision");
    let changed = store.search_coverage(&sources, &[]).unwrap();
    assert_eq!(changed.sources[0].admitted_bodies, Some(1));
    assert_eq!(changed.sources[0].indexed_documents, 0);
    store.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        projection.revoke(&tx, &binding).unwrap();
        tx.commit().unwrap();
    });
    assert!(store.search_coverage(&sources, &[]).is_err());
}
