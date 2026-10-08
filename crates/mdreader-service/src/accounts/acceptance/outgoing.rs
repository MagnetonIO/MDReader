//! Extends the one actual actor fixture; no synthetic authority or queue bypass.
use super::*;
use mdreader_core::device::{assets::AssetReceipt, DeviceDocument};
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=";

fn local(account: &Account, markdown: &str) -> (DeviceDocument, AssetReceipt) {
    let (document, assets, prepared) = {
        let mut owner = account.owner.lock().unwrap();
        let document = device::import(
            &mut owner,
            &account.documents,
            "Selected native public fixture.md".into(),
            markdown.into(),
        )
        .unwrap();
        let assets = account.runtime.assets(&mut owner, &account.assets).unwrap();
        let manifest = assets
            .manifest(&mut owner, &document.id, &document.revision)
            .unwrap();
        let id = assets
            .begin(
                &mut owner,
                &document.id,
                &document.revision,
                "pixel.png",
                68,
                &manifest.manifest,
            )
            .unwrap();
        assets.write(&mut owner, &id, 0, PNG).unwrap();
        let prepared = assets
            .prepare_commit(
                &mut owner,
                &id,
                account.runtime.validation_permit().unwrap(),
            )
            .unwrap();
        (document, assets, prepared)
    };
    let ready = prepared.validate().unwrap();
    let mut owner = account.owner.lock().unwrap();
    let (receipt, _) = assets.publish(&mut owner, ready).unwrap();
    device::pin(&mut owner, &document.id, true).unwrap();
    device::position(&mut owner, &document.id, &document.revision, 3).unwrap();
    (device::document(&mut owner, &document.id).unwrap(), receipt)
}

