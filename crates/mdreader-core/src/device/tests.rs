use super::*;

#[test]
fn app_container_relocation_preserves_exact_imports_and_never_revives_revoked_sources() {
    let sandbox = tempfile::tempdir().unwrap();
    let old = sandbox.path().join("old-container");
    fs::create_dir(&old).unwrap();
    let docs = old.join("device-markdown-v1");
    let mut store = ReaderStore::open(&old.join("reader-v1")).unwrap();
    let text = "\u{feff}# Moved\r\n\r\nSearchable original bytes.\r\n";
    let saved = import(&mut store, &docs, "moved.md".into(), text.into()).unwrap();
    let revoked = import(&mut store, &docs, "revoked.md".into(), "# Hidden".into()).unwrap();
    position(&mut store, &saved.id, &saved.revision, 3).unwrap();
    store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_documents_v1 SET pinned=1 WHERE id=?1",
                [&saved.id],
            )
        })
        .unwrap();
    store.set_source_enabled(&revoked.source_id, false).unwrap();
    drop(store);
    let new = sandbox.path().join("new-container");
    fs::rename(&old, &new).unwrap();
    let mut store = ReaderStore::open(&new.join("reader-v1")).unwrap();
    let library = recover_imports(&mut store, &new.join("device-markdown-v1")).unwrap();
    assert_eq!(library.documents.len(), 1);
    let actual = read(&mut store, &saved.id, Some(&saved.revision)).unwrap();
    assert_eq!(actual.markdown, text);
    assert_eq!(actual.document.source_id, saved.source_id);
    assert!(actual.document.pinned);
    assert_eq!(actual.document.line, 3);
    assert_eq!(search(&mut store, "Searchable").unwrap().len(), 1);
    assert!(read(&mut store, &revoked.id, None).is_err());
    assert!(
        !store
            .list_sources()
            .unwrap()
            .iter()
            .find(|source| source.id == revoked.source_id)
            .unwrap()
            .enabled
    );
    // A replacement directory must not acquire the old grant, even if its
    // Markdown bytes match. Keep the old object alive to avoid inode reuse.
    let root = new.join("device-markdown-v1").join(&saved.id);
    fs::rename(&root, root.with_extension("retained")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("document.md"), text).unwrap();
    assert!(recover_imports(&mut store, &new.join("device-markdown-v1")).is_err());
    assert!(read(&mut store, &saved.id, None).is_err());
}
#[test]
fn interrupted_imports_reserve_quota_and_publish_grant_atomically_after_reopen() {
    let profile = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let parent = SourceRoot::capture(docs.path().canonicalize().unwrap()).unwrap();
    let mut store = ReaderStore::open(profile.path()).unwrap();
    let mut complete =
        reserve_import(&mut store, &parent, "complete.md".into(), "# Complete").unwrap();
    write_import(&mut store, &parent, &mut complete, "# Complete").unwrap();
    store.with_library_connection(|db| db.execute_batch("CREATE TEMP TRIGGER reject_device_metadata BEFORE INSERT ON device_documents_v1 BEGIN SELECT RAISE(ABORT,'simulated metadata failure'); END;")).unwrap();
    assert!(publish_import(&mut store, docs.path(), &complete).is_err());
    assert!(store.list_sources().unwrap().is_empty());
    assert_eq!(pending_imports(&mut store).unwrap().len(), 1);
    drop(store); // The durable complete copy recovers; the failed grant does not.
    let mut store = ReaderStore::open(profile.path()).unwrap();
    let recovered = recover_imports(&mut store, docs.path()).unwrap();
    assert!(recovered.pending.is_empty());
    assert_eq!(recovered.documents.len(), 1);
    assert_eq!(
        read(&mut store, &complete.id, None).unwrap().markdown,
        "# Complete"
    );
    let mut partial = reserve_import(
        &mut store,
        &parent,
        "partial.md".into(),
        "# Expected complete body",
    )
    .unwrap();
    write_import(&mut store, &parent, &mut partial, "# Partial").unwrap();
    drop(store);
    let mut store = ReaderStore::open(profile.path()).unwrap();
    let snapshot = recover_imports(&mut store, docs.path()).unwrap();
    assert_eq!(snapshot.documents.len(), 1);
    assert_eq!(snapshot.pending[0].id, partial.id);
    assert_eq!(store.list_sources().unwrap().len(), 1);
    assert!(document(&mut store, &partial.id).is_err());
    // Quota checks include unpublished copies without thousands of imports.
    store.with_library_connection(|db|db.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<998) INSERT INTO device_import_intents_v1 SELECT 'quota-'||i,'quota.md',?1,1,?2,NULL,'reserved' FROM n",params![complete.revision,complete.parent_identity])).unwrap();
    assert!(matches!(
        reserve_import(&mut store, &parent, "over-limit.md".into(), "x"),
        Err(Error::Quota)
    ));
    store
        .with_library_connection(|db| {
            db.execute(
                "DELETE FROM device_import_intents_v1 WHERE id LIKE 'quota-%'",
                [],
            )
        })
        .unwrap();
    // Simulate a crash after deleting the incomplete directory but before
    // releasing its SQLite reservation: retry is safe and bounded.
    fs::remove_file(parent.path.join(&partial.id).join("document.md")).unwrap();
    fs::remove_dir(parent.path.join(&partial.id)).unwrap();
    discard_import(&mut store, docs.path(), &partial.id).unwrap();
    assert!(pending_imports(&mut store).unwrap().is_empty());
    assert!(parent.path.join(&complete.id).join("document.md").exists());
    assert!(discard_import(&mut store, docs.path(), &complete.id).is_err());
}
#[test]
fn imported_bytes_fulltext_pins_and_revision_bound_position_survive_reopen_and_index_loss() {
    let profile = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(profile.path()).unwrap();
    let text = "\u{feff}# Mobile\r\n\r\nOffline searchable naïve.\r\n";
    let imported = import(&mut store, docs.path(), "notes.md".into(), text.into()).unwrap();
    assert_eq!(read(&mut store, &imported.id, None).unwrap().markdown, text);
    assert_eq!(
        search(&mut store, "searchable").unwrap()[0].document.id,
        imported.id
    );
    position(&mut store, &imported.id, &imported.revision, 3).unwrap();
    assert!(position(&mut store, &imported.id, "stale", 2).is_err());
    assert!(position(&mut store, &imported.id, &imported.revision, 999).is_err());
    store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_documents_v1 SET pinned=1 WHERE id=?1",
                [&imported.id],
            )
        })
        .unwrap();
    drop(store);
    fs::remove_file(profile.path().join("index.sqlite")).unwrap();
    let mut store = ReaderStore::open(profile.path()).unwrap();
    let actual = list(&mut store).unwrap().pop().unwrap();
    assert!(actual.pinned);
    assert_eq!(actual.line, 3);
    assert_eq!(read(&mut store, &imported.id, None).unwrap().markdown, text);
    store.rebuild_index().unwrap();
    assert_eq!(search(&mut store, "searchable").unwrap().len(), 1);
    store
        .set_source_enabled(&imported.source_id, false)
        .unwrap();
    assert!(list(&mut store).unwrap().is_empty());
    assert!(read(&mut store, &imported.id, None).is_err());
    assert!(search(&mut store, "searchable").unwrap().is_empty());
    assert!(docs.path().join(imported.id).join("document.md").exists());
}
#[test]
fn import_bounds_duplicate_names_and_changed_body_are_explicit() {
    let profile = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(profile.path()).unwrap();
    assert!(import(&mut store, docs.path(), "../escape.md".into(), "x".into()).is_err());
    assert!(import(
        &mut store,
        docs.path(),
        "huge.md".into(),
        "x".repeat(MAX_BYTES + 1)
    )
    .is_err());
    let a = import(&mut store, docs.path(), "same.md".into(), "# First".into()).unwrap();
    let b = import(&mut store, docs.path(), "same.md".into(), "# Second".into()).unwrap();
    assert_ne!(a.id, b.id);
    fs::write(docs.path().join(&a.id).join("document.md"), "# Changed").unwrap();
    assert!(read(&mut store, &a.id, Some(&a.revision)).is_err());
    assert_eq!(read(&mut store, &b.id, None).unwrap().markdown, "# Second");
    assert!(document(&mut store, "../outside").is_err());
}
