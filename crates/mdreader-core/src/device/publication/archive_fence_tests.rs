//! Actual owner transactions retain file evidence until publication completes.
use super::*;

fn selected(f: &mut Fixture, id: &str) -> (VerifiedCurrent, ArchivedDocument) {
    let current = prepare_current(&mut f.store, id, &f.assets, &f.origin)
        .unwrap()
        .verify()
        .unwrap();
    let archive = prepare_archive(&mut f.store, id, &f.assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    (current, archive)
}

#[test]
fn final_archive_fence_rolls_back_association_on_changed_or_replaced_asset_files() {
    for mutation in ["write", "replace", "remove", "root"] {
        let mut f = Fixture::new();
        let ready = f.ready();
        let receipt = commit(&mut f, &ready);
        let (current, archive) = selected(&mut f, &receipt.document.id);
        f.store
            .with_library_connection(|db| {
                db.execute_batch("CREATE TABLE test_selected_files(document TEXT)")
            })
            .unwrap();
        let asset_root = f.temp.path().join("assets");
        let path = asset_root.join(format!("{}.bin", receipt.assets[0].asset_id));
        let unchanged = current.state().clone();
        let outcome = with_transition(
            &mut f.store,
            &current,
            &f.origin,
            |db, guard| -> PublicationResult<()> {
                let tx = db.transaction()?;
                let writes = tx.total_changes();
                revalidate_current_archive(&tx, &guard, &current, &archive)?;
                assert_eq!(tx.total_changes(), writes, "fence must be read-only");
                tx.execute(
                    "INSERT INTO test_selected_files VALUES(?1)",
                    [&receipt.document.id],
                )?;
                match mutation {
                    "write" => {
                        let mut bytes = std::fs::read(&path)?;
                        bytes[0] ^= 1;
                        std::fs::write(&path, bytes)?;
                    }
                    "replace" => {
                        let replacement = f.temp.path().join("replacement.bin");
                        // Equal bytes still belong to a different file object.
                        std::fs::write(&replacement, std::fs::read(&path)?)?;
                        std::fs::rename(replacement, &path)?;
                    }
                    "remove" => std::fs::remove_file(&path)?,
                    "root" => {
                        std::fs::rename(&asset_root, f.temp.path().join("old-assets"))?;
                        std::fs::create_dir(&asset_root)?;
                    }
                    _ => unreachable!(),
                }
                revalidate_current_archive(&tx, &guard, &current, &archive)?;
                tx.commit()?;
                Ok(())
            },
        )
        .unwrap();
        assert!(outcome.is_err(), "accepted changed asset: {mutation}");
        assert_eq!(f.count("test_selected_files"), 0);
        assert!(device::state::current(&mut f.store, &receipt.document.id).unwrap() == unchanged);
    }
}

#[test]
fn archive_fence_binds_exact_owner_metadata_and_complete_resources() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let receipt = commit(&mut f, &ready);
    let (current, archive) = selected(&mut f, &receipt.document.id);
    with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            revalidate_current_archive(&tx, &guard, &current, &archive)?;
            let writes = tx.total_changes();
            let schema: u64 = tx.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
            revalidate_current_archive(&tx, &guard, &current, &archive)?;
            assert_eq!(tx.total_changes(), writes);
            assert_eq!(
                tx.query_row("PRAGMA schema_version", [], |r| r.get::<_, u64>(0))?,
                schema
            );
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap()
    .unwrap();

    // A second actual owner sees identical rows/files, but cannot adopt this proof.
    let mut reopened = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    let reopened_assets = AssetStore::open(&mut reopened, &f.temp.path().join("assets")).unwrap();
    let next = prepare_current(
        &mut reopened,
        &receipt.document.id,
        &reopened_assets,
        &f.origin,
    )
    .unwrap()
    .verify()
    .unwrap();
    assert!(next.state() == archive.state());
    with_transition(
        &mut reopened,
        &next,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            revalidate_current(&tx, &guard, &next)?;
            assert!(revalidate_current_archive(&tx, &guard, &next, &archive).is_err());
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    drop(reopened_assets);
    drop(reopened);

    // Same document state, but a later asset observation cannot use earlier bytes.
    f.store
        .with_library_connection(|db| {
            db.execute(
        "UPDATE device_asset_generations_v1 SET generation=generation+1 WHERE document_id=?1",
        [&receipt.document.id],
    )
        })
        .unwrap();
    let later = prepare_current(&mut f.store, &receipt.document.id, &f.assets, &f.origin)
        .unwrap()
        .verify()
        .unwrap();
    with_transition(
        &mut f.store,
        &later,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            revalidate_current(&tx, &guard, &later)?;
            assert!(revalidate_current_archive(&tx, &guard, &later, &archive).is_err());
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    drop(archive);

    let asset = f
        .temp
        .path()
        .join("assets")
        .join(format!("{}.bin", receipt.assets[0].asset_id));
    std::fs::remove_file(asset).unwrap();
    let incomplete = prepare_archive(&mut f.store, &receipt.document.id, &f.assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    assert!(
        incomplete.assets()[0].bytes().is_none(),
        "archival metadata remains exportable"
    );
    with_transition(
        &mut f.store,
        &later,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            revalidate_current(&tx, &guard, &later)?;
            assert!(matches!(
                revalidate_current_archive(&tx, &guard, &later, &incomplete),
                Err(PublicationError::Incomplete)
            ));
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
}