pub(super) fn selected_roundtrip(account: &Account, markdown: &str) {
    let (document, asset) = local(account, markdown);
    let operation = Uuid::new_v4();
    let selection = selection(account, &document, operation);
    let before = graph_counts(account);
    let mut stale = selection.clone();
    stale.receipt.revision = if stale.receipt.revision == "f".repeat(64) {
        "e".repeat(64)
    } else {
        "f".repeat(64)
    };
    let rpc::AccountReply::Begun { job_id } = account
        .coordinator
        .execute(rpc::AccountRequest::SyncSelected { selection: stale })
        .unwrap()
    else {
        panic!("job");
    };
    assert!(matches!(
        account.settled(job_id).state,
        JobState::Failed {
            error: AccountError::Conflict
        }
    ));
    assert_eq!(
        graph_counts(account),
        before,
        "stale exact selected revision admitted ciphertext"
    );
    let rpc::AccountReply::Begun { job_id: id } = account
        .coordinator
        .execute(rpc::AccountRequest::SyncSelected {
            selection: selection.clone(),
        })
        .unwrap()
    else {
        panic!("job");
    };
    let job = account.settled(id);
    assert!(matches!(
        job.state,
        JobState::Completed {
            outcome: Outcome::Sync {
                committed: 1,
                conflicts: 0,
                received: 3,
                applied: 3,
                acknowledged: 3,
                pending: None,
                ..
            }
        }
    ));
    assert!(job.selected_sync.as_ref() == Some(&selection));
    let rpc::AccountReply::Begun { job_id } = account
        .coordinator
        .execute(rpc::AccountRequest::InspectSelected {
            selection: selection.clone(),
        })
        .unwrap()
    else {
        panic!("inspect");
    };
    assert!(
        matches!(account.settled(job_id).state, JobState::Completed { outcome: Outcome::SelectedStatus { selection: exact, purpose: SelectedPurpose::Content, disposition: crate::sync::outgoing::SelectedDisposition::Applied { sequence: 3 } } } if exact == selection)
    );
    let mut owner = account.owner.lock().unwrap();
    let current = device::document(&mut owner, &document.id).unwrap();
    assert_eq!(current.id, document.id);
    assert_eq!(current.source_id, document.source_id);
    assert_eq!(current.revision, document.revision);
    assert!(current.pinned);
    assert_eq!(current.line, 3);
    assert_eq!(
        device::read(&mut owner, &document.id, Some(&document.revision))
            .unwrap()
            .markdown,
        markdown
    );
    assert_eq!(
        device::list(&mut owner).unwrap().len(),
        2,
        "self-echo must not duplicate the explicitly imported native owner"
    );
    let assets = account
        .runtime
        .existing_assets(&mut owner, &account.assets)
        .unwrap()
        .unwrap();
    let manifest = assets
        .manifest(&mut owner, &document.id, &document.revision)
        .unwrap();
    assert_eq!(manifest.assets.len(), 1);
    assert_eq!(manifest.assets[0].asset_id, asset.asset_id);
    assert_eq!(manifest.assets[0].content_hash, asset.content_hash);
    let scope = account.scope();
    crate::sync::with_store(&mut owner, scope.clone(), |sync| {
        let entry = sync.entry(&operation.to_string())?.unwrap();
        assert!(entry.receipt.is_some());
        let dependencies = sync.dependencies(&operation.to_string())?;
        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.iter().all(|d| d.uploaded));
        Ok(())
    })
    .unwrap();
    owner.with_library_connection(|db| {
        let (native,state,sequence):(String,String,u64)=db.query_row("SELECT native_document,state,sequence FROM native_outgoing_intents_v1 WHERE operation_id=?1", [operation.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(native, document.id);
        assert_eq!(state, "applied");
        assert_eq!(sequence, 3);
    });
}

fn incoming(account: &Account) -> Arc<crate::sync::incoming::IncomingCoordinator> {
    account
        .coordinator
        .shared
        .view
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .incoming
        .clone()
}
fn graph_counts(account: &Account) -> (u64, u64, u64) {
    account.owner.lock().unwrap().with_library_connection(|db| {
        let outbox = db.query_row("SELECT count(*) FROM mdreader_sync_outbox", [], |r|r.get(0)).unwrap();
        let nonces = db.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r|r.get(0)).unwrap();
        let present:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='native_outgoing_intents_v1')",[],|r|r.get(0)).unwrap();
        let intents = if present {db.query_row("SELECT count(*) FROM native_outgoing_intents_v1", [], |r|r.get(0)).unwrap()} else {0};
        (outbox,nonces,intents)
    })
}
pub(super) fn cancel_retains_queue(account: &Account, origin: &str, markdown: &str) {
    let (document, _) = local(account, markdown);

    let coordinator = incoming(account);
    let before = graph_counts(account);
    let (entered, receive) = std::sync::mpsc::sync_channel(1);
    let (release, wait) = std::sync::mpsc::sync_channel(1);
    let wait = Mutex::new(wait);
    coordinator.set_materialization_gate(Some(Arc::new(move || {
        entered
            .send(())
            .map_err(|_| crate::sync::incoming::Error::Inactive)?;
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| crate::sync::incoming::Error::Inactive)?;
        Ok(())
    })));
    let pending = selection(account, &document, Uuid::new_v4());
    let id = account
        .coordinator
        .begin_selected_sync(pending.clone())
        .unwrap();
    receive
        .recv_timeout(Duration::from_secs(10))
        .expect("real queue materialization did not reach gate");
    assert!(
        account.owner.try_lock().is_ok(),
        "queue held SQLite while paused"
    );
    account.coordinator.cancel(id).unwrap();
    assert!(matches!(
        account.coordinator.begin_selected_sync(pending),
        Err(AccountError::Busy)
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    assert!(matches!(
        runtime.block_on(account.coordinator.freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_millis(20)
        )),
        Err(AccountError::Interrupted)
    ));
    assert_eq!(
        account.coordinator.shared.view.lock().unwrap().running,
        Some(id),
        "queue cancellation must retain the actual blocking JoinHandle"
    );
    assert_eq!(graph_counts(account), before);
    release.send(()).unwrap();
    assert!(matches!(account.settled(id).state, JobState::Cancelled));
    coordinator.set_materialization_gate(None);
    assert_eq!(
        graph_counts(account),
        before,
        "late cancellation must prevent graph/association admission"
    );

    // A later cancellation after durable admission preserves the exact graph.
    assert!(matches!(
        account.run(AccountIntent::SignIn {}),
        Outcome::Connected
    ));
    control(origin, "{\"stall\":true}");
    let operation = Uuid::new_v4();
    let id = account
        .coordinator
        .begin_selected_sync(selection(account, &document, operation))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if control(origin, "{}")["waiting"].as_bool() == Some(true) {
            break;
        }
        assert!(Instant::now() < until, "actual queued upload did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
    let admitted = graph_counts(account);
    assert_eq!(admitted.0, before.0 + 1);
    assert_eq!(admitted.2, before.2 + 1);
    let exact = crate::sync::with_store(&mut account.owner.lock().unwrap(), account.scope(), |s| {
        Ok(s.entry(&operation.to_string())?
            .unwrap()
            .envelope_utf8
            .unwrap())
    })
    .unwrap();
    account.coordinator.cancel(id).unwrap();
    assert!(matches!(account.settled(id).state, JobState::Cancelled));
    control(origin, "{\"release\":true}");
    assert_eq!(graph_counts(account), admitted);
    account.owner.lock().unwrap().with_library_connection(|db| {
        let body: Vec<u8> = db
            .query_row(
                "SELECT envelope FROM mdreader_sync_outbox WHERE operation_id=?1",
                [operation.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            body, exact,
            "cancel after admission must preserve exact ciphertext"
        );
        let state: String = db
            .query_row(
                "SELECT state FROM native_outgoing_intents_v1 WHERE operation_id=?1",
                [operation.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "queued");
    });
}
fn control(origin: &str, body: &str) -> serde_json::Value {
    serde_json::from_slice(&fixture::curl(
        &format!("{origin}/__synthetic/control"),
        Some(body),
        false,
    ))
    .unwrap()
}

fn selection(
    account: &Account,
    document: &DeviceDocument,
    operation_id: Uuid,
) -> SelectedOperation {
    SelectedOperation {
        scope: SelectedScope::current(&account.scope()).unwrap(),
        receipt: mdreader_core::device::catalog::receipt(document),
        operation_id,
    }
}
