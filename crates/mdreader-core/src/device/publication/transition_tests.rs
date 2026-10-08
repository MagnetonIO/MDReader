use super::*;
use crate::device::state::{self, RetirementKind, RetirementRecord};
use std::time::Instant;

fn verified(f: &mut Fixture, id: &str) -> VerifiedCurrent {
    prepare_current(&mut f.store, id, &f.assets, &f.origin)
        .unwrap()
        .verify()
        .unwrap()
}
fn deleted(f: &Fixture) -> RetirementRecord {
    RetirementRecord {
        kind: RetirementKind::Deleted,
        operation_id: f.origin.wire_revision,
        replacement: None,
    }
}
#[test]
fn local_state_aba_wrong_owner_and_final_callback_changes_are_fenced() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let receipt = commit(&mut f, &ready);
    let id = &receipt.document.id;
    let before = verified(&mut f, id);
    device::pin(&mut f.store, id, true).unwrap();
    device::pin(&mut f.store, id, false).unwrap();
    let mut now = state::current(&mut f.store, id).unwrap();
    assert_eq!(now.generation, 2);
    assert!(!now.document.pinned);
    let stale = with_transition(
        &mut f.store,
        &before,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            publish_state(
                &tx,
                &guard,
                &before,
                StateUpdate {
                    pinned: true,
                    position: PositionUpdate::Preserve,
                },
            )?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(
        stale.is_err(),
        "equal pin values do not erase intervening local edits"
    );
    let grant_before = verified(&mut f, id);
    f.store
        .set_source_enabled(&receipt.document.source_id, false)
        .unwrap();
    f.store
        .set_source_enabled(&receipt.document.source_id, true)
        .unwrap();
    now = state::current(&mut f.store, id).unwrap();
    assert_eq!(now.generation, 4);
    let grant_aba = with_transition(
        &mut f.store,
        &grant_before,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            publish_state(
                &tx,
                &guard,
                &grant_before,
                StateUpdate {
                    pinned: true,
                    position: PositionUpdate::Preserve,
                },
            )?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(
        grant_aba.is_err(),
        "permission ABA cannot restore a prepared transition"
    );
    let current = verified(&mut f, id);
    let rejected = with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            let receipt = publish_state(
                &tx,
                &guard,
                &current,
                StateUpdate {
                    pinned: true,
                    position: PositionUpdate::Line(3),
                },
            )?;
            tx.execute("UPDATE device_documents_v1 SET line=1 WHERE id=?1", [id])?;
            revalidate_state(&tx, &guard, &current, &receipt)?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(rejected.is_err());
    assert!(state::current(&mut f.store, id).unwrap() == now);
    let mut wrong = rusqlite::Connection::open_in_memory().unwrap();
    let rejected = with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |_, guard| -> PublicationResult<()> {
            let tx = wrong.transaction()?;
            publish_state(
                &tx,
                &guard,
                &current,
                StateUpdate {
                    pinned: true,
                    position: PositionUpdate::Start,
                },
            )?;
            Ok(())
        },
    )
    .unwrap();
    assert!(matches!(rejected, Err(PublicationError::Conflict)));
    drop(ready);
    drop(f.store);
    f.store = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    assert!(
        with_transition(&mut f.store, &current, &f.origin, |_, _| Ok::<
            _,
            PublicationError,
        >(()))
        .is_err()
    );
    assert!(state::current(&mut f.store, id).unwrap() == now);
}
#[test]
fn exact_state_line_bounds_commit_and_generation_limit_preserve_originals() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let original = commit(&mut f, &ready);
    let current = verified(&mut f, &original.document.id);
    let too_far = current.max_line() + 1;
    let invalid = with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            publish_state(
                &tx,
                &guard,
                &current,
                StateUpdate {
                    pinned: true,
                    position: PositionUpdate::Line(too_far),
                },
            )?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(matches!(
        invalid,
        Err(PublicationError::Device(device::Error::InvalidPosition))
    ));
    let receipt = with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<_> {
            let tx = db.transaction()?;
            let receipt = publish_state(
                &tx,
                &guard,
                &current,
                StateUpdate {
                    pinned: true,
                    position: PositionUpdate::Line(3),
                },
            )?;
            revalidate_state(&tx, &guard, &current, &receipt)?;
            tx.commit()?;
            Ok(receipt)
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(receipt.state().generation, 1);
    assert_eq!(receipt.state().document.line, 3);
    assert!(receipt.state().document.pinned);
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_documents_v1 SET state_generation=9007199254740991 WHERE id=?1",
                [&original.document.id],
            )
        })
        .unwrap();
    assert_eq!(
        device::pin(&mut f.store, &original.document.id, false).unwrap_err(),
        device::Error::StateLimit
    );
    assert_eq!(
        device::read(
            &mut f.store,
            &original.document.id,
            Some(&original.document.revision)
        )
        .unwrap()
        .markdown
        .as_bytes(),
        Fixture::input().markdown
    );
    f.store
        .set_source_enabled(&original.document.source_id, false)
        .unwrap();
    assert!(f
        .store
        .set_source_enabled(&original.document.source_id, true)
        .is_err());
    assert!(device::read(&mut f.store, &original.document.id, None).is_err());
    let enabled: bool = f
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT enabled FROM sources WHERE id=?1",
                [&original.document.source_id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(
        !enabled,
        "generation exhaustion must not prevent durable revocation"
    );
}
#[test]
fn immutable_replacement_rolls_back_then_archives_old_evidence_and_preserves_pin() {
    let mut f = Fixture::new();
    let first = f.ready();
    let old = commit(&mut f, &first);
    drop(first);
    device::pin(&mut f.store, &old.document.id, true).unwrap();
    device::position(&mut f.store, &old.document.id, &old.document.revision, 3).unwrap();
    f.store.index_source(&old.document.source_id).unwrap();
    f.origin.wire_revision = uuid::Uuid::new_v4();
    let current = verified(&mut f, &old.document.id);
    let ready = f.ready(); // Deliberately identical bytes, distinct immutable revision.
    let rolled_back = with_replacement(
        &mut f.store,
        &ready,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            publish_replacement(&tx, &guard, &ready, &current)?;
            Err(PublicationError::Conflict)
        },
    )
    .unwrap();
    assert!(rolled_back.is_err());
    assert_eq!(f.count("device_documents_v1"), 1);
    assert!(device::read(&mut f.store, &old.document.id, None).is_ok());
    let receipt = with_replacement(
        &mut f.store,
        &ready,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<_> {
            let tx = db.transaction()?;
            let receipt = publish_replacement(&tx, &guard, &ready, &current)?;
            revalidate_replacement(&tx, &guard, &ready, &current, &receipt)?;
            tx.commit()?;
            Ok(receipt)
        },
    )
    .unwrap()
    .unwrap();
    let new = &receipt.publication().document;
    assert_ne!(new.id, old.document.id);
    assert_ne!(new.source_id, old.document.source_id);
    assert_eq!(new.revision, old.document.revision);
    assert!(new.pinned);
    assert_eq!(
        new.line, 1,
        "raw hash equality does not transfer old revision positions"
    );
    assert!(f
        .store
        .read_document(
            &old.document.source_id,
            "document.md",
            Some(&old.document.revision)
        )
        .is_err());
    assert!(f
        .store
        .search("Exact", std::slice::from_ref(&old.document.source_id), 10)
        .is_err());
    assert!(f
        .store
        .set_source_enabled(&old.document.source_id, true)
        .is_err());
    assert_eq!(f.count("device_documents_v1"), 2);
    let inventory = f.assets.inventory(&mut f.store).unwrap();
    assert_eq!(inventory.items.len(), 2);
    assert_eq!(inventory.reserved_bytes, 2 * old.assets[0].bytes);
    let archive = prepare_archive(&mut f.store, &old.document.id, &f.assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(archive.markdown(), Fixture::input().markdown);
    assert!(archive.state().document.pinned);
    assert_eq!(archive.state().document.line, 3);
    assert_eq!(
        archive.assets()[0].bytes().unwrap(),
        Fixture::input().assets[0].bytes
    );
    assert_eq!(archive.assets()[0].state, "retired");
    drop(archive);
    drop(ready);
    drop(f.assets);
    drop(f.store);
    let mut reopened = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    let recovered = device::recover_imports(&mut reopened, &f.documents).unwrap();
    assert_eq!(recovered.documents.len(), 1);
    assert_eq!(recovered.documents[0].id, new.id);
    assert!(reopened
        .set_source_enabled(&old.document.source_id, true)
        .is_err());
    let assets = AssetStore::open(&mut reopened, &f.temp.path().join("assets")).unwrap();
    let archived = prepare_archive(
        &mut reopened,
        &old.document.id,
        &assets,
        &AssetRuntime::default(),
    )
    .unwrap()
    .read()
    .unwrap();
    assert_eq!(archived.markdown(), Fixture::input().markdown);
}
#[test]
fn retirement_final_revocation_fence_releases_asset_sessions_and_keeps_exportable_bytes() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let original = commit(&mut f, &ready);
    let doc = &original.document;
    let manifest = f
        .assets
        .manifest(&mut f.store, &doc.id, &doc.revision)
        .unwrap();
    let snapshot = f
        .assets
        .prepare_open(
            &mut f.store,
            &doc.id,
            &doc.revision,
            &original.assets[0].asset_id,
            &manifest.manifest,
            f.runtime.read_permit(Instant::now()).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    let (session, _) = f
        .runtime
        .install(&f.assets, &mut f.store, snapshot, Instant::now())
        .unwrap();
    let current = verified(&mut f, &doc.id);
    let record = deleted(&f);
    let refused = with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            let receipt = retire_current(&tx, &guard, &current, record.clone())?;
            tx.execute("UPDATE sources SET enabled=1 WHERE id=?1", [&doc.source_id])?;
            revalidate_retirement(&tx, &guard, &current, &receipt)?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(refused.is_err());
    assert!(device::read(&mut f.store, &doc.id, None).is_ok());
    with_transition(
        &mut f.store,
        &current,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            let receipt = retire_current(&tx, &guard, &current, record)?;
            revalidate_retirement(&tx, &guard, &current, &receipt)?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    assert!(f
        .runtime
        .chunk(&f.assets, &mut f.store, &session, 0, Instant::now())
        .is_err());
    let a = f.runtime.read_permit(Instant::now()).unwrap();
    let b = f.runtime.read_permit(Instant::now()).unwrap();
    assert!(f.runtime.read_permit(Instant::now()).is_err());
    drop((a, b));
    assert!(device::recover_imports(&mut f.store, &f.documents)
        .unwrap()
        .documents
        .is_empty());
    assert!(f.store.set_source_enabled(&doc.source_id, true).is_err());
    let archive = prepare_archive(&mut f.store, &doc.id, &f.assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(archive.markdown(), Fixture::input().markdown);
    assert!(archive.state().retirement.as_ref().unwrap().kind == RetirementKind::Deleted);
    assert_eq!(
        archive.assets()[0].bytes().unwrap(),
        Fixture::input().assets[0].bytes
    );
    // Unrelated explicit local grants retain the existing enable/read semantics.
    let local = f.temp.path().join("unrelated");
    std::fs::create_dir(&local).unwrap();
    std::fs::write(local.join("local.md"), "# Local").unwrap();
    let source = f
        .store
        .add_local_source("local", crate::SourceRoot::capture(local).unwrap())
        .unwrap();
    f.store.set_source_enabled(&source.id, false).unwrap();
    f.store.set_source_enabled(&source.id, true).unwrap();
    assert_eq!(
        f.store
            .read_document(&source.id, "local.md", None)
            .unwrap()
            .markdown,
        "# Local"
    );
}
