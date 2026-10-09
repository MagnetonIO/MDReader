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
    let mut copied = prepared.candidate.as_mut().unwrap().store().unwrap();
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
            &prepared
                .candidate
                .as_ref()
                .unwrap()
                .path()
                .join("reader-v1/device-assets-v1"),
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
    prepared.control.with_library_connection(|db| {
        marker::record(db, association.0, &association.1).unwrap();
        assert!(matches!(
            marker::record(db, Uuid::new_v4(), &association.1),
            Err(Error::Conflict)
        ));
        assert_eq!(marker::read(db).unwrap().0, association.0);
    });

    let candidate = prepared.candidate.as_mut().unwrap();
    let observed = candidate
        .committed_candidate(association.1.operation)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&observed).unwrap(),
        serde_json::to_vec(&association.1).unwrap()
    );
    assert!(candidate.committed_candidate(Uuid::new_v4()).is_err());
    candidate.verify_candidate(&association.1).unwrap();

    prepared.control.with_library_connection(|db| {
        db.execute_batch("CREATE TEMP TABLE native_windows_selection_v1(singleton INTEGER)").unwrap();
        let changes = db.total_changes();
        assert!(matches!(marker::present(db), Err(Error::Conflict)));
        assert_eq!(db.total_changes(), changes);
        db.execute_batch("DROP TABLE temp.native_windows_selection_v1; CREATE TEMP TABLE native_windows_legacy_v1(singleton INTEGER)").unwrap();
        let changes = db.total_changes();
        assert!(matches!(legacy_record::read(db), Err(Error::Conflict)));
        assert_eq!(db.total_changes(), changes);
        db.execute_batch("DROP TABLE temp.native_windows_legacy_v1").unwrap();
        db.execute_batch("CREATE TEMP TABLE native_windows_controller_v1(singleton INTEGER)").unwrap();
        let changes = db.total_changes();
        assert!(matches!(controller_record::read(db), Err(Error::Conflict)));
        assert_eq!(db.total_changes(), changes);
        db.execute_batch("DROP TABLE temp.native_windows_controller_v1; CREATE TEMP TABLE native_windows_rollback_v1(singleton INTEGER)").unwrap();
        let changes = db.total_changes();
        assert!(matches!(rollback_record::read(db), Err(Error::Conflict)));
        assert_eq!(db.total_changes(), changes);
        db.execute_batch("DROP TABLE temp.native_windows_rollback_v1").unwrap();

    });
    original.revalidate().unwrap();
    drop(assets);
    drop(runtime);
    drop(copied);
    assert_eq!(prepared.state().unwrap(), "prepared");
    let migration = prepared.launch_directory();
    drop(prepared);
    let mut prepared = Prepared::resume(&migration).unwrap();
    assert_eq!(prepared.native_operation(), native);
    assert_eq!(prepared.original_root().unwrap().path, original.path());
    assert_eq!(prepared.state().unwrap(), "prepared");
    prepared.activate(&mut original).unwrap();
    assert_eq!(prepared.state().unwrap(), "launch_requested");
    let selected = selected_for_launch(&prepared.launch_directory()).unwrap();
    assert_eq!(
        selected.path,
        association.1.candidate.locator.restore().unwrap()
    );
    verify_profile(&selected.path).unwrap();
    // Replay reconciles the same committed selector, without a new generation.
    prepared.activate(&mut original).unwrap();
    let generation = prepared
        .control
        .with_library_connection(|db| {
            Journal::attach(db)?
                .selection()
                .map(|selection| selection.generation)
        })
        .unwrap();
    assert_eq!(generation, 1);
    drop(prepared);
    let mut reopened = Prepared::resume(&migration).unwrap();
    assert_eq!(reopened.native_operation(), native);
    assert_eq!(reopened.state().unwrap(), "launch_requested");
    assert_eq!(reopened.imported().unwrap(), 1);
    let copied_path = selected.path.clone();
    let writer_root =
        mdreader_capture::PrivateDirectory::open(&copied_path.join("reader-v1")).unwrap();
    let writer = writer_root.open_lock(Path::new("reader.lock")).unwrap();
    fs2::FileExt::try_lock_exclusive(&writer).unwrap();
    let mut changed = ReaderStore::open_private(&copied_path.join("reader-v1")).unwrap();
    device::pin(&mut changed, &copy.id, false).unwrap();
    device::position(&mut changed, &copy.id, &copy.revision, 2).unwrap();
    let added = device::import(
        &mut changed,
        &copied_path.join("reader-v1/device-markdown-v1"),
        "candidate-notes.md".into(),
        "# Added after import\n".into(),
    )
    .unwrap();
    drop(changed);
    drop(writer);
    drop(writer_root);
    reopened.rollback(&mut original).unwrap();
    assert_eq!(reopened.state().unwrap(), "rolled_back");
    let selected_original = selected_for_launch(&migration).unwrap();
    assert_eq!(selected_original.path, original.path());
    let preserved = reopened
        .control
        .with_library_connection(|db| rollback_record::read(db))
        .unwrap()
        .unwrap();
    let archive = mdreader_profile::verify(
        &migration.join(format!("rollback-{}", preserved.directory)),
        &preserved.snapshot,
        &Limits::default(),
    )
    .unwrap();
    let database = archive
        .manifest()
        .entries
        .iter()
        .find(|entry| entry.relative.restore().unwrap() == Path::new("reader-v1/library.sqlite"))
        .unwrap()
        .member
        .as_ref()
        .unwrap();
    let db = rusqlite::Connection::open_with_flags(
        migration
            .join(format!("rollback-{}", preserved.directory))
            .join(database),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let retained: (bool, u32) = db
        .query_row(
            "SELECT pinned,line FROM device_documents_v1 WHERE id=?1",
            [&copy.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(retained, (false, 2));
    let body: Vec<u8> = db
        .query_row(
            "SELECT body FROM device_windows_bodies_v1 WHERE id=?1",
            [&added.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(body, b"# Added after import\n");
    let retained_png: Vec<u8> = db
        .query_row(
            "SELECT body FROM device_asset_windows_bytes_v1 WHERE id=?1",
            [&image.asset_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained_png, png);
    drop(db);
    drop(reopened);
    let mut returned = Prepared::resume(&migration).unwrap();
    assert_eq!(returned.state().unwrap(), "rolled_back");
    assert_eq!(returned.native_operation(), native);
    drop(returned);
    drop(original);
    for (name, expected) in names.iter().zip(before) {
        assert_eq!(evidence(&original_path.join(name)), expected);
    }
}
