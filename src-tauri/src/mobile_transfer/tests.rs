use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fs, io::Read};

#[test]
fn transfer_copy_preserves_original_bytes_selected_image_and_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile");
    let backend = crate::reader::ReaderBackend::embedded(&temp.path().join("owner")).unwrap();
    let runtime = crate::mobile_assets::AssetRuntime::default();
    let original =
        "\u{feff}# Original\r\n\r\n![Selected](images/blue.png)\r\n![Unselected](absent.jpg)\r\n";
    let document = backend
        .with_mobile_store(|store| {
            mdreader_core::device::import(
                store,
                &profile.join("device-markdown-v1"),
                "original.md".into(),
                original.into(),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    let reference = Reference {
        document_id: document.id.clone(),
        source_id: document.source_id.clone(),
        path: "document.md".into(),
        revision: document.revision.clone(),
    };
    let cache = temp.path().join("cache");
    let raw = package::prepare(&backend, &runtime, &profile, &cache, &reference).unwrap();
    assert_eq!(fs::read(&raw.path).unwrap(), original.as_bytes());
    assert_eq!(raw.mime, "text/markdown");
    assert!(
        !profile.join("device-assets-v1").exists(),
        "export must not initialize asset metadata or files"
    );

    let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAD0lEQVR4nGNgYPgPRsgUADjcBftBwbf8AAAAAElFTkSuQmCC";
    let bytes = STANDARD.decode(png).unwrap();
    let prepared = backend
        .with_mobile_store(|store| {
            let assets = runtime
                .assets(store, &profile.join("device-assets-v1"))
                .map_err(|e| e.to_string())?;
            let manifest = assets
                .manifest(store, &document.id, &document.revision)
                .map_err(|e| e.to_string())?;
            let id = assets
                .begin(
                    store,
                    &document.id,
                    &document.revision,
                    "images/blue.png",
                    bytes.len() as u32,
                    &manifest.manifest,
                )
                .map_err(|e| e.to_string())?;
            assets
                .write(store, &id, 0, png)
                .map_err(|e| e.to_string())?;
            assets
                .prepare_commit(store, &id, runtime.validation_permit().unwrap())
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let validated = prepared.validate().unwrap();
    backend
        .with_mobile_store(|store| {
            runtime
                .assets(store, &profile.join("device-assets-v1"))
                .and_then(|a| a.publish(store, validated))
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let bundle = package::prepare(&backend, &runtime, &profile, &cache, &reference).unwrap();
    assert_eq!(bundle.mime, "application/zip");
    let mut archive = zip::ZipArchive::new(fs::File::open(bundle.path).unwrap()).unwrap();
    assert_eq!(
        archive.len(),
        2,
        "only the explicit ready image is included"
    );
    let mut body = Vec::new();
    archive
        .by_name("original.md")
        .unwrap()
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, original.as_bytes());
    body.clear();
    archive
        .by_name("images/blue.png")
        .unwrap()
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, bytes);
    let selected = snapshot::prepare(
        &backend,
        &runtime,
        &profile.join("device-markdown-v1"),
        &profile.join("device-assets-v1"),
        &reference,
    )
    .unwrap();
    backend
        .with_mobile_store(|store| {
            store
                .set_source_enabled(&reference.source_id, false)
                .map_err(|e| e.to_string())
        })
        .unwrap();
    assert!(snapshot::revalidate(
        &backend,
        &runtime,
        &profile.join("device-markdown-v1"),
        &profile.join("device-assets-v1"),
        &reference,
        &selected
    )
    .is_err());
    assert!(package::prepare(&backend, &runtime, &profile, &cache, &reference).is_err());
    assert_eq!(
        fs::read(
            profile
                .join("device-markdown-v1")
                .join(&document.id)
                .join("document.md")
        )
        .unwrap(),
        original.as_bytes()
    );
}
