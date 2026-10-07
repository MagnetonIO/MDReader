use mdreader_core::{ProviderProjection, ReaderStore, SourceKind, SourceRoot, EMBEDDING_DIM};
use mdreader_provider::{
    AccountBinding, DisplayText, DocumentBody, DocumentVersion, LibraryId, MarkdownBytes, Metadata,
    OpaqueValue, RecordKey, SourceBinding, SourceId,
};
use tempfile::TempDir;

fn binding(generation: uuid::Uuid) -> SourceBinding {
    SourceBinding::new(
        LibraryId::new("synthetic-library").unwrap(),
        SourceId::new("synthetic-drive").unwrap(),
        AccountBinding::new("synthetic-account").unwrap(),
        generation,
        "1".repeat(64),
    )
    .unwrap()
}
fn body(key: &str, revision: &str, text: &str) -> (Metadata, DocumentBody) {
    let key = RecordKey::new(key).unwrap();
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new(revision).unwrap(),
        content_hash: None,
        conditional_write_token: None,
    };
    (
        Metadata {
            key: key.clone(),
            name: DisplayText::new("Synthetic note").unwrap(),
            display_path: DisplayText::new("Notes / Synthetic").unwrap(),
            media_type: None,
            modified_at_ms: Some(1234),
            size_bytes: Some(text.len() as u64),
            version: version.clone(),
        },
        DocumentBody {
            key,
            version,
            body: MarkdownBytes::new(text.as_bytes().to_vec()).unwrap(),
            observed_at_ms: 1235,
        },
    )
}
fn setup() -> (TempDir, ReaderStore, ProviderProjection, SourceBinding) {
    let temp = TempDir::new().unwrap();
    let store = ReaderStore::open(&temp.path().join("data")).unwrap();
    let projection = ProviderProjection::open(
        &temp.path().join("cache"),
        LibraryId::new("synthetic-library").unwrap(),
    )
    .unwrap();
    (temp, store, projection, binding(uuid::Uuid::new_v4()))
}

#[test]
fn exact_projection_receipt_is_transactional_and_recoverable_without_scanning_cache() {
    let (temp, mut store, projection, binding) = setup();
    let (metadata, document) = body(
        "../../opaque/provider/key",
        "remote-1",
        "# Synthetic\n\n## Evidence\nphotovoltaic solar panel.\n",
    );
    let receipt = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let r = projection
            .admit(&tx, &binding, &metadata, &document)
            .unwrap();
        tx.rollback().unwrap();
        r
    });
    assert!(store
        .read_document(&receipt.reader_source_id, &receipt.path, None)
        .is_err());
    assert!(store.list_sources().unwrap().is_empty());
    let receipt = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let r = projection
            .admit(&tx, &binding, &metadata, &document)
            .unwrap();
        tx.commit().unwrap();
        r
    });
    assert_eq!(
        store.source_kind(&receipt.reader_source_id).unwrap(),
        SourceKind::ProviderDerived
    );
    assert!(!receipt.path.contains(".."));
    assert!(receipt.path.starts_with("records/"));
    assert_eq!(
        store
            .read_document(
                &receipt.reader_source_id,
                &receipt.path,
                Some(&receipt.revision)
            )
            .unwrap()
            .markdown,
        std::str::from_utf8(document.body.as_bytes()).unwrap()
    );
    assert!(store
        .pending_index_jobs()
        .unwrap()
        .contains(&receipt.reader_source_id));
    // A private orphan file is never a provider record, even after restart/rebuild.
    std::fs::write(
        temp.path().join("cache/orphan.md"),
        "secret orphan photovoltaic",
    )
    .unwrap();
    drop(store);
    let mut store = ReaderStore::open(&temp.path().join("data")).unwrap();
    assert_eq!(store.index_source(&receipt.reader_source_id).unwrap(), 1);
    let hits = store
        .search(
            "photovoltaic",
            std::slice::from_ref(&receipt.reader_source_id),
            10,
        )
        .unwrap();
    assert!(!hits.is_empty());
    assert!(hits
        .iter()
        .all(|h| h.path == receipt.path && h.revision == receipt.revision && h.line_start > 0));
    let local = SourceRoot::capture(temp.path().join("cache").canonicalize().unwrap()).unwrap();
    assert!(store
        .find_local_source(&local.path, &local.identity)
        .unwrap()
        .is_none());
    assert!(store
        .add_local_source("Cache is not a local grant", local)
        .is_err());
}

