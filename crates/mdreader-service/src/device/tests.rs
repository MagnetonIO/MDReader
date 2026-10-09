use super::*;
use crate::catalog::{CatalogOpen, CatalogReply, CatalogRequest};
use mdreader_core::device::{assets::AssetStore, publication as p};

// A small public structural PNG fully decoded by the real core validator.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3, 2, 0,
    239, 162, 167, 91, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
struct Fixture {
    _temp: tempfile::TempDir,
    profile: PathBuf,
    documents: PathBuf,
    assets: PathBuf,
    owner: Arc<Mutex<ReaderStore>>,
    queries: Arc<Mutex<ReaderStore>>,
    runtime: AssetRuntime,
    receipt: catalog::Receipt,
}

#[test]
fn imported_library_over_128_documents_keeps_global_search_and_exact_live_scope() {
    let f = Fixture::new();
    let mut last = None;
    {
        let mut owner = f.owner.lock().unwrap();
        for index in 0..129 {
            let markdown = if index == 128 {
                "# Imported\n\nquartzneedle\n"
            } else {
                "# Imported\n\nordinary text\n"
            };
            last = Some(
                mdreader_core::device::import(
                    &mut owner,
                    &f.documents,
                    format!("Imported-{index}.md"),
                    markdown.into(),
                )
                .unwrap(),
            );
        }
    }
    let last = last.unwrap();
    let device = f.coordinator();
    let receipts = device.search_sources().unwrap();
    assert_eq!(receipts.len(), 130);
    let sources: Vec<_> = receipts.iter().map(|r| r.source_id.clone()).collect();
    let hits = f
        .queries
        .lock()
        .unwrap()
        .search("quartzneedle", &sources, 20)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source_id, last.source_id);
    assert_eq!(hits[0].revision, last.revision);
    assert!(f
        .queries
        .lock()
        .unwrap()
        .search(
            "quartzneedle",
            &vec![last.source_id.clone(); mdreader_core::MAX_SEARCH_SOURCES + 1],
            20
        )
        .is_err());
    f.owner
        .lock()
        .unwrap()
        .set_source_enabled(&last.source_id, false)
        .unwrap();
    assert!(f
        .queries
        .lock()
        .unwrap()
        .search("quartzneedle", &sources, 20)
        .is_err());
    assert!(!device
        .search_sources()
        .unwrap()
        .iter()
        .any(|r| r.source_id == last.source_id));
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let documents = temp.path().join("documents");
        let assets = temp.path().join("assets");
        let mut store = ReaderStore::open(&profile).unwrap();
        let runtime = AssetRuntime::default();
        let materialization = runtime.assets(&mut store, &assets).unwrap();
        let input = || {
            p::IncomingDocument::new(
                "received.md".into(),
                b"# Received\n\n![Pixel](pixel.png)\n".to_vec(),
                vec![
                    p::IncomingAsset::new("pixel.png".into(), "image/png".into(), PNG.to_vec())
                        .unwrap(),
                ],
            )
            .unwrap()
        };
        let origin = p::IncomingOrigin {
            scope_digest: [42; 32],
            item_id: uuid::Uuid::new_v4(),
            wire_revision: uuid::Uuid::new_v4(),
            session_epoch: uuid::Uuid::new_v4(),
            selection_epoch: uuid::Uuid::new_v4(),
        };
        let ready = p::reserve_new(&mut store, &documents, &materialization, &origin, &input())
            .unwrap()
            .write(input())
            .unwrap()
            .validate(runtime.validation_permit().unwrap())
            .unwrap();
        let published = p::with_publication(
            &mut store,
            &ready,
            &origin,
            |db, guard| -> p::PublicationResult<_> {
                let tx = db.transaction()?;
                let receipt = p::publish_new(&tx, &guard, &ready)?;
                p::revalidate_publication(&tx, &guard, &ready)?;
                tx.commit()?;
                Ok(receipt)
            },
        )
        .unwrap()
        .unwrap();
        let receipt = catalog::receipt(&published.document);
        let queries = Arc::new(Mutex::new(ReaderStore::open_readonly(&profile).unwrap()));
        Self {
            _temp: temp,
            profile,
            documents,
            assets,
            owner: Arc::new(Mutex::new(store)),
            queries,
            runtime,
            receipt,
        }
    }
    fn coordinator(&self) -> Coordinator {
        Coordinator::new(
            self.owner.clone(),
            self.queries.clone(),
            self.documents.clone(),
            self.assets.clone(),
            self.runtime.clone(),
        )
    }
    fn catalog(&self, device: Arc<Coordinator>) -> crate::catalog::Coordinator {
        let mut catalog =
            crate::catalog::Coordinator::new(self.owner.clone(), self.queries.clone());
        catalog.enable_device_catalog(device);
        catalog
    }
}

