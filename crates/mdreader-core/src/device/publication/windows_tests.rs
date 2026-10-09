//! Real private NTFS projection plus SQLite commit/reopen boundaries.
use super::*;
use crate::{device, device::assets::AssetRuntime, ReaderStore};
fn origin() -> IncomingOrigin {
    IncomingOrigin {
        scope_digest: [17; 32],
        item_id: uuid::Uuid::new_v4(),
        wire_revision: uuid::Uuid::new_v4(),
        session_epoch: uuid::Uuid::new_v4(),
        selection_epoch: uuid::Uuid::new_v4(),
    }
}
fn input(title: &str) -> IncomingDocument {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 3)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    IncomingDocument::new(
        "日本語 % # ?.md".into(),
        format!("\u{feff}# {title}\r\n\r\n![Exact](figure.png)\r\n").into_bytes(),
        vec![
            IncomingAsset::new("figure.png".into(), "image/png".into(), png.into_inner()).unwrap(),
        ],
    )
    .unwrap()
}
fn publish(
    store: &mut ReaderStore,
    ready: &ReadyIncoming,
    origin: &IncomingOrigin,
) -> PublicationReceipt {
    with_publication(store, ready, origin, |db, guard| -> PublicationResult<_> {
        let tx = db.transaction()?;
        let receipt = publish_new(&tx, &guard, ready)?;
        revalidate_publication(&tx, &guard, ready)?;
        tx.commit()?;
        Ok(receipt)
    })
    .unwrap()
    .unwrap()
}
#[test]
fn canonical_incoming_reopens_projects_exact_images_and_preserves_outer_atomicity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("private reader 日本語");
    let documents = root.join("documents");
    let asset_path = root.join("assets");
    let mut store = ReaderStore::open_private(&root).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime.assets(&mut store, &asset_path).unwrap();
    let origin = origin();
    let expected = input("Before");
    let plan = reserve_new(&mut store, &documents, &assets, &origin, &expected).unwrap();
    let id = plan.document_id().to_owned();
    drop(plan);
    drop(assets);
    drop(runtime);
    drop(store);
    let mut store = ReaderStore::open_private(&root).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime
        .existing_assets(&mut store, &asset_path)
        .unwrap()
        .unwrap();
    let ready = resume(&mut store, &documents, &assets, &id, &origin)
        .unwrap()
        .write(input("Before"))
        .unwrap()
        .validate(runtime.validation_permit().unwrap())
        .unwrap();
    let failed = with_publication(
        &mut store,
        &ready,
        &origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            publish_new(&tx, &guard, &ready)?;
            Err(PublicationError::Conflict)
        },
    )
    .unwrap();
    assert!(failed.is_err());
    let count: u64 = store
        .with_library_connection(|db| {
            db.query_row("SELECT count(*) FROM device_documents_v1", [], |r| r.get(0))
        })
        .unwrap();
    assert_eq!(count, 0);
    let receipt = publish(&mut store, &ready, &origin);
    // Finish this admitted validation before acquiring the replacement permit.
    drop(ready);
    assert_eq!(receipt.assets.len(), 1);
    assert_eq!(
        device::read(&mut store, &id, Some(&expected.revision))
            .unwrap()
            .markdown
            .as_bytes(),
        expected.markdown
    );
    device::pin(&mut store, &id, true).unwrap();
    device::position(&mut store, &id, &expected.revision, 3).unwrap();
    let mut replacement_origin = origin.clone();
    replacement_origin.wire_revision = uuid::Uuid::new_v4();
    let next = reserve_new(
        &mut store,
        &documents,
        &assets,
        &replacement_origin,
        &input("After"),
    )
    .unwrap()
    .write(input("After"))
    .unwrap()
    .validate(runtime.validation_permit().unwrap())
    .unwrap();
    // Current and incoming guards must bind the same current selection origin.
    let current = prepare_current(&mut store, &id, &assets, &replacement_origin)
        .unwrap()
        .verify()
        .unwrap();
    let replaced = with_replacement(
        &mut store,
        &next,
        &current,
        &replacement_origin,
        |db, guard| -> PublicationResult<_> {
            let tx = db.transaction()?;
            let receipt = publish_replacement(&tx, &guard, &next, &current)?;
            revalidate_replacement(&tx, &guard, &next, &current, &receipt)?;
            tx.commit()?;
            Ok(receipt)
        },
    )
    .unwrap()
    .unwrap();
    assert!(replaced.publication().document.pinned);
    assert_eq!(replaced.publication().document.line, 1);
    assert!(device::read(&mut store, &id, Some(&expected.revision)).is_err());
    drop(current);
    drop(next);
    drop(assets);
    drop(runtime);
    drop(store);
    let mut store = ReaderStore::open_private(&root).unwrap();
    assert_eq!(
        device::read(
            &mut store,
            &replaced.publication().document.id,
            Some(&input("After").revision)
        )
        .unwrap()
        .markdown
        .as_bytes(),
        input("After").markdown
    );
}
#[test]
fn explicit_drop_retires_canonical_bytes_and_only_known_private_projections() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("private reader");
    let documents = root.join("documents");
    let assets_path = root.join("assets");
    let mut store = ReaderStore::open_private(&root).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime.assets(&mut store, &assets_path).unwrap();
    // Built-in catalog triggers are admitted by exact definition; additional
    // main and TEMP producers cannot join that authority, even under a known name.
    for (create, drop) in [
        ("CREATE TRIGGER foreign_source AFTER INSERT ON sources BEGIN SELECT 1; END;",
         "DROP TRIGGER foreign_source;"),
        ("CREATE TEMP TRIGGER reader_catalog_insert_v1 AFTER INSERT ON sources BEGIN SELECT 1; END;",
         "DROP TRIGGER temp.reader_catalog_insert_v1;"),
    ] {
        store.with_library_connection(|db| db.execute_batch(create)).unwrap();
        let before = store.with_library_connection(|db| db.total_changes());
        assert!(AssetRuntime::default().existing_assets(&mut store, &assets_path).is_err());
        assert_eq!(store.with_library_connection(|db| db.total_changes()), before);
        store.with_library_connection(|db| db.execute_batch(drop)).unwrap();
    }
    let origin = origin();
    let plan = reserve_new(&mut store, &documents, &assets, &origin, &input("Drop")).unwrap();
    let id = plan.document_id().to_owned();
    let staged = plan.write(input("Drop")).unwrap();
    drop(staged);
    let private = mdreader_capture::PrivateDirectory::open(&documents.join(&id)).unwrap();
    let mut unknown = private
        .projection_file(std::path::Path::new("keep.md"), true)
        .unwrap()
        .unwrap();
    std::io::Write::write_all(&mut unknown, b"Keep").unwrap();
    unknown.sync_all().unwrap();
    drop(unknown);
    drop(private);
    let drop = prepare_drop(&mut store, &documents, &assets, &id, &origin).unwrap();
    let removed = drop.remove_files().unwrap();
    finish_drop(&mut store, removed).unwrap();
    assert!(!documents.join(&id).join("document.md").exists());
    assert_eq!(
        std::fs::read(documents.join(&id).join("keep.md")).unwrap(),
        b"Keep"
    );
    let bodies: u64 = store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT count(*) FROM device_incoming_windows_input_v1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(bodies, 0);
    assert!(resume(&mut store, &documents, &assets, &id, &origin).is_err());
}
