use mdreader_core::{ReaderError, ReaderStore, SourceRoot};
use std::fs;
fn setup(count: usize) -> (tempfile::TempDir, ReaderStore, String, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    fs::create_dir(&notes).unwrap();
    for n in 0..count {
        fs::write(
            notes.join(format!("{n:03}.md")),
            format!("# Actual title {n}\n\nSynthetic corpus."),
        )
        .unwrap();
    }
    let mut store = ReaderStore::open(&temp.path().join("profile")).unwrap();
    let source = store
        .add_local_source(
            "Selected",
            SourceRoot::capture(notes.canonicalize().unwrap()).unwrap(),
        )
        .unwrap()
        .id;
    (temp, store, source, notes)
}
#[test]
fn keyset_pages_preserve_indexed_evidence_and_reject_reindex_and_regrant_aba() {
    let (temp, mut store, source, notes) = setup(103);
    store.index_source(&source).unwrap();
    let first = store.catalog_page(&source, None, None, 100).unwrap();
    assert_eq!(first.documents.len(), 100);
    assert_eq!(first.documents[0].title, "Actual title 0");
    assert_eq!(
        first.documents[0].revision,
        store
            .read_document(&source, "000.md", None)
            .unwrap()
            .revision
    );
    let read = ReaderStore::open_readonly(&temp.path().join("profile")).unwrap();
    let next = read
        .catalog_page(
            &source,
            first.next_after.as_deref(),
            first.snapshot.as_ref(),
            100,
        )
        .unwrap();
    assert_eq!(next.documents.len(), 3);
    assert_eq!(next.documents[0].path, "100.md");
    assert!(next.next_after.is_none());
    fs::write(notes.join("101.md"), "# New committed revision").unwrap();
    store.index_source(&source).unwrap();
    assert!(matches!(
        read.catalog_page(
            &source,
            first.next_after.as_deref(),
            first.snapshot.as_ref(),
            100
        ),
        Err(ReaderError::Conflict)
    ));
    let fresh = store.catalog_page(&source, None, None, 2).unwrap();
    store.set_source_enabled(&source, false).unwrap();
    store.set_source_enabled(&source, true).unwrap();
    assert!(matches!(
        store.catalog_page(
            &source,
            fresh.next_after.as_deref(),
            fresh.snapshot.as_ref(),
            2
        ),
        Err(ReaderError::Conflict)
    ));
    // Index publication and its nonce roll back together on an invalid source file.
    let stable = store.catalog_page(&source, None, None, 2).unwrap();
    fs::write(notes.join("bad.md"), [0xff]).unwrap();
    assert!(store.index_source(&source).is_err());
    assert!(store
        .catalog_page(
            &source,
            stable.next_after.as_deref(),
            stable.snapshot.as_ref(),
            2
        )
        .is_ok());
    store.forget_source_index(&source).unwrap();
    store.set_source_enabled(&source, true).unwrap();
    assert!(store
        .catalog_page(&source, None, None, 2)
        .unwrap()
        .snapshot
        .is_none());
}
#[test]
fn empty_completion_differs_from_unindexed_and_future_or_weakened_namespace_is_not_reset() {
    let (temp, mut store, source, _) = setup(0);
    assert!(store
        .catalog_page(&source, None, None, 20)
        .unwrap()
        .snapshot
        .is_none());
    store.index_source(&source).unwrap();
    let empty = store.catalog_page(&source, None, None, 20).unwrap();
    assert!(empty.snapshot.is_some());
    assert!(empty.indexed_at_ms.is_some());
    assert!(empty.documents.is_empty());
    store
        .with_library_connection(|db| {
            db.execute("UPDATE reader_catalog_schema_v1 SET version=999", [])
        })
        .unwrap();
    assert!(store.catalog_page(&source, None, None, 20).is_err());
    drop(store);
    assert!(ReaderStore::open(&temp.path().join("profile")).is_err());
    let db = rusqlite::Connection::open(temp.path().join("profile/library.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT version FROM reader_catalog_schema_v1", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        999
    );
    db.execute("UPDATE reader_catalog_schema_v1 SET version=1", [])
        .unwrap();
    db.execute_batch("DROP TRIGGER reader_catalog_update_v1; CREATE TRIGGER reader_catalog_update_v1 AFTER UPDATE OF enabled ON sources BEGIN SELECT 1; END;").unwrap();
    drop(db);
    assert!(ReaderStore::open(&temp.path().join("profile")).is_err());
}
#[test]
fn historical_index_adoption_preserves_documents_bookmarks_and_disabled_grants() {
    let (temp, mut store, source, _) = setup(2);
    store.index_source(&source).unwrap();
    store.set_bookmark(&source, "000.md", 2).unwrap();
    let before = store.read_document(&source, "000.md", None).unwrap();
    store.set_source_enabled(&source, false).unwrap();
    drop(store);
    let profile = temp.path().join("profile");
    let db = rusqlite::Connection::open(profile.join("library.sqlite")).unwrap();
    db.execute_batch("DROP TRIGGER reader_catalog_insert_v1; DROP TRIGGER reader_catalog_update_v1; DROP TRIGGER reader_catalog_delete_v1; DROP TABLE reader_catalog_authorities_v1; DROP TABLE reader_catalog_schema_v1;").unwrap();
    drop(db);
    let db = rusqlite::Connection::open(profile.join("index.sqlite")).unwrap();
    db.execute_batch(
        "DROP TABLE reader_catalog_snapshots_v1; DROP TABLE reader_catalog_index_schema_v1;",
    )
    .unwrap();
    drop(db);
    let mut reopened = ReaderStore::open(&profile).unwrap();
    assert!(!reopened.list_sources().unwrap()[0].enabled);
    assert!(reopened.catalog_page(&source, None, None, 20).is_err());
    reopened.set_source_enabled(&source, true).unwrap();
    let adopted = reopened.catalog_page(&source, None, None, 20).unwrap();
    assert_eq!(adopted.documents.len(), 2);
    assert!(adopted.indexed_at_ms.is_none());
    assert_eq!(reopened.bookmark(&source, "000.md").unwrap(), Some(2));
    assert_eq!(
        reopened
            .read_document(&source, "000.md", Some(&before.revision))
            .unwrap()
            .markdown,
        before.markdown
    );
}

#[test]
fn repeated_identical_admission_preserves_catalog_cursor_but_rename_invalidates() {
    let (_temp, mut store, source, notes) = setup(3);
    store.index_source(&source).unwrap();
    let page = store.catalog_page(&source, None, None, 1).unwrap();
    for _ in 0..3 {
        assert_eq!(
            store
                .add_local_source(
                    "Selected",
                    SourceRoot::capture(notes.canonicalize().unwrap()).unwrap()
                )
                .unwrap()
                .id,
            source
        );
        assert!(store
            .catalog_page(
                &source,
                page.next_after.as_deref(),
                page.snapshot.as_ref(),
                1
            )
            .is_ok());
    }
    store
        .add_local_source(
            "Renamed",
            SourceRoot::capture(notes.canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        store.catalog_page(
            &source,
            page.next_after.as_deref(),
            page.snapshot.as_ref(),
            1
        ),
        Err(ReaderError::Conflict)
    ));
}
