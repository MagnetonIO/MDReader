use crate::{DeviceSearchStatus, EmbeddedRuntime, SearchMode, ServiceOptions};
use mdreader_core::{
    device::{
        self,
        assets::{AssetRuntime, AssetStore},
        publication::*,
    },
    ReaderStore,
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn ready(
    store: &mut ReaderStore,
    documents: &Path,
    assets: &AssetStore,
    validation: &AssetRuntime,
    origin: &IncomingOrigin,
) -> ReadyIncoming {
    let input = IncomingDocument::new(
        "public.md".into(),
        b"# Public receipt\n\nLexicalreadinessmarker\n".to_vec(),
        vec![],
    )
    .unwrap();
    reserve_new(store, documents, assets, origin, &input)
        .unwrap()
        .write(input)
        .unwrap()
        .validate(validation.validation_permit().unwrap())
        .unwrap()
}

fn ready_status(runtime: &EmbeddedRuntime) -> DeviceSearchStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = runtime.device_search_status().unwrap();
        assert!(!status.enabled);
        assert_eq!(status.model, "disabled");
        assert_eq!(status.embedded_documents, 0);
        if status.indexed_documents == 1 {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "lexical scheduler did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn actual_admission_index_completion_and_same_body_replacement_change_lexical_snapshot() {
    let owner = tempfile::tempdir().unwrap();
    let library = owner.path().join("data");
    let documents = owner.path().join("documents");
    let mut writer = ReaderStore::open(&library).unwrap();
    let assets = AssetStore::open(&mut writer, &owner.path().join("assets")).unwrap();
    let validation = AssetRuntime::default();
    let writer = Arc::new(Mutex::new(writer));
    let runtime = EmbeddedRuntime::new(
        writer.clone(),
        &library,
        ServiceOptions {
            disable_model: true,
        },
    )
    .unwrap();
    runtime.set_active(false);
    let empty = runtime.device_search_status().unwrap();
    assert_eq!((empty.documents, empty.indexed_documents), (0, 0));
    let mut origin = IncomingOrigin {
        scope_digest: [47; 32],
        item_id: uuid::Uuid::new_v4(),
        wire_revision: uuid::Uuid::new_v4(),
        session_epoch: uuid::Uuid::new_v4(),
        selection_epoch: uuid::Uuid::new_v4(),
    };
    let first = {
        let mut store = writer.lock().unwrap();
        let selected = ready(&mut store, &documents, &assets, &validation, &origin);
        let receipt = with_publication(
            &mut store,
            &selected,
            &origin,
            |db, guard| -> PublicationResult<_> {
                let tx = db.transaction()?;
                let receipt = publish_new(&tx, &guard, &selected)?;
                revalidate_publication(&tx, &guard, &selected)?;
                tx.commit()?;
                Ok(receipt)
            },
        )
        .unwrap()
        .unwrap();
        store.queue_index(&receipt.document.source_id).unwrap();
        receipt.document
    };
    let pending = runtime.device_search_status().unwrap();
    assert_eq!((pending.documents, pending.indexed_documents), (1, 0));
    assert_ne!(pending.lexical_snapshot, empty.lexical_snapshot);
    runtime.set_active(true);
    let first_ready = ready_status(&runtime);
    assert_ne!(first_ready.lexical_snapshot, pending.lexical_snapshot);
    let hits = runtime
        .search_device("Lexicalreadinessmarker", SearchMode::Exact)
        .unwrap();
    assert_eq!(hits.hits.len(), 1);
    assert_eq!(hits.hits[0].document.id, first.id);
    assert_eq!(
        runtime.device_search_status().unwrap().lexical_snapshot,
        first_ready.lexical_snapshot
    );

    runtime.set_active(false);
    origin.wire_revision = uuid::Uuid::new_v4();
    let replacement = {
        let mut store = writer.lock().unwrap();
        let current = prepare_current(&mut store, &first.id, &assets, &origin)
            .unwrap()
            .verify()
            .unwrap();
        let selected = ready(&mut store, &documents, &assets, &validation, &origin);
        let receipt = with_replacement(
            &mut store,
            &selected,
            &current,
            &origin,
            |db, guard| -> PublicationResult<_> {
                let tx = db.transaction()?;
                let receipt = publish_replacement(&tx, &guard, &selected, &current)?;
                revalidate_replacement(&tx, &guard, &selected, &current, &receipt)?;
                tx.commit()?;
                Ok(receipt)
            },
        )
        .unwrap()
        .unwrap();
        let document = receipt.publication().document.clone();
        store.queue_index(&document.source_id).unwrap();
        document
    };
    assert_eq!(replacement.revision, first.revision);
    assert_ne!(replacement.id, first.id);
    let pending_replacement = runtime.device_search_status().unwrap();
    assert_eq!(
        (
            pending_replacement.documents,
            pending_replacement.indexed_documents
        ),
        (1, 0)
    );
    assert_ne!(
        pending_replacement.lexical_snapshot,
        pending.lexical_snapshot
    );
    runtime.set_active(true);
    let replacement_ready = ready_status(&runtime);
    assert_ne!(
        replacement_ready.lexical_snapshot,
        first_ready.lexical_snapshot
    );
    assert_ne!(
        replacement_ready.lexical_snapshot,
        pending_replacement.lexical_snapshot
    );
    let hits = runtime
        .search_device("Lexicalreadinessmarker", SearchMode::Exact)
        .unwrap();
    assert_eq!(hits.hits.len(), 1);
    assert_eq!(hits.hits[0].document.id, replacement.id);
    let mut store = writer.lock().unwrap();
    assert!(device::read(&mut store, &first.id, None).is_err());
    let retained = prepare_archive(&mut store, &first.id, &assets, &validation)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(
        retained.markdown(),
        b"# Public receipt\n\nLexicalreadinessmarker\n"
    );
}
