use super::*;

#[test]
fn canonical_reserved_body_recovers_partial_projection_reopens_and_never_regrants() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch
        .path()
        .canonicalize()
        .unwrap()
        .join("private-reader");
    let private = PrivateDirectory::create(&path).unwrap();
    let docs = private.create_child(Path::new("documents")).unwrap();
    let directory = path.join("documents");
    let parent = SourceRoot::capture(directory.clone()).unwrap();
    let library = path.join("reader-v1");
    let mut store = ReaderStore::open(&library).unwrap();
    let text = "\u{feff}# Kept\r\n\r\nOriginal naïve Windows bytes.\r\n";
    let mut intent =
        super::super::imports::reserve_import(&mut store, &parent, "kept.md".into(), text).unwrap();
    let untouched = super::super::imports::reserve_import(
        &mut store,
        &parent,
        "unsent.md".into(),
        "# Not projected",
    )
    .unwrap();
    assert!(store.list_sources().unwrap().is_empty());
    discard_import(&mut store, &directory, &untouched.id).unwrap();

    // Persist the actual child identity, then interrupt while only a canonical
    // prefix is flushed. No source or document exists yet.
    let child = docs.create_child(Path::new(&intent.id)).unwrap();
    let root = SourceRoot::capture(directory.join(&intent.id)).unwrap();
    intent.root_identity = Some(serde_json::to_string(&root.identity).unwrap());
    store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_import_intents_v1 SET root_identity=?2 WHERE id=?1",
                params![intent.id, intent.root_identity],
            )
        })
        .unwrap();
    let mut file = child
        .projection_file(Path::new(BODY), true)
        .unwrap()
        .unwrap();
    file.write_all(&text.as_bytes()[..7]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    drop(store);
    let mut store = ReaderStore::open(&library).unwrap();
    let recovered = super::super::recover_imports(&mut store, &directory).unwrap();
    assert!(recovered.pending.is_empty());
    assert_eq!(recovered.documents.len(), 1);
    let document = &recovered.documents[0];
    assert_eq!(
        super::super::read(&mut store, &document.id, Some(&document.revision))
            .unwrap()
            .markdown,
        text
    );
    super::super::pin(&mut store, &document.id, true).unwrap();
    super::super::position(&mut store, &document.id, &document.revision, 3).unwrap();
    let canonical: Vec<u8> = store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT body FROM device_windows_bodies_v1 WHERE id=?1",
                [&document.id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(canonical, text.as_bytes());
    assert!(publish_import(&mut store, &directory, &intent).is_err());
    store
        .set_source_enabled(&document.source_id, false)
        .unwrap();
    drop(store);
    let mut store = ReaderStore::open(&library).unwrap();
    assert!(super::super::recover_imports(&mut store, &directory)
        .unwrap()
        .documents
        .is_empty());
    assert!(super::super::read(&mut store, &document.id, None).is_err());
    assert_eq!(
        fs::read(directory.join(&document.id).join(BODY)).unwrap(),
        text.as_bytes()
    );
    // Disabled grant and canonical archival bytes remain; recovery is never an
    // authorization fallback. A future marker blocks new publication unchanged.
    store.with_library_connection(|db| db.execute_batch("PRAGMA ignore_check_constraints=ON; UPDATE device_windows_body_schema_v1 SET version=2; PRAGMA ignore_check_constraints=OFF;")).unwrap();
    assert!(super::super::import(
        &mut store,
        &directory,
        "future.md".into(),
        "# Future".into()
    )
    .is_err());
    assert_eq!(store.list_sources().unwrap().len(), 1);
}
