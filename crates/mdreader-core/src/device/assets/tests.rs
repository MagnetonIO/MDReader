#[test]
fn relative_targets_are_exact_and_never_escape() {
    assert_eq!(
        super::canonical_target("./figures/plot%20one.png").unwrap(),
        "figures/plot one.png"
    );
    for target in [
        "../x.png",
        "%2e%2e/x.png",
        "https://x/a.png",
        "/x.png",
        "x\\a.png",
        "x.png?token=x",
        "x.png#frag",
        "a%00.png",
        "a%252fb.png",
    ] {
        assert!(super::canonical_target(target).is_err(), "{target}");
    }
}
use super::*;
struct Fixture {
    temp: tempfile::TempDir,
    root: std::path::PathBuf,
    store: ReaderStore,
    assets: AssetStore,
    id: String,
    source: String,
    revision: String,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("documents");
        let markdown = "# Assets\n\n![First](figures/a.png) ![Second](b.jpg) ![Third](c.png) ![Fourth](d.png) ![Fifth](e.png)\n";
        let mut store = ReaderStore::open(&temp.path().join("data")).unwrap();
        let document =
            crate::device::import(&mut store, &directory, "assets.md".into(), markdown.into())
                .unwrap();
        let root = directory.join(&document.id);
        let id = document.id;
        let source = document.source_id;
        let revision = document.revision;
        let assets = AssetStore::open(&mut store, &temp.path().join("assets")).unwrap();
        Self {
            temp,
            root,
            store,
            assets,
            id,
            source,
            revision,
        }
    }
    fn manifest(&mut self) -> AssetManifest {
        self.assets
            .manifest(&mut self.store, &self.id, &self.revision)
            .unwrap()
    }
    fn png() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }
    fn upload(&mut self, target: &str) -> String {
        let current = self.manifest();
        let png = Self::png();
        let id = self
            .assets
            .begin(
                &mut self.store,
                &self.id,
                &self.revision,
                target,
                png.len() as u32,
                &current.manifest,
            )
            .unwrap();
        self.assets
            .write(&mut self.store, &id, 0, &STANDARD.encode(png))
            .unwrap();
        id
    }
    fn commit(&mut self, id: &str) -> (AssetReceipt, String) {
        let ready = self
            .assets
            .prepare_commit(
                &mut self.store,
                id,
                AssetRuntime::default().validation_permit().unwrap(),
            )
            .unwrap()
            .validate()
            .unwrap();
        self.assets.publish(&mut self.store, ready).unwrap()
    }
}
#[test]
fn manifest_freshness_never_reuses_an_older_empty_selection() {
    let mut f = Fixture::new();
    let pending = f.upload("figures/a.png");
    let other = f.upload("b.jpg");
    let (receipt, stamp) = f.commit(&other);
    f.assets
        .remove(&mut f.store, &f.id, &f.revision, &receipt.asset_id, &stamp)
        .unwrap();
    let prepared = f
        .assets
        .prepare_commit(
            &mut f.store,
            &pending,
            AssetRuntime::default().validation_permit().unwrap(),
        )
        .unwrap()
        .validate()
        .unwrap();
    assert!(
        f.assets.publish(&mut f.store, prepared).is_err(),
        "intervening publication/removal must fence a preexisting upload"
    );
}
#[test]
fn exact_selected_png_and_jpeg_publish_without_changing_original_revision() {
    let mut f = Fixture::new();
    let before = document(&mut f.store, &f.id, &f.revision).unwrap();
    let png = f.upload("figures/a.png");
    let (png_receipt, first) = f.commit(&png);
    assert_eq!(
        (
            png_receipt.mime.as_str(),
            png_receipt.width,
            png_receipt.height
        ),
        ("image/png", 2, 3)
    );
    assert_eq!(png_receipt.content_hash, digest(&Fixture::png()));
    assert_eq!(png_receipt.document_revision, f.revision);
    let mut jpeg = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(3, 2)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let jpeg = jpeg.into_inner();
    let pending = f
        .assets
        .begin(
            &mut f.store,
            &f.id,
            &f.revision,
            "b.jpg",
            jpeg.len() as u32,
            &first,
        )
        .unwrap();
    f.assets
        .write(&mut f.store, &pending, 0, &STANDARD.encode(jpeg))
        .unwrap();
    let (receipt, second) = f.commit(&pending);
    assert_eq!(
        (receipt.mime.as_str(), receipt.width, receipt.height),
        ("image/jpeg", 3, 2)
    );
    assert_ne!(first, second);
    assert_eq!(document(&mut f.store, &f.id, &f.revision).unwrap(), before);
    assert!(f
        .assets
        .begin(
            &mut f.store,
            &f.id,
            &f.revision,
            "not-selected.png",
            1,
            &second
        )
        .is_err());
    assert!(f
        .assets
        .begin(
            &mut f.store,
            &f.id,
            &f.revision,
            "../figures/a.png",
            1,
            &second
        )
        .is_err());
}
#[test]
fn interrupted_append_replays_exactly_after_reopen_and_stays_invisible_until_publication() {
    let mut f = Fixture::new();
    let stamp = f.manifest().manifest;
    let bytes = Fixture::png();
    let id = f
        .assets
        .begin(
            &mut f.store,
            &f.id,
            &f.revision,
            "figures/a.png",
            bytes.len() as u32,
            &stamp,
        )
        .unwrap();
    let item = f.assets.item(&mut f.store, &id).unwrap();
    let mut file = f
        .assets
        .root
        .open_append_relative(&item.filename())
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    // Simulate the durable append / SQLite offset-update crash boundary.
    let Fixture {
        temp,
        store,
        assets,
        id: doc,
        revision,
        ..
    } = f;
    drop(store);
    drop(assets);
    let mut store = ReaderStore::open(&temp.path().join("data")).unwrap();
    let assets = AssetStore::open(&mut store, &temp.path().join("assets")).unwrap();
    let manifest = assets.manifest(&mut store, &doc, &revision).unwrap();
    assert!(manifest.assets.is_empty());
    assert_eq!(manifest.pending[0].received, 0);
    let offset = assets
        .write(&mut store, &id, 0, &STANDARD.encode(&bytes))
        .unwrap();
    assert_eq!(offset, bytes.len() as u32);
    assert_eq!(
        assets
            .write(&mut store, &id, 0, &STANDARD.encode(&bytes))
            .unwrap(),
        offset
    );
    let mut conflict = bytes.clone();
    conflict[0] ^= 1;
    assert!(assets
        .write(&mut store, &id, 0, &STANDARD.encode(conflict))
        .is_err());
    let prepared = assets
        .prepare_commit(
            &mut store,
            &id,
            AssetRuntime::default().validation_permit().unwrap(),
        )
        .unwrap()
        .validate()
        .unwrap();
    assets.publish(&mut store, prepared).unwrap();
    assert_eq!(
        assets
            .manifest(&mut store, &doc, &revision)
            .unwrap()
            .assets
            .len(),
        1
    );
}
#[test]
fn reservations_consume_quota_until_explicit_discard_and_invalid_images_never_publish() {
    let mut f = Fixture::new();
    let stamp = f.manifest().manifest;
    let mut pending = Vec::new();
    for target in ["figures/a.png", "b.jpg", "c.png", "d.png"] {
        pending.push(
            f.assets
                .begin(&mut f.store, &f.id, &f.revision, target, MAX_FILE, &stamp)
                .unwrap(),
        );
    }
    assert!(matches!(
        f.assets
            .begin(&mut f.store, &f.id, &f.revision, "e.png", 1, &stamp),
        Err(Error::Quota)
    ));
    assert_eq!(f.manifest().pending.len(), 4);
    assert!(f
        .assets
        .prepare_commit(
            &mut f.store,
            &pending[0],
            AssetRuntime::default().validation_permit().unwrap()
        )
        .is_err());
    f.assets.discard(&mut f.store, &pending[0]).unwrap();
    let invalid = f
        .assets
        .begin(&mut f.store, &f.id, &f.revision, "figures/a.png", 4, &stamp)
        .unwrap();
    f.assets
        .write(&mut f.store, &invalid, 0, &STANDARD.encode(b"oops"))
        .unwrap();
    assert!(f
        .assets
        .prepare_commit(
            &mut f.store,
            &invalid,
            AssetRuntime::default().validation_permit().unwrap()
        )
        .unwrap()
        .validate()
        .is_err());
    assert!(f.manifest().assets.is_empty());
    assert!(f
        .assets
        .write(
            &mut f.store,
            &invalid,
            0,
            &STANDARD.encode(vec![0; MAX_CHUNK + 1])
        )
        .is_err());
}
#[test]
fn source_revocation_and_root_replacement_fence_pending_publication_and_read_sessions() {
    let mut f = Fixture::new();
    let pending = f.upload("figures/a.png");
    let (receipt, stamp) = f.commit(&pending);
    let runtime = AssetRuntime::default();
    let prepared = f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &stamp,
            runtime.read_permit(Instant::now()).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    let (session, _) = runtime
        .install(&f.assets, &mut f.store, prepared, Instant::now())
        .unwrap();
    f.store.set_source_enabled(&f.source, false).unwrap();
    assert!(runtime
        .chunk(&f.assets, &mut f.store, &session, 0, Instant::now())
        .is_err());
    // A rejected read releases its slot immediately, rather than retaining
    // the decoded snapshot until a later TTL sweep.
    let first = runtime.read_permit(Instant::now()).unwrap();
    let second = runtime.read_permit(Instant::now()).unwrap();
    drop((first, second));
    f.store.set_source_enabled(&f.source, true).unwrap();
    let pending = f.upload("b.jpg");
    let validated = f
        .assets
        .prepare_commit(&mut f.store, &pending, runtime.validation_permit().unwrap())
        .unwrap()
        .validate()
        .unwrap();
    f.store.set_source_enabled(&f.source, false).unwrap();
    assert!(f.assets.publish(&mut f.store, validated).is_err());
    f.store.set_source_enabled(&f.source, true).unwrap();
    fs::rename(&f.root, f.temp.path().join("old-document")).unwrap();
    fs::create_dir(&f.root).unwrap();
    assert!(f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &stamp,
            runtime.read_permit(Instant::now()).unwrap()
        )
        .is_err());
}
#[test]
fn read_sessions_are_bounded_expire_and_detect_changed_private_files() {
    let mut f = Fixture::new();
    let pending = f.upload("figures/a.png");
    let (receipt, stamp) = f.commit(&pending);
    let runtime = AssetRuntime::default();
    let now = Instant::now();
    let one = runtime.read_permit(now).unwrap();
    let two = runtime.read_permit(now).unwrap();
    assert!(runtime.read_permit(now).is_err());
    drop(two);
    let snapshot = f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &stamp,
            one,
        )
        .unwrap()
        .verify()
        .unwrap();
    let (session, _) = runtime
        .install(&f.assets, &mut f.store, snapshot, now)
        .unwrap();
    assert!(runtime
        .chunk(&f.assets, &mut f.store, &session, 0, now + SESSION_TTL)
        .is_err());
    let permit = runtime.validation_permit().unwrap();
    assert!(runtime.validation_permit().is_err());
    drop(permit);
    assert!(runtime.validation_permit().is_ok());
    fs::write(
        f.assets.root.path.join(format!("{}.bin", receipt.asset_id)),
        vec![0; receipt.bytes as usize],
    )
    .unwrap();
    assert!(f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &stamp,
            runtime.read_permit(now).unwrap()
        )
        .unwrap()
        .verify()
        .is_err());
    let root = f.assets.root.path.clone();
    fs::rename(&root, f.temp.path().join("old-assets")).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(f.assets.manifest(&mut f.store, &f.id, &f.revision).is_err());
    assert!(AssetStore::open(&mut f.store, &root).is_err());
}

