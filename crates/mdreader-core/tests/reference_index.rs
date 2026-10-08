use mdreader_core::{ReaderStore, SourceRoot};
use std::fs;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn unchanged_content_touch_refreshes_recent_and_after_evidence_without_a_new_revision() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    for (name, seconds) in [("first.md", 1_735_689_600), ("second.md", 1_767_225_600)] {
        let path = notes.path().join(name);
        fs::write(&path, "# Note\nCommon reading text.\n").unwrap();
        fs::File::open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Selected",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let before = store
        .search_recent("Common", std::slice::from_ref(&source.id), 20)
        .unwrap();
    assert_eq!(before[0].path, "second.md");
    assert!(store
        .search(
            "Common after:2026-06-01",
            std::slice::from_ref(&source.id),
            20
        )
        .unwrap()
        .is_empty());
    fs::File::open(notes.path().join("first.md"))
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_798_761_600))
        .unwrap();
    store.index_source(&source.id).unwrap();
    let after = store
        .search_recent("Common", std::slice::from_ref(&source.id), 20)
        .unwrap();
    assert_eq!(after[0].path, "first.md");
    assert_eq!(after[0].revision, before[1].revision);
    assert_eq!(after[0].modified_ms, 1_798_761_600_000);
    let filtered = store
        .search(
            "Common after:2026-06-01",
            std::slice::from_ref(&source.id),
            20,
        )
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].path, "first.md");
}

#[test]
fn a_link_budget_failure_rolls_back_documents_and_the_previous_graph() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    fs::write(
        notes.path().join("from.md"),
        "# From\n[Target](target.md)\n",
    )
    .unwrap();
    fs::write(notes.path().join("target.md"), "# Target\nOriginal.\n").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Selected",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let index = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    // Synthetic derived rows exercise the exact bound without a giant fixture.
    index.execute("WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO links SELECT d.id,'https:external','link',n.x FROM documents d,n WHERE d.source_id=?1 AND d.path='from.md'",[&source.id]).unwrap();
    fs::write(notes.path().join("new.md"), "# New\nUncommitted change.\n").unwrap();
    let error = store.index_source(&source.id).unwrap_err();
    assert!(error
        .to_string()
        .contains("link materialization exceeds budget"));
    assert_eq!(
        store.related(&source.id, "from.md", 20).unwrap()[0].path,
        "target.md"
    );
    assert!(store
        .search("Uncommitted", std::slice::from_ref(&source.id), 20)
        .unwrap()
        .is_empty());
    index
        .execute("DELETE FROM links WHERE target='https:external'", [])
        .unwrap();
    store.index_source(&source.id).unwrap();
    assert_eq!(
        store
            .search("Uncommitted", std::slice::from_ref(&source.id), 20)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn readonly_handles_never_create_migrate_or_write_and_can_read_during_an_owner_write() {
    let data = tempdir().unwrap();
    let missing = data.path().join("missing");
    assert!(ReaderStore::open_readonly(&missing).is_err());
    assert!(!missing.exists());
    assert!(ReaderStore::open_readonly(data.path()).is_err());
    assert_eq!(fs::read_dir(data.path()).unwrap().count(), 0);
    let notes = tempdir().unwrap();
    fs::write(notes.path().join("note.md"), "# Note\nVisible text.\n").unwrap();
    let mut owner = ReaderStore::open(data.path()).unwrap();
    let source = owner
        .add_local_source(
            "Selected",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    owner.index_source(&source.id).unwrap();
    let mut reader = ReaderStore::open_readonly(data.path()).unwrap();
    assert_eq!(
        reader
            .search("Visible", std::slice::from_ref(&source.id), 20)
            .unwrap()
            .len(),
        1
    );
    assert!(reader.set_source_enabled(&source.id, false).is_err());
    assert!(reader.index_source(&source.id).is_err());
    assert!(reader
        .with_library_connection(|connection| connection.execute("DELETE FROM sources", []))
        .is_err());
    // The borrowed connection participates in the owner's transaction. WAL
    // readers see the previous committed source state without creating a writer.
    owner.with_library_connection(|connection| {
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("UPDATE sources SET name='Uncommitted'", [])
            .unwrap();
        assert_eq!(reader.list_sources().unwrap()[0].name, "Selected");
        assert_eq!(
            reader
                .search("Visible", std::slice::from_ref(&source.id), 20)
                .unwrap()
                .len(),
            1
        );
        transaction.commit().unwrap();
    });
    assert_eq!(reader.list_sources().unwrap()[0].name, "Uncommitted");
    let mut index_writer = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    let transaction = index_writer.transaction().unwrap();
    transaction
        .execute("UPDATE documents SET title='Pending title'", [])
        .unwrap();
    assert_eq!(
        reader
            .search("Visible", std::slice::from_ref(&source.id), 20)
            .unwrap()[0]
            .title,
        "Note"
    );
    transaction.rollback().unwrap();
    drop(index_writer);
    drop(reader);
    drop(owner);
    for filename in ["library.sqlite", "index.sqlite"] {
        let connection = rusqlite::Connection::open(data.path().join(filename)).unwrap();
        let expected = if filename == "index.sqlite" { 6 } else { 4 };
        for version in [0, expected - 1, expected + 1] {
            connection
                .pragma_update(None, "user_version", version)
                .unwrap();
            assert!(ReaderStore::open_readonly(data.path()).is_err());
            let current: i64 = connection
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap();
            assert_eq!(current, version, "readonly must not migrate {filename}");
        }
        connection.pragma_update(None, "user_version", expected).unwrap();
    }
    assert!(ReaderStore::open_readonly(data.path()).is_ok());
}

#[test]
fn linked_and_related_work_is_bounded_by_indexed_neighbors_at_library_scale() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    let count = 3000;
    for index in 0..count {
        fs::write(
            notes.path().join(format!("note{index:05}.md")),
            format!(
                "# Note {index}\nBody.\n[Next](note{:05}.md)\n",
                (index + 1) % count
            ),
        )
        .unwrap();
    }
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Synthetic",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let start = Instant::now();
    let related = store.related(&source.id, "note00001.md", 20).unwrap();
    assert_eq!(
        related.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        ["note00000.md", "note00002.md"]
    );
    let incoming = store
        .search("linked-to:note00001.md", &[source.id], 20)
        .unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].path, "note00000.md");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "two one-neighbor queries must not rescan all documents once per link: {:?}",
        start.elapsed()
    );
}

