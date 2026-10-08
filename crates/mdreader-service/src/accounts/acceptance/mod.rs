//! One real local HTTPS/PKCE/Worker/SQLite/R2 vertical case, synthetic identity
//! and public fixture material only. Explicitly scheduled; never live accounts.
use super::*;
use mdreader_core::device;
use mdreader_sync::{
    authority::{
        AuthorityError, AuthorityResult, OidcClient, OidcEndpoints, RecoveryExport,
        ReviewedOidcRegistration, SyncKeyringVault, SyncVaultClient,
    },
    CursorState, SyncScope,
};
use std::{
    future::Future,
    net::TcpListener,
    pin::Pin,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
mod fixture;
mod outgoing;
mod vault;
struct Recovery(Vec<u8>);
impl NativeRecovery for Recovery {
    fn import<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<RecoveryInput>> + Send + 'a>> {
        // Published synthetic test secret: 32 bytes of 91, base64url, not native keys.
        Box::pin(async move {
            Ok(RecoveryInput {
                file: self.0.clone(),
                secret: Zeroizing::new("W1tbW1tbW1tbW1tbW1tbW1tbW1tbW1tbW1tbW1tbW1s".into()),
            })
        })
    }
    fn export<'a>(
        &'a self,
        _: RecoveryExport,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async { Err(AccountError::Unsupported) })
    }
}
struct Account {
    coordinator: Arc<Coordinator>,
    owner: Arc<Mutex<ReaderStore>>,
    runtime: AssetRuntime,
    assets: PathBuf,
    documents: PathBuf,
    _temp: tempfile::TempDir,
}
impl Account {
    fn new(origin: &str, file: &str, library: Uuid) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let owner = Arc::new(Mutex::new(
            ReaderStore::open(&temp.path().join("owner")).unwrap(),
        ));
        let registration = ReviewedOidcRegistration::new(
            "https://native-synthetic-issuer.example.test",
            "native-synthetic",
            OidcEndpoints {
                authorization: format!("{origin}/__synthetic/oauth/authorize"),
                token: format!("{origin}/__synthetic/oauth/token"),
                jwks: format!("{origin}/__synthetic/keys"),
            },
            origin,
            "https://mdreader.dev/sync",
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let setup = NativeSetup {
            factory: NativeAuthorityFactory::new(
                OidcClient::new(registration).unwrap(),
                SyncVaultClient::new(Arc::new(
                    SyncKeyringVault::new("mdreader.synthetic.actor").unwrap(),
                ))
                .unwrap(),
                SessionController::new(),
            ),
            transport: Transport::new(origin).unwrap(),
            browser: Arc::new(fixture::Browser),
            registered_redirect: format!("http://127.0.0.1:{port}/mdreader-sync/callback"),
            recovery: Some(Arc::new(Recovery(file.as_bytes().to_vec()))),
        };
        let runtime = AssetRuntime::default();
        let assets = temp.path().join("assets");
        let documents = temp.path().join("documents");
        let coordinator = Arc::new(
            Coordinator::with_asset_runtime(
                owner.clone(),
                Some(setup),
                CacheRoots {
                    documents: documents.clone(),
                    assets: assets.clone(),
                },
                runtime.clone(),
            )
            .unwrap(),
        );
        let account = Self {
            coordinator,
            owner,
            runtime,
            assets,
            documents,
            _temp: temp,
        };
        assert!(matches!(
            account.run(AccountIntent::Recover {
                remote_library: library
            }),
            Outcome::Connected
        ));
        account
    }
    fn settled(&self, id: Uuid) -> Job {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if self
                .coordinator
                .shared
                .view
                .lock()
                .unwrap()
                .running
                .is_none()
            {
                return self.coordinator.operation(id).unwrap().unwrap();
            }
            assert!(Instant::now() < until, "real account actor did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn run(&self, intent: AccountIntent) -> Outcome {
        let job = self.settled(self.coordinator.begin(intent).unwrap());
        match job.state {
            JobState::Completed { outcome } => outcome,
            JobState::Failed { error } => panic!("actor failed: {error:?}"),
            _ => panic!("actor did not complete"),
        }
    }
    fn scope(&self) -> SyncScope {
        self.coordinator
            .shared
            .view
            .lock()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .authority
            .scope()
            .unwrap()
    }
    fn cursor(&self, scope: &SyncScope) -> CursorState {
        crate::sync::with_store(&mut self.owner.lock().unwrap(), scope.clone(), |s| {
            s.cursors()
        })
        .unwrap()
    }
    fn sync(&self, count: u64) {
        assert!(
            matches!(self.run(AccountIntent::SyncNow {}), Outcome::Sync { received, applied, acknowledged, pending: None, .. } if received == count && applied == count && acknowledged == count)
        );
    }
}
#[test]
#[ignore = "requires explicit current local Worker bundle and Node; synthetic TLS child only"]
fn actual_actor_pkce_recovery_pump_and_retained_projection_cancel() {
    let Ok(origin) = std::env::var("MDREADER_ACTOR_CHILD_ORIGIN") else {
        fixture::Fixture::child_test();
        return;
    };
    vault::install();
    let seed = fixture::peer(&origin, "seed");
    let library = Uuid::parse_str(seed["libraryId"].as_str().unwrap()).unwrap();
    let first = Account::new(&origin, seed["recoveryFile"].as_str().unwrap(), library);
    let scope = first.scope();
    first.sync(1);
    let doc = device::list(&mut first.owner.lock().unwrap())
        .unwrap()
        .remove(0);
    assert_eq!(
        device::read(
            &mut first.owner.lock().unwrap(),
            &doc.id,
            Some(&doc.revision)
        )
        .unwrap()
        .markdown,
        seed["sourceUtf8"].as_str().unwrap()
    );
    let mut owner = first.owner.lock().unwrap();
    let assets = first.runtime.assets(&mut owner, &first.assets).unwrap();
    let manifest = assets.manifest(&mut owner, &doc.id, &doc.revision).unwrap();
    assert_eq!(manifest.assets.len(), 1);
    assert_eq!(
        manifest.assets[0].content_hash,
        seed["assetSha256"].as_str().unwrap()
    );
    drop(owner);
    fixture::peer(&origin, "pin");
    first.sync(2);
    let updated = device::document(&mut first.owner.lock().unwrap(), &doc.id).unwrap();
    assert!(updated.pinned);
    assert_eq!(updated.line, 2);
    outgoing::selected_roundtrip(&first, seed["sourceUtf8"].as_str().unwrap());

    // Second real recovery device, same account/library, independent local owner.
    let second = Account::new(&origin, seed["recoveryFile"].as_str().unwrap(), library);
    let second_scope = second.scope();
    let incoming = second
        .coordinator
        .shared
        .view
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .incoming
        .clone();
    let (entered, receive) = std::sync::mpsc::sync_channel(1);
    let (release, wait) = std::sync::mpsc::sync_channel(1);
    let wait = Mutex::new(wait);
    incoming.set_materialization_gate(Some(Arc::new(move || {
        entered
            .send(())
            .map_err(|_| crate::sync::incoming::Error::Inactive)?;
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| crate::sync::incoming::Error::Inactive)?;
        Ok(())
    })));
    let id = second.coordinator.begin(AccountIntent::SyncNow {}).unwrap();
    receive
        .recv_timeout(Duration::from_secs(10))
        .expect("real materialization did not reach gate");
    assert!(
        second.owner.try_lock().is_ok(),
        "materialization retained SQLite lock"
    );
    second.coordinator.cancel(id).unwrap();
    assert!(matches!(
        second.coordinator.operation(id).unwrap().unwrap().state,
        JobState::Cancelled
    ));
    assert!(matches!(
        second.coordinator.begin(AccountIntent::SignIn {}),
        Err(AccountError::Busy)
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    assert!(matches!(
        runtime.block_on(second.coordinator.freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_millis(20)
        )),
        Err(AccountError::Interrupted)
    ));
    assert_eq!(
        second.coordinator.shared.view.lock().unwrap().running,
        Some(id),
        "cancel must retain real blocking JoinHandle"
    );
    release.send(()).unwrap();
    assert!(matches!(second.settled(id).state, JobState::Cancelled));
    incoming.set_materialization_gate(None);
    assert!(device::list(&mut second.owner.lock().unwrap())
        .unwrap()
        .is_empty());
    let cursor = second.cursor(&second_scope);
    assert_eq!(cursor.received, 3);
    assert_eq!(cursor.applied, 0);
    assert_eq!(cursor.server_acknowledged, 0);
    assert!(matches!(
        second.run(AccountIntent::SignIn {}),
        Outcome::Connected
    ));
    assert!(matches!(
        second.run(AccountIntent::SyncNow {}),
        Outcome::Sync {
            applied: 0,
            acknowledged: 0,
            pending: Some(SyncPending {
                reason: crate::sync::incoming::PendingReason::ReauthorizationRequired,
                ..
            }),
            ..
        }
    ));
    outgoing::cancel_retains_queue(&second, &origin, seed["sourceUtf8"].as_str().unwrap());
    fixture::peer(&origin, "delete");
    first.sync(4);
    assert!(device::read(&mut first.owner.lock().unwrap(), &doc.id, None).is_err());
    assert_eq!(first.cursor(&scope).applied, 4);
    drop(first);
    drop(second);
}
