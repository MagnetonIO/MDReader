use super::*;

fn snapshot(store: &mut ReaderStore) -> (Vec<String>, Vec<String>) {
    store.with_library_connection(|db| {
        let documents=db.prepare("SELECT quote(id)||'|'||quote(name)||'|'||quote(source_id)||'|'||quote(revision)||'|'||quote(bytes)||'|'||quote(pinned)||'|'||quote(line)||'|'||quote(indexed) FROM device_documents_v1 ORDER BY rowid").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        let intents=db.prepare("SELECT quote(id)||'|'||quote(name)||'|'||quote(revision)||'|'||quote(bytes)||'|'||quote(parent_identity)||'|'||quote(root_identity)||'|'||quote(state) FROM device_import_intents_v1 ORDER BY rowid").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        (documents,intents)
    })
}
fn marker(store: &mut ReaderStore) -> Option<i64> {
    store.with_library_connection(|db| {
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='device_document_schema_v1')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if exists {
            Some(
                db.query_row(
                    "SELECT version FROM device_document_schema_v1 WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )
                .unwrap(),
            )
        } else {
            None
        }
    })
}
#[test]
fn populated_unmarked_layout_adopts_without_reset_and_future_or_malformed_schema_fails_closed() {
    let profile = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(profile.path()).unwrap();
    let saved = import(
        &mut store,
        docs.path(),
        "saved.md".into(),
        "# Saved\n\nExact original\r\n".into(),
    )
    .unwrap();
    let disabled = import(
        &mut store,
        docs.path(),
        "disabled.md".into(),
        "# Disabled".into(),
    )
    .unwrap();
    pin(&mut store, &saved.id, true).unwrap();
    position(&mut store, &saved.id, &saved.revision, 3).unwrap();
    store
        .set_source_enabled(&disabled.source_id, false)
        .unwrap();
    let parent = SourceRoot::capture(docs.path().canonicalize().unwrap()).unwrap();
    let pending = reserve_import(&mut store, &parent, "pending.md".into(), "# Pending").unwrap();
    store
        .with_library_connection(|db| db.execute_batch("DROP TABLE device_document_schema_v1;"))
        .unwrap();
    let before = snapshot(&mut store);
    let original = fs::read(docs.path().join(&saved.id).join("document.md")).unwrap();
    drop(store);
    let mut store = ReaderStore::open(profile.path()).unwrap();
    let documents = list(&mut store).unwrap();
    assert_eq!(documents.len(), 1);
    assert!(documents[0].pinned);
    assert_eq!(documents[0].line, 3);
    assert_eq!(marker(&mut store), Some(1));
    assert_eq!(snapshot(&mut store), before);
    assert_eq!(pending_imports(&mut store).unwrap()[0].id, pending.id);
    assert!(
        !store
            .list_sources()
            .unwrap()
            .iter()
            .find(|s| s.id == disabled.source_id)
            .unwrap()
            .enabled
    );
    assert_eq!(
        fs::read(docs.path().join(&saved.id).join("document.md")).unwrap(),
        original
    );
    let versions = store
        .with_library_connection(|db| {
            db.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
        })
        .unwrap();
    assert_eq!(versions, 4);
    drop(store);
    let mut store = ReaderStore::open(profile.path()).unwrap();
    assert_eq!(snapshot(&mut store), before);
    store
        .with_library_connection(|db| {
            db.execute("UPDATE device_document_schema_v1 SET version=99", [])
        })
        .unwrap();
    assert_eq!(list(&mut store).unwrap_err(), Error::UnsupportedSchema);
    assert_eq!(marker(&mut store), Some(99));
    assert_eq!(snapshot(&mut store), before);
    assert_eq!(
        pin(&mut store, &saved.id, false).unwrap_err(),
        Error::UnsupportedSchema
    );
    store.with_library_connection(|db|db.execute_batch("DROP TABLE device_document_schema_v1; ALTER TABLE device_import_intents_v1 ADD COLUMN future_field TEXT;")).unwrap();
    assert_eq!(list(&mut store).unwrap_err(), Error::InvalidSchema);
    assert_eq!(marker(&mut store), None);
    assert_eq!(snapshot(&mut store), before);
    assert_eq!(
        fs::read(docs.path().join(&saved.id).join("document.md")).unwrap(),
        original
    );

    // Exact docs-only layout can result from interrupted old initialization.
    // Adoption adds an empty intents table; it does not reconstruct history.
    let partial = tempfile::tempdir().unwrap();
    let root = partial.path().join("selected");
    fs::create_dir(&root).unwrap();
    let body = "# Partial\n\nOriginal";
    fs::write(root.join("document.md"), body).unwrap();
    let mut store = ReaderStore::open(&partial.path().join("data")).unwrap();
    let source = store
        .add_local_source(
            "partial.md",
            SourceRoot::capture(root.canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let revision = blake3::hash(body.as_bytes()).to_hex().to_string();
    store.with_library_connection(|db| {
        db.execute_batch(include_str!("migrations/001-device-documents.sql").split(';').next().unwrap()).unwrap();
        db.execute("INSERT INTO device_documents_v1(id,name,source_id,revision,bytes,pinned,line) VALUES(?1,'partial.md',?2,?3,?4,1,3)",params![id,source.id,revision,body.len()]).unwrap();
    });
    // Simulate an owner failure after all adoption DDL but before commit.
    let failed: Result<()> = store.with_library_connection(|db| {
        let tx = db.transaction()?;
        schema::initialize_in(&tx)?;
        Err(Error::Storage)
    });
    assert_eq!(failed, Err(Error::Storage));
    assert_eq!(marker(&mut store), None);
    assert!(!store
        .with_library_connection(|db| db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='device_import_intents_v1')",
            [],
            |r| r.get::<_, bool>(0)
        ))
        .unwrap());
    let recovered = list(&mut store).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, id);
    assert_eq!(recovered[0].line, 3);
    assert!(recovered[0].pinned);
    assert!(snapshot(&mut store).1.is_empty());
    assert_eq!(marker(&mut store), Some(1));
    assert_eq!(
        read(&mut store, &id, Some(&revision)).unwrap().markdown,
        body
    );

    let malformed = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(malformed.path()).unwrap();
    store.with_library_connection(|db|db.execute_batch("CREATE TABLE device_documents_v1(id TEXT PRIMARY KEY,source_id TEXT NOT NULL,revision TEXT NOT NULL); INSERT INTO device_documents_v1 VALUES('opaque','unavailable','kept');")).unwrap();
    assert_eq!(list(&mut store).unwrap_err(), Error::InvalidSchema);
    assert_eq!(marker(&mut store), None);
    assert_eq!(
        store
            .with_library_connection(|db| db.query_row(
                "SELECT revision FROM device_documents_v1",
                [],
                |r| r.get::<_, String>(0)
            ))
            .unwrap(),
        "kept"
    );
}