#[test]
fn inventory_and_explicit_forget_work_without_restoring_revoked_document_authority() {
    let mut f = Fixture::new();
    let pending = f.upload("figures/a.png");
    let (receipt, stamp) = f.commit(&pending);
    let pending = f.upload("b.jpg");
    let inventory = f.assets.inventory(&mut f.store).unwrap();
    assert_eq!(inventory.items.len(), 2);
    assert_eq!(inventory.reserved_bytes, receipt.bytes * 2);
    assert_eq!(inventory.max_bytes, MAX_LIBRARY_BYTES as u32);
    f.store.set_source_enabled(&f.source, false).unwrap();
    let original = f.root.join("document.md");
    fs::write(&original, "externally changed original").unwrap();
    assert!(f.assets.manifest(&mut f.store, &f.id, &f.revision).is_err());
    assert_eq!(f.assets.inventory(&mut f.store).unwrap().items.len(), 2);

    // A failed private deletion retires read authority, retains quota and is
    // visible for explicit retry even though the original is unreadable.
    let copy = f.assets.root.path.join(format!("{}.bin", receipt.asset_id));
    fs::remove_file(&copy).unwrap();
    fs::create_dir(&copy).unwrap();
    assert!(f.assets.forget(&mut f.store, &receipt.asset_id).is_err());
    let retained = f.assets.inventory(&mut f.store).unwrap();
    assert_eq!(retained.reserved_bytes, inventory.reserved_bytes);
    assert_eq!(
        retained
            .items
            .iter()
            .find(|i| i.asset_id == receipt.asset_id)
            .unwrap()
            .state,
        "retired"
    );
    let items = f.assets.items(&mut f.store, &f.id, &f.revision).unwrap();
    assert_ne!(
        AssetStore::stamp(&mut f.store, &f.id, &f.revision, &items).unwrap(),
        stamp
    );

    fs::remove_dir(&copy).unwrap();
    f.assets.forget(&mut f.store, &receipt.asset_id).unwrap();
    f.assets.forget(&mut f.store, &pending).unwrap();
    assert_eq!(f.assets.inventory(&mut f.store).unwrap().reserved_bytes, 0);
    assert!(f.assets.inventory(&mut f.store).unwrap().items.is_empty());
    assert_eq!(
        fs::read_to_string(&original).unwrap(),
        "externally changed original"
    );
    assert!(
        !f.store
            .list_sources()
            .unwrap()
            .iter()
            .find(|s| s.id == f.source)
            .unwrap()
            .enabled
    );
    assert!(f
        .assets
        .forget(&mut f.store, "../../document/document.md")
        .is_err());
}