#[test]
fn changed_deleted_and_revoked_records_hide_old_index_and_vectors_before_reindex() {
    let (temp, mut store, projection, binding) = setup();
    let (metadata, document) = body(
        "opaque-key",
        "remote-1",
        "## Evidence\nphotovoltaic solar panel.\n",
    );
    let first = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let r = projection
            .admit(&tx, &binding, &metadata, &document)
            .unwrap();
        tx.commit().unwrap();
        r
    });
    store.index_source(&first.reader_source_id).unwrap();
    let (_, inputs) = store
        .embedding_inputs(&first.reader_source_id, &first.path)
        .unwrap();
    let mut vector = vec![0.; EMBEDDING_DIM];
    vector[0] = 1.;
    store
        .install_embeddings(
            &first.reader_source_id,
            &first.path,
            &first.revision,
            &vec![vector.clone(); inputs.len()],
        )
        .unwrap();
    let reader = ReaderStore::open_readonly(&temp.path().join("data")).unwrap();
    assert!(!reader
        .semantic_search(&vector, std::slice::from_ref(&first.reader_source_id), 10)
        .unwrap()
        .is_empty());
    // Same bytes, new exact remote revision: old evidence must disappear immediately.
    let (metadata, document) = body(
        "opaque-key",
        "remote-2",
        "## Evidence\nphotovoltaic solar panel.\n",
    );
    let second = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let r = projection
            .admit(&tx, &binding, &metadata, &document)
            .unwrap();
        tx.commit().unwrap();
        r
    });
    assert_ne!(first.revision, second.revision);
    assert_eq!(first.path, second.path);
    assert!(reader
        .search(
            "photovoltaic",
            std::slice::from_ref(&first.reader_source_id),
            10
        )
        .unwrap()
        .is_empty());
    assert!(reader
        .semantic_search(&vector, std::slice::from_ref(&first.reader_source_id), 10)
        .unwrap()
        .is_empty());
    assert!(reader
        .resolve_reference(&first.reader_source_id, "entry.md", &first.path)
        .unwrap()
        .is_empty());
    assert!(reader
        .related(&first.reader_source_id, &first.path, 10)
        .unwrap()
        .is_empty());
    assert!(reader
        .related_ranked(&first.reader_source_id, &first.path, 10)
        .unwrap()
        .anchor
        .is_none());
    assert!(reader
        .pending_embedding_documents(&first.reader_source_id)
        .unwrap()
        .is_empty());
    assert_eq!(
        reader
            .index_status(std::slice::from_ref(&first.reader_source_id))
            .unwrap()
            .embedded,
        0
    );
    assert!(reader
        .read_document(&first.reader_source_id, &first.path, Some(&first.revision))
        .is_err());
    assert!(store
        .install_embeddings(
            &first.reader_source_id,
            &first.path,
            &first.revision,
            &vec![vector.clone(); inputs.len()]
        )
        .is_err());
    store.index_source(&second.reader_source_id).unwrap();
    assert!(!reader
        .search(
            "photovoltaic",
            std::slice::from_ref(&second.reader_source_id),
            10
        )
        .unwrap()
        .is_empty());
    store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        projection.remove(&tx, &binding, &metadata.key).unwrap();
        tx.commit().unwrap();
    });
    assert!(reader
        .search(
            "photovoltaic",
            std::slice::from_ref(&second.reader_source_id),
            10
        )
        .unwrap()
        .is_empty());
    assert!(reader
        .read_document(&second.reader_source_id, &second.path, None)
        .is_err());
    store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        projection.revoke(&tx, &binding).unwrap();
        projection.revoke(&tx, &binding).unwrap();
        tx.commit().unwrap();
    });
    assert!(reader
        .index_status(std::slice::from_ref(&second.reader_source_id))
        .is_err());
    assert!(store
        .set_source_enabled(&second.reader_source_id, true)
        .is_err());
    assert!(store.index_source(&second.reader_source_id).is_err());
    assert!(!store
        .pending_index_jobs()
        .unwrap()
        .contains(&second.reader_source_id));
    assert!(!store
        .record_index_job_if_current(&second.reader_source_id, 1, "done", None)
        .unwrap());
    assert!(store
        .with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            projection.admit(&tx, &binding, &metadata, &document)
        })
        .is_err());
}

