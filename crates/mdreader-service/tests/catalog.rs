use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_service::catalog::*;
use std::{
    fs,
    sync::{Arc, Mutex},
};
fn fixture(count: usize) -> (tempfile::TempDir, Arc<Mutex<ReaderStore>>, String) {
    let t = tempfile::tempdir().unwrap();
    let notes = t.path().join("notes");
    fs::create_dir(&notes).unwrap();
    for n in 0..count {
        fs::write(
            notes.join(format!("{n:03}.md")),
            format!("# Document {n}\nOriginal {n}\n"),
        )
        .unwrap();
    }
    let mut owner = ReaderStore::open(&t.path().join("profile")).unwrap();
    let source = owner
        .add_local_source("Selected", SourceRoot::capture(notes).unwrap())
        .unwrap()
        .id;
    owner.index_source(&source).unwrap();
    (t, Arc::new(Mutex::new(owner)), source)
}
fn page(c: &Coordinator, id: &str, cursor: Option<String>) -> CatalogReply {
    c.execute(CatalogRequest::Page {
        catalog_id: format!("local:{id}"),
        cursor,
        limit: 2,
    })
    .unwrap()
}
#[test]
fn catalog_pagination_is_exact_bounded_and_never_skips_on_retry_or_accepts_reindex_regrant() {
    let (t, owner, id) = fixture(5);
    let c = Coordinator::new(
        owner.clone(),
        Arc::new(Mutex::new(
            ReaderStore::open_readonly(&t.path().join("profile")).unwrap(),
        )),
    );
    let CatalogReply::Page {
        documents,
        next_cursor,
        ..
    } = page(&c, &id, None)
    else {
        panic!()
    };
    assert_eq!(documents.len(), 2);
    let old = next_cursor.unwrap();
    let CatalogReply::Page {
        documents,
        next_cursor,
        ..
    } = page(&c, &id, Some(old.clone()))
    else {
        panic!()
    };
    assert_eq!(documents[0].display_path, "002.md");
    assert!(matches!(documents[0].open, CatalogOpen::Exact { .. }));
    assert!(matches!(
        c.execute(CatalogRequest::Page {
            catalog_id: format!("local:{id}"),
            cursor: Some(old),
            limit: 2
        }),
        Err(CatalogError::Stale)
    ));
    let cursor = next_cursor.unwrap();
    fs::write(t.path().join("notes/004.md"), "# Changed").unwrap();
    owner.lock().unwrap().index_source(&id).unwrap();
    assert!(matches!(
        c.execute(CatalogRequest::Page {
            catalog_id: format!("local:{id}"),
            cursor: Some(cursor),
            limit: 2
        }),
        Err(CatalogError::Stale)
    ));
    let CatalogReply::Page { next_cursor, .. } = page(&c, &id, None) else {
        panic!()
    };
    owner
        .lock()
        .unwrap()
        .set_source_enabled(&id, false)
        .unwrap();
    owner.lock().unwrap().set_source_enabled(&id, true).unwrap();
    assert!(matches!(
        c.execute(CatalogRequest::Page {
            catalog_id: format!("local:{id}"),
            cursor: next_cursor,
            limit: 2
        }),
        Err(CatalogError::Stale)
    ));
}
#[test]
fn one_unavailable_source_and_bad_provider_namespace_do_not_hide_valid_local_catalog_or_admit_roots(
) {
    let (t, owner, id) = fixture(1);
    let c = Coordinator::new(
        owner.clone(),
        Arc::new(Mutex::new(
            ReaderStore::open_readonly(&t.path().join("profile")).unwrap(),
        )),
    );
    let missing = "f".repeat(64);
    let before = owner.lock().unwrap().list_sources().unwrap();
    owner
        .lock()
        .unwrap()
        .with_library_connection(|db| {
            db.execute_batch("CREATE TABLE mdreader_provider_registry_meta(version INTEGER)")
        })
        .unwrap();
    let CatalogReply::List {
        sources,
        provider_failure,
    } = c
        .execute(CatalogRequest::List {
            local_sources: vec![id.clone(), missing],
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(sources.len(), 2);
    assert!(sources[0].failure.is_none());
    assert!(sources[1].failure.is_some());
    assert!(provider_failure.is_some());
    assert_eq!(
        owner.lock().unwrap().list_sources().unwrap().len(),
        before.len()
    );
    let CatalogReply::Page {
        source, documents, ..
    } = page(&c, &id, None)
    else {
        panic!()
    };
    assert_eq!(source.coverage, "indexed_observation");
    assert_eq!(documents.len(), 1);
    assert!(documents[0].size_bytes.is_none());
    let json = serde_json::to_string(&documents).unwrap();
    assert!(!json.contains("Original"));
    assert!(!json.contains("profile"));
    owner
        .lock()
        .unwrap()
        .set_source_enabled(&id, false)
        .unwrap();
    assert!(matches!(
        c.execute(CatalogRequest::Page {
            catalog_id: format!("local:{id}"),
            cursor: None,
            limit: 2
        }),
        Err(CatalogError::Invalid | CatalogError::Unauthorized)
    ));
}
#[test]
fn local_catalog_reads_do_not_wait_for_a_busy_writer_and_cloud_metadata_reports_busy_explicitly() {
    let (t, owner, id) = fixture(1);
    let read = Arc::new(Mutex::new(
        ReaderStore::open_readonly(&t.path().join("profile")).unwrap(),
    ));
    let c = Arc::new(Coordinator::new(owner.clone(), read));
    let held = owner.lock().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = c.clone();
    let source = id.clone();
    let thread = std::thread::spawn(move || {
        let page = worker.execute(CatalogRequest::Page {
            catalog_id: format!("local:{source}"),
            cursor: None,
            limit: 2,
        });
        let list = worker.execute(CatalogRequest::List {
            local_sources: vec![source],
        });
        send.send((page, list)).unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(1));
    drop(held);
    thread.join().unwrap();
    let (page, list) = result.expect("local metadata must complete without the writer mutex");
    assert!(matches!(page,Ok(CatalogReply::Page{documents,..})if documents.len()==1));
    assert!(
        matches!(list,Ok(CatalogReply::List{sources,provider_failure:Some(CatalogError::Busy)})if sources.len()==1)
    );
}