#[test]
fn selectable_targets_are_bounded_and_report_truncation() {
    let mut f = Fixture::new();
    let markdown: String = (0..65)
        .map(|i| format!("![Figure](figures/{i:03}.png)\n"))
        .collect();
    fs::write(f.root.join("document.md"), &markdown).unwrap();
    f.revision = blake3::hash(markdown.as_bytes()).to_hex().to_string();
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_documents_v1 SET revision=?1 WHERE id=?2",
                params![f.revision, f.id],
            )
        })
        .unwrap();
    let manifest = f.manifest();
    assert_eq!(manifest.targets.len(), 64);
    assert!(manifest.targets_truncated);
    assert_eq!(manifest.targets[0], "figures/000.png");
    assert_eq!(manifest.targets[63], "figures/063.png");
}

#[test]
fn future_document_schema_blocks_asset_authority_but_keeps_cleanup_metadata_available() {
    let mut f = Fixture::new();
    let pending = f.upload("figures/a.png");
    let (receipt, stamp) = f.commit(&pending);
    let copy = f.assets.root.path.join(format!("{}.bin", receipt.asset_id));
    let before = fs::read(&copy).unwrap();
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_document_schema_v1 SET version=2 WHERE singleton=1",
                [],
            )
        })
        .unwrap();
    assert!(matches!(
        f.assets.manifest(&mut f.store, &f.id, &f.revision),
        Err(Error::UnsupportedDocumentSchema)
    ));
    let runtime = AssetRuntime::default();
    assert!(f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &stamp,
            runtime.read_permit(Instant::now()).unwrap(),
        )
        .is_err());
    let inventory = f.assets.inventory(&mut f.store).unwrap();
    assert_eq!(inventory.items.len(), 1);
    assert_eq!(fs::read(copy).unwrap(), before);
}