#[test]
fn generation_rebind_and_cross_library_identity_fail_closed() {
    let (_temp, mut store, projection, old) = setup();
    let (metadata, document) = body("key", "v1", "## Evidence\nsynthetic photovoltaic\n");
    let first = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let r = projection.admit(&tx, &old, &metadata, &document).unwrap();
        tx.commit().unwrap();
        r
    });
    let new = binding(uuid::Uuid::new_v4());
    assert!(store
        .with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            projection.admit(&tx, &new, &metadata, &document)
        })
        .is_err());
    store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        projection.revoke(&tx, &old).unwrap();
        tx.commit().unwrap();
    });
    let next = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let r = projection.admit(&tx, &new, &metadata, &document).unwrap();
        tx.commit().unwrap();
        r
    });
    assert_ne!(first.reader_source_id, next.reader_source_id);
    // Late old revoke replay cannot revoke the replacement; late old admit cannot revive itself.
    store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        projection.revoke(&tx, &old).unwrap();
        tx.commit().unwrap();
    });
    assert!(store
        .read_document(&next.reader_source_id, &next.path, None)
        .is_ok());
    assert!(store
        .with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            projection.admit(&tx, &old, &metadata, &document)
        })
        .is_err());
    let other = SourceBinding::new(
        LibraryId::new("other-library").unwrap(),
        new.source_id().clone(),
        new.account().clone(),
        uuid::Uuid::new_v4(),
        "1".repeat(64),
    )
    .unwrap();
    assert!(store
        .with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            projection.admit(&tx, &other, &metadata, &document)
        })
        .is_err());
    let mut mismatched = document.clone();
    mismatched.version.provider_revision = OpaqueValue::new("wrong-version").unwrap();
    assert!(store
        .with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            projection.admit(&tx, &new, &metadata, &mismatched)
        })
        .is_err());
}

#[test]
fn integrity_and_atomic_revoke_gate_do_not_publish_partial_authority() {
    use mdreader_provider::{ContentHash, HashAlgorithm};
    let (temp, mut store, projection, binding) = setup();
    let (mut metadata, mut document) = body("key", "v1", "## Evidence\nphotovoltaic\n");
    let wrong = ContentHash::new(HashAlgorithm::Sha256, vec![0; 32]).unwrap();
    metadata.version.content_hash = Some(wrong.clone());
    document.version.content_hash = Some(wrong);
    assert!(store
        .with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            projection.admit(&tx, &binding, &metadata, &document)
        })
        .is_err());
    assert!(store.list_sources().unwrap().is_empty());
    let correct = ContentHash::new(
        HashAlgorithm::Sha256,
        mdreader_provider::body_hash(document.body.as_bytes()).to_vec(),
    )
    .unwrap();
    metadata.version.content_hash = Some(correct.clone());
    document.version.content_hash = Some(correct);
    let receipt = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let r = projection
            .admit(&tx, &binding, &metadata, &document)
            .unwrap();
        tx.commit().unwrap();
        r
    });
    store.with_library_connection(|c| {c.execute_batch("CREATE TABLE projection_test_registry(enabled INTEGER);INSERT INTO projection_test_registry VALUES(1);").unwrap();let tx=c.transaction().unwrap();projection.revoke(&tx,&binding).unwrap();tx.execute("UPDATE projection_test_registry SET enabled=0",[]).unwrap();tx.rollback().unwrap();assert_eq!(c.query_row("SELECT enabled FROM projection_test_registry",[],|r|r.get::<_,i64>(0)).unwrap(),1);});
    assert!(store
        .read_document(
            &receipt.reader_source_id,
            &receipt.path,
            Some(&receipt.revision)
        )
        .is_ok());
    // Corrupting a private cache object cannot silently publish a new revision.
    let cache_path: String = store.with_library_connection(|c| {
        c.query_row(
            "SELECT cache_path FROM core_provider_records WHERE reader_source_id=?1",
            [&receipt.reader_source_id],
            |r| r.get(0),
        )
        .unwrap()
    });
    std::fs::write(
        temp.path().join("cache").join(cache_path),
        "corrupted bytes",
    )
    .unwrap();
    assert!(store
        .read_document(&receipt.reader_source_id, &receipt.path, None)
        .is_err());
    assert!(store.index_source(&receipt.reader_source_id).is_err());
    // Namespaced migration guard leaves the shared library schema version alone.
    store.with_library_connection(|c| {
        assert_eq!(
            c.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            4
        );
        c.execute("UPDATE core_provider_schema SET version=99", [])
            .unwrap();
    });
    drop(store);
    assert!(ReaderStore::open_readonly(&temp.path().join("data")).is_err());
    assert!(ReaderStore::open(&temp.path().join("data")).is_err());
}
