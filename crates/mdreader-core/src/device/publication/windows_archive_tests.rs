//! Ordinary upload originals, not incoming projections, exercise archival/edit
//! preservation through real private NTFS roots and the actual owner SQLite DB.
use super::*;
use crate::device::{assets::AssetStore, catalog, state};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::params;
use std::path::PathBuf;

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    documents: PathBuf,
    store: ReaderStore,
    runtime: AssetRuntime,
    assets: AssetStore,
    document: device::DeviceDocument,
    png: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("private reader 日本語 % #");
        let documents = root.join("documents");
        let mut store = ReaderStore::open_private(&root).unwrap();
        let document = device::import(
            &mut store,
            &documents,
            "Exact.md".into(),
            "\u{feff}# Before\r\n![Ready](figure.png)\r\n![Partial](partial.png)\r\n".into(),
        )
        .unwrap();
        device::pin(&mut store, &document.id, true).unwrap();
        device::position(&mut store, &document.id, &document.revision, 3).unwrap();
        let runtime = AssetRuntime::default();
        let assets = runtime.assets(&mut store, &root.join("assets")).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        Self {
            temp,
            root,
            documents,
            store,
            runtime,
            assets,
            document,
            png: png.into_inner(),
        }
    }
    fn reserve(&mut self, target: &str) -> String {
        let manifest = self
            .assets
            .manifest(&mut self.store, &self.document.id, &self.document.revision)
            .unwrap();
        self.assets
            .begin(
                &mut self.store,
                &self.document.id,
                &self.document.revision,
                target,
                self.png.len() as u32,
                &manifest.manifest,
            )
            .unwrap()
    }
    fn upload(&mut self) -> String {
        let id = self.reserve("figure.png");
        self.assets
            .write(&mut self.store, &id, 0, &STANDARD.encode(&self.png))
            .unwrap();
        let prepared = self
            .assets
            .prepare_commit(
                &mut self.store,
                &id,
                self.runtime.validation_permit().unwrap(),
            )
            .unwrap();
        self.assets
            .publish(&mut self.store, prepared.validate().unwrap())
            .unwrap();
        assert!(!self.root.join("assets").join(format!("{id}.bin")).exists());
        id
    }
    fn archive(&mut self) -> ArchivedDocument {
        prepare_archive(
            &mut self.store,
            &self.document.id,
            &self.assets,
            &self.runtime,
        )
        .unwrap()
        .read()
        .unwrap()
    }
}
#[test]
fn ordinary_upload_archive_edits_and_reopens_exact_original_and_image() {
    let mut f = Fixture::new();
    f.upload();
    let archive = f.archive();
    archive.revalidate(&mut f.store).unwrap();
    assert_eq!(archive.assets()[0].bytes(), Some(f.png.as_slice()));
    // Export bytes are the exact canonical upload even though no .bin exists.
    let export = f.temp.path().join("explicit image export.png");
    std::fs::write(&export, archive.assets()[0].bytes().unwrap()).unwrap();
    assert_eq!(std::fs::read(&export).unwrap(), f.png);
    let edited = b"# Edited\n![Ready](figure.png)\n".to_vec();
    let selection =
        EditSelection::new(catalog::receipt(&f.document), uuid::Uuid::new_v4(), &edited).unwrap();
    let origin = selection.origin();
    let current = prepare_current(&mut f.store, &f.document.id, &f.assets, &origin)
        .unwrap()
        .verify()
        .unwrap();
    let input = archive.edited_input(edited.clone()).unwrap();
    let ready = reserve_new(&mut f.store, &f.documents, &f.assets, &origin, &input)
        .unwrap()
        .write(input)
        .unwrap()
        .validate(f.runtime.validation_permit().unwrap())
        .unwrap();
    let saved = commit_edit(&mut f.store, &ready, &current, &archive, &selection).unwrap();
    assert!(saved.pinned);
    assert_ne!(saved.id, f.document.id);
    drop((ready, current, archive));
    drop(f.assets);
    drop(f.runtime);
    drop(f.store);
    let mut store = ReaderStore::open_private(&f.root).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime
        .existing_assets(&mut store, &f.root.join("assets"))
        .unwrap()
        .unwrap();
    assert_eq!(replay_edit(&mut store, &selection).unwrap().unwrap(), saved);
    for id in [&f.document.id, &saved.id] {
        let archive = prepare_archive(&mut store, id, &assets, &runtime)
            .unwrap()
            .read()
            .unwrap();
        archive.revalidate(&mut store).unwrap();
        assert_eq!(archive.assets()[0].bytes(), Some(f.png.as_slice()));
        if id == &saved.id {
            assert_eq!(archive.markdown(), edited);
        } else {
            assert!(archive.state().retirement.is_some());
        }
    }
    assert!(state::current(&mut store, &f.document.id).is_err());
}
#[test]
fn reserved_and_retired_prefixes_export_without_regrant_or_body_inference() {
    let mut f = Fixture::new();
    let retired = f.upload();
    let ready = f.upload(); // Retains the preceding exact ready upload as retired.
    let partial = f.reserve("partial.png");
    f.assets
        .write(&mut f.store, &partial, 0, &STANDARD.encode(&f.png[..7]))
        .unwrap();
    let archive = f.archive();
    archive.revalidate(&mut f.store).unwrap();
    let find = |id: &str| archive.assets().iter().find(|a| a.asset_id == id).unwrap();
    assert_eq!(find(&retired).state, "retired");
    assert_eq!(find(&ready).state, "ready");
    assert_eq!(find(&partial).state, "reserved");
    assert_eq!(find(&partial).bytes(), Some(&f.png[..7]));
    assert!(archive.edited_input(b"# Edit".to_vec()).is_err());
    drop(archive);
    let origin = origin();
    let current = prepare_current(&mut f.store, &f.document.id, &f.assets, &origin)
        .unwrap()
        .verify()
        .unwrap();
    with_transition(
        &mut f.store,
        &current,
        &origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            let receipt = retire_current(
                &tx,
                &guard,
                &current,
                state::RetirementRecord {
                    kind: state::RetirementKind::Deleted,
                    operation_id: origin.wire_revision,
                    replacement: None,
                },
            )?;
            revalidate_retirement(&tx, &guard, &current, &receipt)?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    drop(current);
    let archive = f.archive();
    archive.revalidate(&mut f.store).unwrap();
    assert!(archive.assets().iter().all(|a| a.state == "retired"));
    assert_eq!(
        archive
            .assets()
            .iter()
            .find(|a| a.asset_id == partial)
            .unwrap()
            .bytes(),
        Some(&f.png[..7])
    );
    drop(archive);
    // Absence of canonical original bytes cannot be repaired from a file path.
    f.store
        .with_library_connection(|db| {
            db.execute(
                "DELETE FROM device_asset_windows_bytes_v1 WHERE id=?1",
                [&partial],
            )
        })
        .unwrap();
    assert!(prepare_archive(&mut f.store, &f.document.id, &f.assets, &f.runtime).is_err());
    let enabled: bool = f
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT enabled FROM sources WHERE id=?1",
                [&f.document.source_id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(!enabled);
}
#[test]
fn canonical_body_callback_changes_abort_after_provisional_retirement() {
    let mut f = Fixture::new();
    let id = f.upload();
    let archive = f.archive();
    let edited = b"# Edited\n![Ready](figure.png)\n".to_vec();
    let selection =
        EditSelection::new(catalog::receipt(&f.document), uuid::Uuid::new_v4(), &edited).unwrap();
    let origin = selection.origin();
    let current = prepare_current(&mut f.store, &f.document.id, &f.assets, &origin)
        .unwrap()
        .verify()
        .unwrap();
    let input = archive.edited_input(edited).unwrap();
    let ready = reserve_new(&mut f.store, &f.documents, &f.assets, &origin, &input)
        .unwrap()
        .write(input)
        .unwrap()
        .validate(f.runtime.validation_permit().unwrap())
        .unwrap();
    let result = with_replacement(
        &mut f.store,
        &ready,
        &current,
        &origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            revalidate_current_archive(&tx, &guard.old, &current, &archive)?;
            let replaced = publish_replacement(&tx, &guard, &ready, &current)?;
            tx.execute(
                "UPDATE device_asset_windows_bytes_v1 SET body=zeroblob(length(body)) WHERE id=?1",
                [&id],
            )?;
            // Equal sizes and unchanged asset metadata cannot certify changed bytes.
            revalidate_replacement(&tx, &guard, &ready, &current, &replaced)?;
            archive.verify_retired_assets_in(&tx)?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(result.is_err());
    assert!(state::current(&mut f.store, &f.document.id).is_ok());
    archive.revalidate(&mut f.store).unwrap();
    let count: u64 = f
        .store
        .with_library_connection(|db| {
            db.query_row("SELECT count(*) FROM device_documents_v1", [], |r| r.get(0))
        })
        .unwrap();
    assert_eq!(count, 1);
    let bytes: Vec<u8> = f
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT body FROM device_asset_windows_bytes_v1 WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(bytes, f.png);
    // An old owner's snapshot cannot validate under a reopened owner session.
    drop((current, ready));
    drop(f.assets);
    drop(f.runtime);
    drop(f.store);
    let mut reopened = ReaderStore::open_private(&f.root).unwrap();
    assert!(archive.revalidate(&mut reopened).is_err());
}
