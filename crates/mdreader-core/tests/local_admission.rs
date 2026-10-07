use mdreader_core::{ReaderError, ReaderStore, SourceRoot};
use rusqlite::params;

#[test]
fn local_source_and_trusted_metadata_publish_or_rollback_together_across_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let local = temp.path().join("local");
    std::fs::create_dir(&local).unwrap();
    std::fs::write(
        local.join("document.md"),
        "## Synthetic\nDurable local import.\n",
    )
    .unwrap();
    let authority = SourceRoot::capture(local.canonicalize().unwrap()).unwrap();
    let mut store = ReaderStore::open(&data).unwrap();
    store.with_library_connection(|db|db.execute_batch("CREATE TABLE synthetic_publication(id TEXT PRIMARY KEY,source_id TEXT NOT NULL REFERENCES sources(id));")).unwrap();
    let failed = store.add_local_source_with(
        "Synthetic",
        authority.clone(),
        |tx, source| -> mdreader_core::Result<()> {
            tx.execute(
                "INSERT INTO synthetic_publication VALUES('failed',?1)",
                [&source.id],
            )?;
            Err(ReaderError::Invalid("synthetic publisher failure".into()))
        },
    );
    assert!(failed.is_err());
    drop(store);
    let mut store = ReaderStore::open(&data).unwrap();
    assert!(store.list_sources().unwrap().is_empty());
    assert_eq!(
        store
            .with_library_connection(|db| db.query_row(
                "SELECT count(*) FROM synthetic_publication",
                [],
                |row| row.get::<_, u32>(0)
            ))
            .unwrap(),
        0
    );
    let (source, published) = store
        .add_local_source_with("Synthetic", authority.clone(), |tx, source| {
            assert!(source.enabled);
            tx.execute(
                "INSERT INTO synthetic_publication VALUES('complete',?1)",
                [&source.id],
            )?;
            Ok("complete")
        })
        .unwrap();
    assert_eq!(published, "complete");
    drop(store);
    let mut store = ReaderStore::open(&data).unwrap();
    let linked: String = store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT source_id FROM synthetic_publication WHERE id='complete'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(linked, source.id);
    assert_eq!(
        store
            .read_document(&source.id, "document.md", None)
            .unwrap()
            .markdown,
        "## Synthetic\nDurable local import.\n"
    );
    store.set_source_enabled(&source.id, false).unwrap();
    let (same, ()) = store
        .add_local_source_with("Updated name", authority, |_tx, source| {
            assert!(!source.enabled);
            Ok(())
        })
        .unwrap();
    assert_eq!(same.id, source.id);
    assert!(!same.enabled);
    assert!(store.read_document(&same.id, "document.md", None).is_err());
    // The retained root must still be the admitted object after the publisher
    // returns; a successful callback cannot commit a replacement directory.
    let changing = temp.path().join("changing");
    std::fs::create_dir(&changing).unwrap();
    let moved = temp.path().join("moved");
    let changing_root = SourceRoot::capture(changing.canonicalize().unwrap()).unwrap();
    let invalidated = store.add_local_source_with("Changing root", changing_root, |tx, source| {
        tx.execute(
            "INSERT INTO synthetic_publication VALUES(?1,?2)",
            params!["invalidated", source.id],
        )?;
        std::fs::rename(&changing, &moved)?;
        std::fs::create_dir(&changing)?;
        Ok(())
    });
    assert!(invalidated.is_err());
    drop(store);
    let store = ReaderStore::open_readonly(&data).unwrap();
    assert_eq!(store.list_sources().unwrap().len(), 1);
    assert!(!store.list_sources().unwrap()[0].enabled);
    drop(store);
    let mut store = ReaderStore::open(&data).unwrap();
    assert_eq!(
        store
            .with_library_connection(|db| db.query_row(
                "SELECT count(*) FROM synthetic_publication",
                [],
                |r| r.get::<_, u32>(0)
            ))
            .unwrap(),
        1
    );
}
