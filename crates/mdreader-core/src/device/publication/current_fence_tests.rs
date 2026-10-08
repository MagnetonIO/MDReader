//! Caller-owned associations use the current fence without document mutation.
use super::*;
fn verified(f: &mut Fixture, id: &str) -> VerifiedCurrent {
    prepare_current(&mut f.store, id, &f.assets, &f.origin)
        .unwrap()
        .verify()
        .unwrap()
}
#[test]
fn current_fence_keeps_state_and_rolls_back_changed_source_assets_or_generation() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let receipt = commit(&mut f, &ready);
    let id = &receipt.document.id;
    let current = verified(&mut f, id);
    f.store
        .with_library_connection(|db| {
            db.execute_batch(
        "CREATE TABLE test_selected_association(document TEXT PRIMARY KEY,revision TEXT NOT NULL)"
    )
        })
        .unwrap();
    // Real callback mutations after provisional association must roll back with it.
    for change in [
        "UPDATE sources SET enabled=0 WHERE id=?1",
        "UPDATE device_asset_items_v1 SET state='retired' WHERE document_id=?1",
        "UPDATE device_documents_v1 SET state_generation=state_generation+1 WHERE id=?1",
    ] {
        let target = if change.contains("UPDATE sources") {
            &receipt.document.source_id
        } else {
            id
        };
        let rejected = with_transition(
            &mut f.store,
            &current,
            &f.origin,
            |db, guard| -> PublicationResult<()> {
                let tx = db.transaction()?;
                revalidate_current(&tx, &guard, &current)?;
                tx.execute(
                    "INSERT INTO test_selected_association VALUES(?1,?2)",
                    params![id, receipt.document.revision],
                )?;
                tx.execute(change, [target])?;
                revalidate_current(&tx, &guard, &current)?;
                tx.commit()?;
                Ok(())
            },
        )
        .unwrap();
        assert!(rejected.is_err());
        assert_eq!(f.count("test_selected_association"), 0);
    }
    with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            tx.execute(
                "INSERT INTO test_selected_association VALUES(?1,?2)",
                params![id, receipt.document.revision],
            )?;
            let changes = tx.total_changes();
            revalidate_current(&tx, &guard, &current)?;
            assert_eq!(tx.total_changes(), changes, "final fence must not write");
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    let before = current.state().clone();
    drop(current);
    drop(ready);
    drop(f.assets);
    drop(f.store);
    let mut reopened = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    assert!(device::state::current(&mut reopened, id).unwrap() == before);
    assert_eq!(
        device::read(&mut reopened, id, Some(&receipt.document.revision))
            .unwrap()
            .markdown
            .as_bytes(),
        Fixture::input().markdown
    );
    let count: u64 = reopened
        .with_library_connection(|db| {
            db.query_row("SELECT count(*) FROM test_selected_association", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(count, 1);
}
#[test]
fn current_fence_rejects_old_future_missing_and_partial_namespace_without_repair() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let receipt = commit(&mut f, &ready);
    let current = verified(&mut f, &receipt.document.id);
    for change in [
        // A coherent supported v1 layout would otherwise be implicitly upgraded
        // by guard_current's compatibility initializer during the final fence.
        "ALTER TABLE device_documents_v1 DROP COLUMN retirement; ALTER TABLE device_documents_v1 DROP COLUMN retired; ALTER TABLE device_documents_v1 DROP COLUMN state_generation; UPDATE device_document_schema_v1 SET version=1",
        "UPDATE device_document_schema_v1 SET version=3",
        "DROP TABLE device_document_schema_v1",
        "DROP TABLE device_import_intents_v1",
    ] {
        with_transition(
            &mut f.store,
            &current,
            &f.origin,
            |db, guard| -> PublicationResult<()> {
                let tx = db.transaction()?;
                tx.execute_batch(change)?;
                let writes = tx.total_changes();
                let schema: u64 = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
                assert!(revalidate_current(&tx, &guard, &current).is_err());
                assert_eq!(tx.total_changes(), writes);
                assert_eq!(
                    tx.query_row("PRAGMA schema_version", [], |r| r.get::<_, u64>(0))?,
                    schema
                );
                // No commit: the caller rolls back its entire callback mutation.
                Ok(())
            },
        )
        .unwrap()
        .unwrap();
    }
    f.store.with_library_connection(|db| {
        let version: u64 = db
            .query_row("SELECT version FROM device_document_schema_v1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, 2);
        db.prepare("SELECT * FROM device_import_intents_v1")
            .unwrap();
    });
}