#[test]
fn actual_publication_is_discoverable_readable_and_uses_the_same_bounded_image_runtime() {
    let f = Fixture::new();
    let device = Arc::new(f.coordinator());
    let selected = device.search_sources().unwrap();
    assert!(selected == [f.receipt.clone()]);
    let catalog = f.catalog(device.clone());
    let CatalogReply::List { sources, .. } = catalog
        .execute(CatalogRequest::List {
            local_sources: vec![],
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(sources
        .iter()
        .any(|s| s.catalog_id == "device:library" && s.kind == "device"));
    let CatalogReply::Page { documents, .. } = catalog
        .execute(CatalogRequest::Page {
            catalog_id: "device:library".into(),
            cursor: None,
            limit: 100,
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(
        matches!(&documents[0].open, CatalogOpen::Device { document_id, revision, .. } if document_id == &f.receipt.document_id && revision == &f.receipt.revision)
    );
    let DeviceReply::ReadDocument { markdown, .. } = device
        .execute(DeviceRequest::Read {
            receipt: f.receipt.clone(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(markdown, "# Received\n\n![Pixel](pixel.png)\n");
    assert!(
        f.owner
            .lock()
            .unwrap()
            .list_documents(&f.receipt.source_id)
            .unwrap()
            .is_empty(),
        "catalog discovery does not require an index turn"
    );
    let DeviceReply::Manifest {
        value: Some(manifest),
    } = device
        .execute(DeviceRequest::Manifest {
            receipt: f.receipt.clone(),
        })
        .unwrap()
    else {
        panic!()
    };
    let open = || DeviceRequest::Open {
        receipt: f.receipt.clone(),
        asset_id: manifest.assets[0].asset_id.clone(),
        manifest: manifest.manifest.clone(),
    };
    let held = f.runtime.read_permit(Instant::now()).unwrap();
    let DeviceReply::Opened { session_id, .. } = device.execute(open()).unwrap() else {
        panic!()
    };
    assert!(
        matches!(device.execute(open()), Err(DeviceError::Busy)),
        "publication and reading share one permit pool"
    );
    let DeviceReply::Chunk {
        base64, complete, ..
    } = device
        .execute(DeviceRequest::Chunk {
            session_id,
            offset: 0,
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(complete);
    assert!(!base64.is_empty());
    drop(held);
    let DeviceReply::Opened { session_id, .. } = device.execute(open()).unwrap() else {
        panic!()
    };
    {
        let mut owner = f.owner.lock().unwrap();
        owner
            .set_source_enabled(&f.receipt.source_id, false)
            .unwrap();
        owner
            .set_source_enabled(&f.receipt.source_id, true)
            .unwrap();
    }
    assert!(
        device
            .execute(DeviceRequest::Chunk {
                session_id: session_id.clone(),
                offset: 0
            })
            .is_err(),
        "grant ABA cannot revive an old image lease"
    );
    device.execute(DeviceRequest::Close { session_id }).unwrap();
    let _one = f.runtime.read_permit(Instant::now()).unwrap();
    let _two = f.runtime.read_permit(Instant::now()).unwrap();
    drop((_one, _two));
    let outside = f.runtime.read_permit(Instant::now()).unwrap();
    let DeviceReply::Opened { .. } = device.execute(open()).unwrap() else {
        panic!()
    };
    let operation = uuid::Uuid::new_v4();
    let epoch = uuid::Uuid::new_v4();
    let frozen = device
        .freeze(
            operation,
            epoch,
            Instant::now() + std::time::Duration::from_secs(2),
        )
        .unwrap();
    frozen.revalidate(&device, operation, epoch).unwrap();
    assert!(matches!(
        device.execute(DeviceRequest::Read {
            receipt: f.receipt.clone()
        }),
        Err(DeviceError::Busy)
    ));
    assert!(matches!(
        catalog.execute(CatalogRequest::Page {
            catalog_id: "device:library".into(),
            cursor: None,
            limit: 100
        }),
        Err(crate::catalog::CatalogError::Busy)
    ));
    let freed = f.runtime.read_permit(Instant::now()).unwrap();
    assert!(
        f.runtime.read_permit(Instant::now()).is_err(),
        "freezing this participant must preserve unrelated shared-runtime permits"
    );
    assert!(frozen
        .revalidate(&f.coordinator(), operation, epoch)
        .is_err());
    drop((frozen, outside, freed));
    assert!(device
        .execute(DeviceRequest::Read {
            receipt: f.receipt.clone()
        })
        .is_ok());
    let active = device.admission.begin().unwrap();
    assert!(matches!(
        device.freeze(
            operation,
            epoch,
            Instant::now() + std::time::Duration::from_millis(5)
        ),
        Err(DeviceError::Busy)
    ));
    drop(active);
    assert!(
        device
            .execute(DeviceRequest::Read {
                receipt: f.receipt.clone()
            })
            .is_ok(),
        "a failed drain must reopen admission without claiming quiescence"
    );
}

#[test]
fn runtime_routes_device_catalog_and_read_without_requiring_configured_cloud_credentials() {
    let f = Fixture::new();
    let runtime = crate::runtime::Runtime::new_with_adapters(
        f.owner.clone(),
        f.profile.join("models"),
        crate::ServiceOptions {
            disable_model: true,
        },
        crate::NativeAdapters {
            factory: None,
            accounts: Some((
                Err(crate::accounts::AccountError::Invalid),
                crate::accounts::CacheRoots {
                    documents: f.documents.clone(),
                    assets: f.assets.clone(),
                },
            )),
        },
    )
    .unwrap();
    let list = runtime
        .execute(crate::Request::Catalog {
            request: CatalogRequest::List {
                local_sources: vec![],
            },
        })
        .unwrap();
    assert!(list["result"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["catalogId"] == "device:library"));
    let owned: crate::catalog::ReaderOwnedSources =
        serde_json::from_value(runtime.execute(crate::Request::ReaderOwnedSources).unwrap())
            .unwrap();
    assert!(
        owned.devices.len() == 1
            && owned.devices[0].document_id == f.receipt.document_id
            && owned.devices[0].source_id == f.receipt.source_id
            && owned.devices[0].revision == f.receipt.revision
    );
    let body = runtime
        .execute(crate::Request::Device {
            request: DeviceRequest::Read { receipt: f.receipt },
        })
        .unwrap();
    assert_eq!(
        body["result"]["markdown"],
        "# Received\n\n![Pixel](pixel.png)\n"
    );
    assert!(body["error"].is_null());
    let status = runtime
        .execute(crate::Request::Account {
            request: crate::accounts::rpc::AccountRequest::Status {},
        })
        .unwrap();
    assert_eq!(status["error"], "invalid");
}

#[test]
fn readonly_device_reads_work_during_writer_contention_and_fail_closed_on_schema_or_scope_change() {
    let f = Fixture::new();
    let device = Arc::new(f.coordinator());
    let mut owner = f.owner.lock().unwrap();
    assert!(matches!(
        device.execute(DeviceRequest::Read {
            receipt: f.receipt.clone()
        }),
        Ok(DeviceReply::ReadDocument { .. })
    ));
    let CatalogReply::List { sources, .. } = f
        .catalog(device.clone())
        .execute(CatalogRequest::List {
            local_sources: vec![],
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(sources
        .iter()
        .any(|s| s.kind == "device" && s.failure.is_none()));
    assert!(matches!(
        device.execute(DeviceRequest::Manifest {
            receipt: f.receipt.clone()
        }),
        Err(DeviceError::Busy)
    ));
    let before = owner.with_library_connection(|db| db.total_changes());
    drop(owner);
    let wrong_scope = Coordinator::new(
        f.owner.clone(),
        f.queries.clone(),
        f._temp.path().to_path_buf(),
        f.assets.clone(),
        AssetRuntime::default(),
    );
    assert!(wrong_scope
        .execute(DeviceRequest::Read {
            receipt: f.receipt.clone()
        })
        .is_err());
    owner = f.owner.lock().unwrap();
    assert_eq!(
        owner.with_library_connection(|db| db.total_changes()),
        before
    );
    owner
        .with_library_connection(|db| {
            db.execute("UPDATE device_document_schema_v1 SET version=99", [])
        })
        .unwrap();
    drop(owner);
    assert!(matches!(
        device.execute(DeviceRequest::Read {
            receipt: f.receipt.clone()
        }),
        Err(DeviceError::Unsupported)
    ));
    assert!(AssetStore::open_existing(
        &mut ReaderStore::open_readonly(&f.profile).unwrap(),
        &f.assets
    )
    .unwrap()
    .is_some());
    assert!(serde_json::from_value::<DeviceRequest>(
        serde_json::json!({"operation":"read","receipt":f.receipt,"path":"/private"})
    )
    .is_err());
    assert!(serde_json::from_value::<DeviceRequest>(
        serde_json::json!({"operation":"manifest","receipt":f.receipt,"token":"secret"})
    )
    .is_err());
}
