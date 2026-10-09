use super::*;
use crate::profile_transition::persistence::JsonOwner;
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::device::{self, assets::AssetRuntime};
use std::{fs::File, io::Read, time::Instant};
fn evidence(path: &Path) -> (mdreader_capture::WindowsObjectStamp, Vec<u8>) {
    let mut file = File::open(path).unwrap();
    let stamp = mdreader_capture::observe_object(&file, false, true).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    (stamp, bytes)
}
#[test]
fn selected_windows_prepare_keeps_original_bytes_png_state_and_private_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    let original_path = parent.join("original");
    mdreader_capture::PrivateDirectory::create(&original_path).unwrap();
    let json = JsonOwner::new(SourceRoot::capture(original_path.clone()).unwrap()).unwrap();
    let mut store = ReaderStore::open_private(&original_path.join("reader-v1")).unwrap();
    let markdown = "\u{feff}# Original\r\n\r\n![Preserved](diagram.png)\r\n";
    let document = device::import(
        &mut store,
        &original_path.join("reader-v1/device-markdown-v1"),
        "original.md".into(),
        markdown.into(),
    )
    .unwrap();
    device::pin(&mut store, &document.id, true).unwrap();
    device::position(&mut store, &document.id, &document.revision, 3).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime
        .assets(
            &mut store,
            &original_path.join("reader-v1/device-assets-v1"),
        )
        .unwrap();
    let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAD0lEQVR4nGNgYPgPRsgUADjcBftBwbf8AAAAAElFTkSuQmCC").unwrap();
    let manifest = assets
        .manifest(&mut store, &document.id, &document.revision)
        .unwrap();
    let id = assets
        .begin(
            &mut store,
            &document.id,
            &document.revision,
            "diagram.png",
            png.len() as u32,
            &manifest.manifest,
        )
        .unwrap();
    assets
        .write(&mut store, &id, 0, &STANDARD.encode(&png))
        .unwrap();
    let prepared = assets
        .prepare_commit(&mut store, &id, runtime.validation_permit().unwrap())
        .unwrap()
        .validate()
        .unwrap();
    assets.publish(&mut store, prepared).unwrap();
    drop(assets);
    drop(runtime);
    drop(store);
    drop(json);
    let names = [
        "reader-v1/library.sqlite",
        "reader-v1/library.sqlite-journal",
        "reader-json-v1.sqlite",
        "reader-json-v1.sqlite-journal",
    ];
    let before: Vec<_> = names
        .iter()
        .map(|name| evidence(&original_path.join(name)))
        .collect();
    let mut original = ClosedOwner::acquire(&original_path).unwrap();
    let native = Uuid::new_v4();
    let mut prepared = Prepared::prepare(
        &mut original,
        &parent,
        &[Selection {
            document_id: document.id.clone(),
            revision: document.revision.clone(),
        }],
        native,
    )
    .unwrap();
    assert_eq!(prepared.imported().unwrap(), 1);
    let mut copied = prepared.candidate.store().unwrap();
    let copies = super::super::conversion::documents(&copied).unwrap();
    assert_eq!(copies.len(), 1);
    let copy = &copies[0];
    assert_ne!(copy.id, document.id);
    assert_ne!(copy.source_id, document.source_id);
    assert_eq!(copy.revision, document.revision);
    assert!(copy.pinned);
    assert_eq!(copy.line, 3);
    assert_eq!(
        device::catalog::read(&copied, &device::catalog::receipt(copy))
            .unwrap()
            .markdown,
        markdown
    );
    let runtime = AssetRuntime::default();
    let assets = runtime
        .existing_assets(
            &mut copied,
            &prepared.candidate.path().join("reader-v1/device-assets-v1"),
        )
        .unwrap()
        .unwrap();
    let manifest = assets
        .manifest(&mut copied, &copy.id, &copy.revision)
        .unwrap();
    assert_eq!(manifest.assets.len(), 1);
    assert!(manifest.pending.is_empty());
    let image = &manifest.assets[0];
    let snapshot = assets
        .prepare_open(
            &mut copied,
            &copy.id,
            &copy.revision,
            &image.asset_id,
            &manifest.manifest,
            runtime.read_permit(Instant::now()).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    let (session, _) = runtime
        .install(&assets, &mut copied, snapshot, Instant::now())
        .unwrap();
    let chunk = runtime
        .chunk(&assets, &mut copied, &session, 0, Instant::now())
        .unwrap();
    assert!(chunk.complete);
    assert_eq!(STANDARD.decode(chunk.base64).unwrap(), png);
    let association = prepared
        .control
        .with_library_connection(|db| marker::read(db))
        .unwrap();
    assert_eq!(association.0, native);
    assert_eq!(association.1.imported_records, 1);
    original.revalidate().unwrap();
    drop(copied);
    drop(prepared);
    drop(original);
    for (name, expected) in names.iter().zip(before) {
        assert_eq!(evidence(&original_path.join(name)), expected);
    }
}
