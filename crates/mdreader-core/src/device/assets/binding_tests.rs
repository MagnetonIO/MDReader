use super::*;

#[test]
fn cached_owner_reuse_and_current_asset_namespace_are_revalidated() {
    let mut f = Fixture::new();
    let runtime = AssetRuntime::default();
    let directory = f.temp.path().join("assets");
    runtime.assets(&mut f.store, &directory).unwrap();
    // Same durable database/path does not identify another live owner session.
    let mut other = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    assert!(matches!(
        runtime.assets(&mut other, &directory),
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        f.assets.inventory(&mut other),
        Err(Error::Unavailable)
    ));
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_meta_v1 SET value='2' WHERE key='schema_version'",
                [],
            )
        })
        .unwrap();
    assert!(matches!(
        runtime.assets(&mut f.store, &directory),
        Err(Error::UnsupportedSchema)
    ));
    assert!(matches!(
        f.assets.inventory(&mut f.store),
        Err(Error::UnsupportedSchema)
    ));
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_meta_v1 SET value='1' WHERE key='schema_version'",
                [],
            )
        })
        .unwrap();
    let original: String = f
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT value FROM device_asset_meta_v1 WHERE key='root_identity'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    let replacement = SourceRoot::capture(f.root.clone()).unwrap();
    let changed = serde_json::to_string(&replacement.identity).unwrap();
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_meta_v1 SET value=?1 WHERE key='root_identity'",
                [changed],
            )
        })
        .unwrap();
    assert!(matches!(
        f.assets.inventory(&mut f.store),
        Err(Error::Unavailable)
    ));
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_meta_v1 SET value=?1 WHERE key='root_identity'",
                [original],
            )
        })
        .unwrap();
    assert!(f.assets.inventory(&mut f.store).unwrap().items.is_empty());
}

#[test]
fn prepared_values_cannot_cross_owners_roots_or_session_permit_origins() {
    let mut f = Fixture::new();
    let pending = f.upload("figures/a.png");
    let runtime = AssetRuntime::default();
    let validated = f
        .assets
        .prepare_commit(&mut f.store, &pending, runtime.validation_permit().unwrap())
        .unwrap()
        .validate()
        .unwrap();
    let mut other = Fixture::new();
    assert!(matches!(
        other.assets.publish(&mut other.store, validated),
        Err(Error::Unavailable)
    ));
    let other_pending = other.upload("figures/a.png");
    let (other_receipt, other_manifest) = other.commit(&other_pending);
    runtime
        .assets(&mut f.store, &f.temp.path().join("assets"))
        .unwrap();
    let other_snapshot = other
        .assets
        .prepare_open(
            &mut other.store,
            &other.id,
            &other.revision,
            &other_receipt.asset_id,
            &other_manifest,
            runtime.read_permit(Instant::now()).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    assert!(matches!(
        runtime.install(
            &other.assets,
            &mut other.store,
            other_snapshot,
            Instant::now()
        ),
        Err(Error::Unavailable)
    ));
    // Rejected publication releases the carried validation permit.
    assert!(runtime.validation_permit().is_ok());
    // A changed root under the same live owner still cannot receive old preparation.
    let validated = f
        .assets
        .prepare_commit(&mut f.store, &pending, runtime.validation_permit().unwrap())
        .unwrap()
        .validate()
        .unwrap();
    let original = serde_json::to_string(&f.assets.root.identity).unwrap();
    let second_directory = f.temp.path().join("other-assets");
    f.store
        .with_library_connection(|db| {
            db.execute(
                "DELETE FROM device_asset_meta_v1 WHERE key='root_identity'",
                [],
            )
        })
        .unwrap();
    let second_assets = AssetStore::open(&mut f.store, &second_directory).unwrap();
    assert!(matches!(
        second_assets.publish(&mut f.store, validated),
        Err(Error::Unavailable)
    ));
    assert!(second_assets
        .inventory(&mut f.store)
        .unwrap()
        .items
        .iter()
        .all(|item| item.state == "reserved"));
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_meta_v1 SET value=?1 WHERE key='root_identity'",
                [original],
            )
        })
        .unwrap();
    let (receipt, manifest) = f.commit(&pending);
    let now = Instant::now();
    let snapshot = f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &manifest,
            runtime.read_permit(now).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    assert!(matches!(
        runtime.install(&other.assets, &mut other.store, snapshot, now),
        Err(Error::Unavailable)
    ));
    let snapshot = f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &manifest,
            runtime.read_permit(now).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    assert!(matches!(
        AssetRuntime::default().install(&f.assets, &mut f.store, snapshot, now),
        Err(Error::Unavailable)
    ));
    let snapshot = f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &manifest,
            runtime.read_permit(now).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    let (session, _) = runtime
        .install(&f.assets, &mut f.store, snapshot, now)
        .unwrap();
    let chunk = runtime
        .chunk(&f.assets, &mut f.store, &session, 0, now)
        .unwrap();
    assert_eq!(chunk.offset, 0);
    assert!(chunk.complete);
    assert_eq!(STANDARD.decode(chunk.base64).unwrap(), Fixture::png());
    let snapshot = f
        .assets
        .prepare_open(
            &mut f.store,
            &f.id,
            &f.revision,
            &receipt.asset_id,
            &manifest,
            runtime.read_permit(now).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    let (invalidated_session, _) = runtime
        .install(&f.assets, &mut f.store, snapshot, now)
        .unwrap();
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_asset_meta_v1 SET value='2' WHERE key='schema_version'",
                [],
            )
        })
        .unwrap();
    assert!(matches!(
        runtime.chunk(&f.assets, &mut f.store, &invalidated_session, 0, now),
        Err(Error::UnsupportedSchema)
    ));
    // Complete delivery and authority invalidation both release their permits.
    let first = runtime.read_permit(now).unwrap();
    let second = runtime.read_permit(now).unwrap();
    assert!(matches!(runtime.read_permit(now), Err(Error::ReadsBusy)));
    drop((first, second));
}