#[test]
fn exact_relative_precedence_alias_ambiguity_and_heading_evidence_survive_reindex() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    fs::create_dir_all(notes.path().join("docs")).unwrap();
    fs::create_dir_all(notes.path().join("other")).unwrap();
    for (path,body) in [
        ("docs/from.md","# From\n[Exact](target.md#details)\n[Missing](future.md)\n![Image](../other/image.md)\n[Web](https://example.invalid/target.md)\n[Escape](../../target.md)\n[Self](#from)\n"),
        ("docs/target.md","# Shared Title\n\n## Details\nExact neighbor.\n"),
        ("other/target.md","# Shared Title\n\n## Details\nDuplicate basename.\n\n## Remote Only\nThe exact local target does not have this heading.\n"),
        ("other/image.md","# Image\nAn image link must not create a related document.\n"),
    ] {fs::write(notes.path().join(path),body).unwrap();}
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Selected",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let exact = store
        .resolve_reference(&source.id, "docs/from.md", "target.md#details")
        .unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].path, "docs/target.md");
    assert_eq!(exact[0].line, 3);
    assert!(
        store
            .resolve_reference(&source.id, "docs/from.md", "target.md#remote-only")
            .unwrap()
            .is_empty(),
        "missing heading on an exact path must not activate alias fallback"
    );
    let ambiguous = store
        .resolve_reference(&source.id, "from.md", "target#Details")
        .unwrap();
    assert_eq!(ambiguous.len(), 2);
    let title = store
        .resolve_reference(&source.id, "from.md", "shared title")
        .unwrap();
    assert_eq!(title.len(), 2);
    assert_eq!(
        store.related(&source.id, "docs/from.md", 20).unwrap().len(),
        1
    );
    assert!(store
        .search(
            "linked-to:other/image.md",
            std::slice::from_ref(&source.id),
            20
        )
        .unwrap()
        .is_empty());
    fs::write(
        notes.path().join("docs/future.md"),
        "# Future\nCreated target.\n",
    )
    .unwrap();
    store.index_source(&source.id).unwrap();
    assert_eq!(
        store.related(&source.id, "docs/from.md", 20).unwrap().len(),
        2,
        "previously unresolved edges must refresh when a target appears"
    );
    fs::write(
        notes.path().join("docs/target.md"),
        "# Renamed\n\n\n## Details\nNew revision and line.\n",
    )
    .unwrap();
    store.index_source(&source.id).unwrap();
    let new = store
        .resolve_reference(&source.id, "docs/from.md", "target.md#details")
        .unwrap();
    assert_ne!(new[0].revision, exact[0].revision);
    assert_eq!(new[0].line, 4);
    assert_eq!(
        store
            .resolve_reference(&source.id, "from.md", "shared title")
            .unwrap()
            .len(),
        1,
        "old title alias must be removed"
    );
    assert_eq!(
        store
            .resolve_reference(&source.id, "from.md", "renamed")
            .unwrap()[0]
            .path,
        "docs/target.md"
    );
    let related = store.related(&source.id, "docs/from.md", 20).unwrap();
    assert_eq!(
        related
            .iter()
            .find(|r| r.path == "docs/target.md")
            .unwrap()
            .line,
        4
    );
    fs::remove_file(notes.path().join("docs/target.md")).unwrap();
    store.index_source(&source.id).unwrap();
    let fallback = store
        .resolve_reference(&source.id, "docs/from.md", "target.md#details")
        .unwrap();
    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0].path, "other/target.md");
    let fresh = store.related(&source.id, "docs/from.md", 20).unwrap();
    assert!(fresh.iter().all(|r| r.path != "docs/target.md"));
    assert!(fresh.iter().any(|r| r.path == "other/target.md"));
    drop(store);
    let store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(
        store.related(&source.id, "docs/from.md", 20).unwrap().len(),
        2
    );
}

