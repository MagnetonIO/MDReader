use mdreader_core::{publish_local_source, ReaderError, ReaderStore, SourceRoot};
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

#[test]
fn external_transaction_publishes_or_rolls_back_source_owner_and_synthetic_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let local = temp.path().join("local");
    std::fs::create_dir(&local).unwrap();
    let original = "\u{feff}# Exact synthetic\r\nπ\r\n";
    std::fs::write(local.join("document.md"), original).unwrap();
    let authority = SourceRoot::capture(local.canonicalize().unwrap()).unwrap();
    let mut store = ReaderStore::open(&data).unwrap();
    store
        .with_library_connection(|db| {
            db.execute_batch(
                "CREATE TABLE synthetic_owner(source_id TEXT PRIMARY KEY REFERENCES sources(id));
         CREATE TABLE synthetic_cursor(applied INTEGER NOT NULL);
         INSERT INTO synthetic_cursor VALUES(0);",
            )
        })
        .unwrap();
    let failed: mdreader_core::Result<()> = store.with_library_connection(|db| {
        let transaction = db.transaction()?;
        publish_local_source(&transaction, "External", authority.clone(), |tx, source| {
            tx.execute("INSERT INTO synthetic_owner VALUES(?1)", [&source.id])?;
            tx.execute("UPDATE synthetic_cursor SET applied=1", [])?;
            Ok(())
        })?;
        // The helper must not commit: a later owner failure aborts the same unit.
        Err(ReaderError::Conflict)
    });
    assert!(matches!(failed, Err(ReaderError::Conflict)));
    drop(store);
    let mut store = ReaderStore::open(&data).unwrap();
    assert!(store.list_sources().unwrap().is_empty());
    let state = store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT (SELECT count(*) FROM synthetic_owner),applied FROM synthetic_cursor",
                [],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
            )
        })
        .unwrap();
    assert_eq!(state, (0, 0));
    assert_eq!(
        std::fs::read(local.join("document.md")).unwrap(),
        original.as_bytes()
    );
    let source = store
        .with_library_connection(|db| -> mdreader_core::Result<_> {
            let transaction = db.transaction()?;
            let (source, ()) =
                publish_local_source(&transaction, "External", authority, |tx, source| {
                    assert!(source.enabled);
                    tx.execute("INSERT INTO synthetic_owner VALUES(?1)", [&source.id])?;
                    tx.execute("UPDATE synthetic_cursor SET applied=1", [])?;
                    Ok(())
                })?;
            transaction.commit()?;
            Ok(source)
        })
        .unwrap();
    drop(store);
    let mut store = ReaderStore::open(&data).unwrap();
    assert_eq!(store.list_sources().unwrap()[0].id, source.id);
    let state = store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT source_id,(SELECT applied FROM synthetic_cursor) FROM synthetic_owner",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)),
            )
        })
        .unwrap();
    assert_eq!(state, (source.id.clone(), 1));
    assert_eq!(
        store
            .read_document(&source.id, "document.md", None)
            .unwrap()
            .markdown,
        original
    );
    assert_eq!(store.index_status(&[source.id]).unwrap().documents, 0);
}

#[test]
fn external_transaction_rechecks_cache_overlap_after_callback_and_rolls_back() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let local = temp.path().join("local");
    let child_cache = local.join("private-cache");
    std::fs::create_dir_all(&child_cache).unwrap();
    let authority = SourceRoot::capture(local.canonicalize().unwrap()).unwrap();
    let mut store = ReaderStore::open(&data).unwrap();
    store
        .with_library_connection(|db| {
            db.execute_batch(
                "CREATE TABLE synthetic_owner(source_id TEXT PRIMARY KEY REFERENCES sources(id));",
            )
        })
        .unwrap();
    // Reject both a cache below the source and a source below the cache.
    for private in [
        child_cache.canonicalize().unwrap(),
        temp.path().canonicalize().unwrap(),
    ] {
        let cache_root = SourceRoot::capture(private.clone()).unwrap();
        #[cfg(unix)]
        let encoded = {
            use std::os::unix::ffi::OsStrExt;
            private.as_os_str().as_bytes().to_vec()
        };
        #[cfg(windows)]
        let encoded: Vec<u8> = {
            use std::os::windows::ffi::OsStrExt;
            private
                .as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        let failed: mdreader_core::Result<()> = store.with_library_connection(|db| {
            let transaction = db.transaction()?;
            publish_local_source(&transaction, "External", authority.clone(), |tx, source| {
                tx.execute("INSERT INTO synthetic_owner VALUES(?1)", [&source.id])?;
                tx.execute(
                    "INSERT INTO core_provider_profile VALUES(1,'synthetic-library',?1,?2)",
                    params![
                        encoded,
                        serde_json::to_string(&cache_root.identity).unwrap()
                    ],
                )?;
                Ok(())
            })?;
            transaction.commit()?;
            Ok(())
        });
        assert!(matches!(failed, Err(ReaderError::Invalid(_))));
    }
    drop(store);
    let mut store = ReaderStore::open(&data).unwrap();
    assert!(store.list_sources().unwrap().is_empty());
    let counts = store.with_library_connection(|db| db.query_row(
        "SELECT (SELECT count(*) FROM synthetic_owner),(SELECT count(*) FROM core_provider_profile)", [],
        |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
    )).unwrap();
    assert_eq!(counts, (0, 0));
}
