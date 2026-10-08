use super::*;
use crate::{
    device,
    device::assets::{AssetRuntime, AssetStore},
    ReaderStore,
};
use rusqlite::params;
use std::path::PathBuf;
struct Fixture {
    temp: tempfile::TempDir,
    store: ReaderStore,
    assets: AssetStore,
    documents: PathBuf,
    origin: IncomingOrigin,
    runtime: AssetRuntime,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(&temp.path().join("data")).unwrap();
        let assets = AssetStore::open(&mut store, &temp.path().join("assets")).unwrap();
        let documents = temp.path().join("documents");
        let origin = IncomingOrigin {
            scope_digest: [42; 32],
            item_id: uuid::Uuid::new_v4(),
            wire_revision: uuid::Uuid::new_v4(),
            session_epoch: uuid::Uuid::new_v4(),
            selection_epoch: uuid::Uuid::new_v4(),
        };
        Self {
            temp,
            store,
            assets,
            documents,
            origin,
            runtime: AssetRuntime::default(),
        }
    }
    fn input() -> IncomingDocument {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        IncomingDocument::new(
            "incoming.md".into(),
            b"\xef\xbb\xbf# Exact\r\n\r\n![Image](figure.png)\r\n".to_vec(),
            vec![
                IncomingAsset::new("figure.png".into(), "image/png".into(), png.into_inner())
                    .unwrap(),
            ],
        )
        .unwrap()
    }
    fn plan(&mut self) -> WritePlan {
        reserve_new(
            &mut self.store,
            &self.documents,
            &self.assets,
            &self.origin,
            &Self::input(),
        )
        .unwrap()
    }
    fn ready(&mut self) -> ReadyIncoming {
        self.plan()
            .write(Self::input())
            .unwrap()
            .validate(self.runtime.validation_permit().unwrap())
            .unwrap()
    }
    fn count(&mut self, table: &str) -> u64 {
        self.store
            .with_library_connection(|db| {
                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            })
            .unwrap()
    }
}
fn commit(f: &mut Fixture, ready: &ReadyIncoming) -> PublicationReceipt {
    with_publication(
        &mut f.store,
        ready,
        &f.origin,
        |db, guard| -> PublicationResult<_> {
            let tx = db.transaction()?;
            let receipt = publish_new(&tx, &guard, ready)?;
            revalidate_publication(&tx, &guard, ready)?;
            tx.commit()?;
            Ok(receipt)
        },
    )
    .unwrap()
    .unwrap()
}
#[test]
fn exact_originals_and_assets_publish_with_outer_receipt_or_rollback_together() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let id = ready.document_id().to_owned();
    let failed=with_publication(&mut f.store,&ready,&f.origin,|db,guard|->PublicationResult<()> {
        let tx=db.transaction()?;publish_new(&tx,&guard,&ready)?;
        tx.execute_batch("CREATE TABLE test_receipt(value TEXT);INSERT INTO test_receipt VALUES('provisional');")?;
        Err(PublicationError::Conflict)
    }).unwrap();
    assert!(failed.is_err());
    assert_eq!(f.count("sources"), 0);
    assert_eq!(f.count("device_documents_v1"), 0);
    let state: String = f
        .store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT state FROM device_incoming_intents_v1 WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(state, "reserved");
    let receipt = commit(&mut f, &ready);
    assert!(!receipt.document.indexed);
    assert_eq!(receipt.assets.len(), 1);
    let expected = Fixture::input();
    assert_eq!(
        device::read(&mut f.store, &id, Some(&expected.revision))
            .unwrap()
            .markdown
            .as_bytes(),
        expected.markdown
    );
    assert_eq!(
        std::fs::read(
            f.temp
                .path()
                .join("assets")
                .join(format!("{}.bin", receipt.assets[0].asset_id))
        )
        .unwrap(),
        expected.assets[0].bytes
    );
    assert_ne!(id, f.origin.item_id.to_string());
    assert!(f
        .store
        .list_documents(&receipt.document.source_id)
        .unwrap()
        .is_empty());
    assert!(reserve_new(
        &mut f.store,
        &f.documents,
        &f.assets,
        &f.origin,
        &Fixture::input()
    )
    .is_err());
    drop(ready);
    drop(f.assets);
    drop(f.store);
    let mut reopened = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    let library = device::recover_imports(&mut reopened, &f.documents).unwrap();
    assert_eq!(library.documents[0].id, id);
    assert!(library.pending.is_empty());
    assert_eq!(
        device::read(&mut reopened, &id, Some(&expected.revision))
            .unwrap()
            .markdown
            .as_bytes(),
        expected.markdown
    );
}
#[test]
fn interrupted_incoming_never_autoactivates_and_exact_resume_is_explicit() {
    let mut f = Fixture::new();
    let plan = f.plan();
    let id = plan.document_id().to_owned();
    use std::io::Write;
    let mut partial = plan
        .root
        .create_new_relative(std::path::Path::new("document.md"))
        .unwrap();
    partial.write_all(&Fixture::input().markdown[..5]).unwrap();
    partial.sync_all().unwrap();
    drop(partial);
    assert!(device::recover_imports(&mut f.store, &f.documents)
        .unwrap()
        .documents
        .is_empty());
    assert_eq!(f.count("sources"), 0);
    drop(plan);
    drop(f.assets);
    drop(f.store);
    f.store = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    f.assets = AssetStore::open(&mut f.store, &f.temp.path().join("assets")).unwrap();
    let first = resume(&mut f.store, &f.documents, &f.assets, &id, &f.origin).unwrap();
    assert!(first.read().is_err());
    let replay = reserve_new(
        &mut f.store,
        &f.documents,
        &f.assets,
        &f.origin,
        &Fixture::input(),
    )
    .unwrap();
    assert_eq!(replay.document_id(), id);
    let ready = replay
        .write(Fixture::input())
        .unwrap()
        .validate(f.runtime.validation_permit().unwrap())
        .unwrap();
    commit(&mut f, &ready);
    assert_eq!(f.count("device_documents_v1"), 1);
}
#[test]
fn epoch_wrong_connection_collision_and_final_callback_mutation_cannot_commit() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let mut changed = f.origin.clone();
    changed.session_epoch = uuid::Uuid::new_v4();
    assert!(
        with_publication(&mut f.store, &ready, &changed, |_, _| Ok::<
            _,
            PublicationError,
        >(()))
        .is_err()
    );
    let mut wrong = rusqlite::Connection::open_in_memory().unwrap();
    let rejected = with_publication(
        &mut f.store,
        &ready,
        &f.origin,
        |_, guard| -> PublicationResult<()> {
            let tx = wrong.transaction()?;
            publish_new(&tx, &guard, &ready)?;
            Ok(())
        },
    )
    .unwrap();
    assert!(matches!(rejected, Err(PublicationError::Conflict)));
    for mutation in ["revoke", "cache", "generation"] {
        let result = with_publication(
            &mut f.store,
            &ready,
            &f.origin,
            |db, guard| -> PublicationResult<()> {
                let tx = db.transaction()?;
                let receipt = publish_new(&tx, &guard, &ready)?;
                if mutation == "revoke" {
                    tx.execute(
                        "UPDATE sources SET enabled=0 WHERE id=?1",
                        [&receipt.document.source_id],
                    )?;
                } else if mutation=="generation" {
                    tx.execute("UPDATE device_asset_generations_v1 SET generation=generation+1 WHERE document_id=?1",[ready.document_id()])?;
                } else {
                    use std::os::unix::ffi::OsStrExt;
                    tx.execute("INSERT INTO core_provider_profile(singleton,library_id,cache_path,cache_identity) VALUES(1,?1,?2,?3)",params![uuid::Uuid::new_v4().to_string(),ready.plan.root.path.as_os_str().as_bytes(),serde_json::to_string(&ready.plan.root.identity)?])?;
                }
                revalidate_publication(&tx, &guard, &ready)?;
                tx.commit()?;
                Ok(())
            },
        )
        .unwrap();
        if mutation == "revoke" {
            assert!(matches!(result, Err(PublicationError::Conflict)));
        } else if mutation == "generation" {
            assert!(matches!(
                result,
                Err(PublicationError::Asset(device::assets::Error::Unavailable))
            ));
        } else {
            assert!(matches!(
                result,
                Err(PublicationError::Device(device::Error::InvalidSelection))
            ));
        }
        assert_eq!(f.count("sources"), 0);
    }
    let intent = &ready.plan.intent;
    f.store.with_library_connection(|db|db.execute("INSERT INTO device_import_intents_v1(id,name,revision,bytes,parent_identity,state) VALUES(?1,?2,?3,?4,?5,'reserved')",params![intent.id,intent.name,intent.revision,intent.bytes,intent.parent_identity])).unwrap();
    let result = with_publication(
        &mut f.store,
        &ready,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            publish_new(&tx, &guard, &ready)?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(matches!(result, Err(PublicationError::Conflict)));
    assert_eq!(f.count("sources"), 0);
}
#[test]
fn changed_copy_or_root_and_failed_pixel_decode_preserve_unapplied_reservations() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let file = f.documents.join(ready.document_id()).join("document.md");
    let mut bytes = std::fs::read(&file).unwrap();
    bytes[4] ^= 1;
    std::fs::write(&file, bytes).unwrap();
    assert!(
        with_publication(&mut f.store, &ready, &f.origin, |_, _| Ok::<
            _,
            PublicationError,
        >(()))
        .is_err()
    );
    assert_eq!(f.count("sources"), 0);
    drop(ready);
    let mut changed_root = Fixture::new();
    let ready = changed_root.ready();
    let directory = changed_root.documents.join(ready.document_id());
    std::fs::rename(&directory, directory.with_extension("moved")).unwrap();
    std::fs::create_dir(&directory).unwrap();
    assert!(with_publication(
        &mut changed_root.store,
        &ready,
        &changed_root.origin,
        |_, _| Ok::<_, PublicationError>(())
    )
    .is_err());
    assert_eq!(changed_root.count("sources"), 0);
    let mut g = Fixture::new();
    let invalid = IncomingDocument::new(
        "bad.md".into(),
        b"![bad](figure.png)".to_vec(),
        vec![IncomingAsset::new(
            "figure.png".into(),
            "image/png".into(),
            b"invalid pixel data".to_vec(),
        )
        .unwrap()],
    )
    .unwrap();
    let plan = reserve_new(&mut g.store, &g.documents, &g.assets, &g.origin, &invalid).unwrap();
    assert!(matches!(
        plan.write(invalid)
            .unwrap()
            .validate(g.runtime.validation_permit().unwrap()),
        Err(PublicationError::Asset(
            crate::device::assets::Error::InvalidImage
        ))
    ));
    assert_eq!(g.count("sources"), 0);
    assert!(device::recover_imports(&mut g.store, &g.documents)
        .unwrap()
        .documents
        .is_empty());
}
#[test]
fn incoming_quota_dropping_fence_and_future_schema_are_shared_with_local_imports() {
    let mut f = Fixture::new();
    let plan = f.plan();
    let id = plan.document_id().to_owned();
    let intent = plan.intent.clone();
    f.store.with_library_connection(|db| {
        for _ in 0..64 {db.execute("INSERT INTO device_incoming_intents_v1(id,origin,name,revision,bytes,input_digest,parent_identity,assets_identity,assets,state) VALUES(?1,?2,'quota.md',?3,1048576,?4,?5,?6,'[]','reserved')",params![uuid::Uuid::new_v4().to_string(),uuid::Uuid::new_v4().to_string(),intent.revision,intent.digest,intent.parent_identity,intent.assets_identity]).unwrap();}
    });
    assert!(matches!(
        device::import(
            &mut f.store,
            &f.documents,
            "local.md".into(),
            "local".into()
        ),
        Err(device::Error::Quota)
    ));
    f.store
        .with_library_connection(|db| {
            db.execute("DELETE FROM device_incoming_intents_v1 WHERE id<>?1", [&id])
        })
        .unwrap();
    let ready = plan
        .write(Fixture::input())
        .unwrap()
        .validate(f.runtime.validation_permit().unwrap())
        .unwrap();
    let discard = prepare_drop(&mut f.store, &f.documents, &f.assets, &id, &f.origin).unwrap();
    let rejected = with_publication(
        &mut f.store,
        &ready,
        &f.origin,
        |db, guard| -> PublicationResult<()> {
            let tx = db.transaction()?;
            publish_new(&tx, &guard, &ready)?;
            tx.commit()?;
            Ok(())
        },
    )
    .unwrap();
    assert!(rejected.is_err());
    drop(ready);
    finish_drop(&mut f.store, discard.remove_files().unwrap()).unwrap();
    assert_eq!(f.assets.inventory(&mut f.store).unwrap().reserved_bytes, 0);
    assert!(reserve_new(
        &mut f.store,
        &f.documents,
        &f.assets,
        &f.origin,
        &Fixture::input()
    )
    .is_err());
    f.store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_incoming_schema_v1 SET version=2 WHERE singleton=1",
                [],
            )
        })
        .unwrap();
    assert!(matches!(
        device::import(
            &mut f.store,
            &f.documents,
            "local.md".into(),
            "local".into()
        ),
        Err(device::Error::UnsupportedSchema)
    ));
    let version: i64 = f
        .store
        .with_library_connection(|db| {
            db.query_row("SELECT version FROM device_incoming_schema_v1", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(f.count("sources"), 0);
}

#[path = "transition_tests.rs"]
mod transitions;
