use super::*;
use std::time::Instant;

#[test]
fn canonical_assets_reopen_preserve_exact_png_and_reject_changed_validation() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("private reader");
    let mut owner = ReaderStore::open_private(&directory).unwrap();
    let document = crate::device::import(
        &mut owner,
        &directory.join("documents"),
        "images.md".into(),
        "\u{feff}# Images\r\n\r\n![Exact](diagram.png)\r\n".into(),
    )
    .unwrap();
    crate::device::pin(&mut owner, &document.id, true).unwrap();
    crate::device::position(&mut owner, &document.id, &document.revision, 3).unwrap();
    let asset_directory = directory.join("assets");
    let runtime = AssetRuntime::default();
    let assets = runtime.assets(&mut owner, &asset_directory).unwrap();
    let before = assets
        .manifest(&mut owner, &document.id, &document.revision)
        .unwrap();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 3)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let id = assets
        .begin(
            &mut owner,
            &document.id,
            &document.revision,
            "diagram.png",
            bytes.len() as u32,
            &before.manifest,
        )
        .unwrap();
    assert_eq!(
        assets
            .write(&mut owner, &id, 0, &STANDARD.encode(&bytes[..3]))
            .unwrap(),
        3
    );
    assert_eq!(
        assets
            .write(&mut owner, &id, 0, &STANDARD.encode(&bytes[..3]))
            .unwrap(),
        3
    );
    assert!(assets
        .write(&mut owner, &id, 0, &STANDARD.encode([0, 0, 0]))
        .is_err());
    assert!(assets
        .manifest(&mut owner, &document.id, &document.revision)
        .unwrap()
        .assets
        .is_empty());
    // There is no directory-fsync/file-rename receipt: partial bytes and offset
    // are the same committed private library transaction.
    assert_eq!(fs::read_dir(&asset_directory).unwrap().count(), 0);
    drop(assets);
    drop(runtime);
    drop(owner);
    let mut owner = ReaderStore::open_private(&directory).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime
        .existing_assets(&mut owner, &asset_directory)
        .unwrap()
        .unwrap();
    assert_eq!(
        assets
            .write(&mut owner, &id, 3, &STANDARD.encode(&bytes[3..]))
            .unwrap(),
        bytes.len() as u32
    );
    let validated = assets
        .prepare_commit(&mut owner, &id, runtime.validation_permit().unwrap())
        .unwrap()
        .validate()
        .unwrap();
    let mut replaced = bytes.clone();
    replaced[0] ^= 1;
    owner
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_windows_bytes_v1 SET body=?2 WHERE id=?1",
                params![id, replaced],
            )
        })
        .unwrap();
    assert!(assets.publish(&mut owner, validated).is_err());
    assert!(assets
        .manifest(&mut owner, &document.id, &document.revision)
        .unwrap()
        .assets
        .is_empty());
    owner
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_windows_bytes_v1 SET body=?2 WHERE id=?1",
                params![id, bytes],
            )
        })
        .unwrap();
    let validated = assets
        .prepare_commit(&mut owner, &id, runtime.validation_permit().unwrap())
        .unwrap()
        .validate()
        .unwrap();
    let (receipt, stamp) = assets.publish(&mut owner, validated).unwrap();
    assert_eq!((receipt.width, receipt.height), (2, 3));
    drop(assets);
    drop(runtime);
    drop(owner);
    let mut reader = ReaderStore::open_durable_readonly(&directory).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime
        .existing_assets(&mut reader, &asset_directory)
        .unwrap()
        .unwrap();
    let snapshot = assets
        .prepare_open(
            &mut reader,
            &document.id,
            &document.revision,
            &id,
            &stamp,
            runtime.read_permit(Instant::now()).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    let (session, _) = runtime
        .install(&assets, &mut reader, snapshot, Instant::now())
        .unwrap();
    let chunk = runtime
        .chunk(&assets, &mut reader, &session, 0, Instant::now())
        .unwrap();
    assert!(chunk.complete);
    assert_eq!(STANDARD.decode(chunk.base64).unwrap(), bytes);
    let state = crate::device::catalog::document(&reader, &document.id).unwrap();
    assert!(state.pinned);
    assert_eq!(state.line, 3);
    drop(reader);
    let mut owner = ReaderStore::open_private(&directory).unwrap();
    owner.with_library_connection(|db| db.execute_batch("CREATE TRIGGER substituted_asset AFTER UPDATE ON device_asset_items_v1 BEGIN UPDATE device_asset_windows_bytes_v1 SET body=zeroblob(length(body)); END;")).unwrap();
    let before = owner.device_connection().total_changes();
    assert!(AssetRuntime::default()
        .existing_assets(&mut owner, &asset_directory)
        .is_err());
    assert_eq!(owner.device_connection().total_changes(), before);
}