#[test]
fn source_scope_revocation_and_derived_migration_keep_authority_boundaries() {
    let data = tempdir().unwrap();
    let selected = tempdir().unwrap();
    let hidden = tempdir().unwrap();
    fs::write(
        selected.path().join("from.md"),
        "# From\n[Next](target.md)\n",
    )
    .unwrap();
    fs::write(selected.path().join("target.md"), "# Target\nSelected.\n").unwrap();
    fs::write(hidden.path().join("target.md"), "# Target\nHidden.\n").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Selected",
            SourceRoot::capture(selected.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    let other = store
        .add_local_source(
            "Hidden",
            SourceRoot::capture(hidden.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    store.index_source(&other.id).unwrap();
    assert_eq!(
        store
            .resolve_reference(&source.id, "from.md", "target")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.related(&other.id, "target.md", 20).unwrap().len(), 0);
    assert!(store
        .search("linked-to:target.md", &[other.id], 20)
        .unwrap()
        .is_empty());
    store.set_source_enabled(&source.id, false).unwrap();
    assert!(store.related(&source.id, "target.md", 20).is_err());
    assert!(store
        .resolve_reference(&source.id, "from.md", "target.md")
        .is_err());
    assert!(store
        .search("linked-to:target.md", std::slice::from_ref(&source.id), 20)
        .is_err());
    store.set_source_enabled(&source.id, true).unwrap();
    drop(store);
    // Reproduce a v3 derived database without touching durable library state.
    let index = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    index.execute_batch("DROP INDEX documents_parser_version; ALTER TABLE documents DROP COLUMN parser_version; DROP TABLE IF EXISTS embedding_memberships; DROP TABLE IF EXISTS resolved_links; DROP TABLE IF EXISTS document_aliases; PRAGMA user_version=3;").unwrap();
    drop(index);
    let store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(
        store.related(&source.id, "target.md", 20).unwrap()[0].path,
        "from.md"
    );
}