#[test]
fn additive_schema_upgrade_preserves_copies_and_future_versions_fail_closed() {
    let mut f = Fixture::new();
    let pending = f.upload("figures/a.png");
    let (receipt, stamp) = f.commit(&pending);
    let copy = f.assets.root.path.join(format!("{}.bin", receipt.asset_id));
    let before = fs::read(&copy).unwrap();
    // The unpublished prototype had these exact tables but no version marker.
    f.store
        .with_library_connection(|db| {
            db.execute(
                "DELETE FROM device_asset_meta_v1 WHERE key='schema_version'",
                [],
            )
        })
        .unwrap();
    drop(f.assets);
    drop(f.store);
    f.store = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    f.assets = AssetStore::open(&mut f.store, &f.temp.path().join("assets")).unwrap();
    assert_eq!(f.manifest().manifest, stamp);
    assert_eq!(f.manifest().assets[0].asset_id, receipt.asset_id);
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_meta_v1 SET value='99' WHERE key='schema_version'",
                [],
            )
        })
        .unwrap();
    assert!(matches!(
        AssetStore::open(&mut f.store, &f.temp.path().join("assets")),
        Err(Error::UnsupportedSchema)
    ));
    let version: String = f
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT value FROM device_asset_meta_v1 WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(version, "99");
    assert_eq!(fs::read(&copy).unwrap(), before);
}

#[path = "binding_tests.rs"]
mod binding_tests;
