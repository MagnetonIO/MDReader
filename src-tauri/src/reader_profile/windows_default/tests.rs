use super::*;

#[test]
fn temporary_selector_shadow_refuses_read_and_cas_and_keeps_durable_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    let original = parent.join("standard-original");
    let slot = DefaultSelection::open(&original).unwrap();
    let reader = slot.fresh_reader().unwrap();
    let binding = RootBinding::capture_private(&reader.path).unwrap();
    let encoded = serde_json::to_string(&binding).unwrap();
    let expected = {
        let mut owner = slot.owner.lock().unwrap();
        owner.store.with_library_connection(|db| {
            db.execute(
                "UPDATE main.native_default_windows_v1 SET generation=7,target=?1 WHERE singleton=1",
                [&encoded],
            )
        }).unwrap();
        let actual = owner.store.with_library_connection(|db| read(db)).unwrap();
        owner.expected = actual.clone();
        let ddl: String = owner
            .store
            .with_library_connection(|db| {
                db.query_row(
                    "SELECT sql FROM main.sqlite_schema WHERE name='native_default_windows_v1'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        owner.store.with_library_connection(|db| db.execute_batch(&format!(
            "{}; INSERT INTO temp.native_default_windows_v1 SELECT * FROM main.native_default_windows_v1;",
            ddl.replacen("CREATE TABLE", "CREATE TEMP TABLE", 1),
        ))).unwrap();
        actual
    };
    assert!(matches!(slot.startup_selection(), Err(Error::Conflict)));
    let before = slot
        .owner
        .lock()
        .unwrap()
        .store
        .with_library_connection(|db| db.total_changes());
    assert!(matches!(slot.publish(None), Err(Error::Conflict)));
    {
        let mut owner = slot.owner.lock().unwrap();
        assert_eq!(
            owner.store.with_library_connection(|db| db.total_changes()),
            before
        );
        assert!(owner.expected == expected);
        for namespace in ["main", "temp"] {
            let (generation, target): (u64, Option<String>) = owner.store.with_library_connection(|db| db.query_row(
                &format!("SELECT generation,target FROM {namespace}.native_default_windows_v1 WHERE singleton=1"),
                [], |r| Ok((r.get(0)?, r.get(1)?)),
            )).unwrap();
            assert_eq!(generation, 7);
            assert_eq!(target.as_deref(), Some(encoded.as_str()));
        }
    }
    // No adapter deletes the TEMP table. Closing its owning connection retires
    // only SQLite's temporary namespace; reopen observes unchanged durable CAS.
    drop(slot);
    let reopened = DefaultSelection::open(&original).unwrap();
    assert!(reopened.owner.lock().unwrap().expected == expected);
    reopened.publish(None).unwrap();
    let (generation, target): (u64, Option<String>) = reopened
        .owner
        .lock()
        .unwrap()
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT generation,target FROM main.native_default_windows_v1 WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(generation, 8);
    assert!(target.is_none());
    assert!(!original.exists());
}

#[test]
fn default_reopens_without_original_and_refuses_stale_or_triggered_cas() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    let original = parent.join("standard-original");
    let slot = DefaultSelection::open(&original).unwrap();
    let stale = DefaultSelection::open(&original).unwrap();
    assert!(!original.exists());
    let (selected, imported) = slot.startup_selection().unwrap();
    assert!(selected.is_none());
    assert!(!imported);
    slot.owner
        .lock()
        .unwrap()
        .store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE native_default_windows_v1 SET generation=2 WHERE singleton=1",
                [],
            )
        })
        .unwrap();
    assert!(matches!(stale.startup_selection(), Err(Error::Conflict)));
    assert!(matches!(stale.publish(None), Err(Error::Conflict)));
    drop(slot);
    drop(stale);
    let reopened = DefaultSelection::open(&original).unwrap();
    assert!(reopened.selected().unwrap().is_none());
    reopened.owner.lock().unwrap().store.with_library_connection(|db| {
        db.execute_batch("CREATE TRIGGER rejected_default AFTER UPDATE ON native_default_windows_v1 BEGIN UPDATE native_default_windows_v1 SET target=NULL; END;")
    }).unwrap();
    assert!(matches!(reopened.publish(None), Err(Error::Conflict)));
    let generation: u64 = reopened
        .owner
        .lock()
        .unwrap()
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT generation FROM native_default_windows_v1",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(generation, 2);
    assert!(!original.exists());
}
