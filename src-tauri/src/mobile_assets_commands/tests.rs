use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::fs;

#[tokio::test]
async fn close_releases_snapshot_after_asset_directory_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("assets");
    let documents = temp.path().join("documents");
    let backend = ReaderBackend::embedded(&temp.path().join("data")).unwrap();
    let runtime = AssetRuntime::default();
    // Public synthetic 2x3 blue PNG; no model, provider or installed profile.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAD0lEQVR4nGNgYPgPRsgUADjcBftBwbf8AAAAAElFTkSuQmCC";
    let bytes = STANDARD.decode(png).unwrap();
    let (document, prepared) = backend
        .with_mobile_store(|store| {
            let document = mdreader_core::device::import(
                store,
                &documents,
                "image.md".into(),
                "# Image\n\n![Blue](blue.png)\n".into(),
            )
            .map_err(|error| error.to_string())?;
            let assets = runtime
                .assets(store, &directory)
                .map_err(|error| error.to_string())?;
            let prepared = (|| -> assets::Result<_> {
                let manifest = assets.manifest(store, &document.id, &document.revision)?;
                let intent = assets.begin(
                    store,
                    &document.id,
                    &document.revision,
                    "blue.png",
                    bytes.len() as u32,
                    &manifest.manifest,
                )?;
                assets.write(store, &intent, 0, png)?;
                assets.prepare_commit(store, &intent, runtime.validation_permit()?)
            })()
            .map_err(|error| error.to_string())?;
            Ok((document, prepared))
        })
        .unwrap();
    // Decoder and hashing work stays outside the embedded owner's mutex.
    let validated = prepared.validate().unwrap();
    let (receipt, manifest) = backend
        .with_mobile_store(|store| {
            runtime
                .assets(store, &directory)
                .and_then(|assets| assets.publish(store, validated))
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let opened = dispatch(
        backend.clone(),
        runtime.clone(),
        directory.clone(),
        AssetRequest::Open {
            document_id: document.id,
            revision: document.revision,
            asset_id: receipt.asset_id.clone(),
            manifest,
        },
    )
    .await
    .unwrap();
    let AssetReply::Opened {
        session_id,
        receipt: wire,
    } = opened
    else {
        panic!("expected an opened read session");
    };
    assert_eq!(wire.content_hash, receipt.content_hash);
    assert_eq!((wire.width, wire.height), (2, 3));
    let second = runtime.read_permit(Instant::now()).unwrap();
    assert!(runtime.read_permit(Instant::now()).is_err());
    drop(second);

    let relocated = temp.path().join("old-assets");
    fs::rename(&directory, &relocated).unwrap();
    fs::create_dir(&directory).unwrap();
    // Closing a retained snapshot narrows authority and must not reopen storage.
    let reply = dispatch(
        backend,
        runtime.clone(),
        directory,
        AssetRequest::Close { session_id },
    )
    .await
    .unwrap();
    assert!(matches!(reply, AssetReply::Complete));
    let first = runtime.read_permit(Instant::now()).unwrap();
    let second = runtime.read_permit(Instant::now()).unwrap();
    assert!(runtime.read_permit(Instant::now()).is_err());
    drop((first, second));
    assert_eq!(
        fs::read(relocated.join(format!("{}.bin", receipt.asset_id))).unwrap(),
        bytes
    );
}
