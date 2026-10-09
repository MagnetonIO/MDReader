//! Synthetic storage and callback cancellation; no credentials or remote requests.
use super::*;
use mdreader_sync::authority::{
    AuthorityError, AuthorityResult, CustodyRecord, OidcClient, OidcEndpoints,
    ReviewedOidcRegistration, SyncVault, SyncVaultClient,
};
use std::{
    future::Future,
    net::TcpListener,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn reference() -> SyncCredentialReference {
    SyncCredentialReference::parse(&format!("native-sync-v1-{}", Uuid::new_v4())).unwrap()
}
fn roots(path: &std::path::Path) -> CacheRoots {
    CacheRoots {
        documents: path.join("documents"),
        assets: path.join("assets"),
    }
}

#[test]
fn retained_reference_reopens_without_regrant_and_rejects_future_schema() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("owner");
    let mut owner = ReaderStore::open(&data).unwrap();
    assert!(storage::load(&mut owner).unwrap().is_none());
    let library = Uuid::new_v4();
    let original = reference();
    storage::reserve(&mut owner, library, &original).unwrap();
    storage::reserve(&mut owner, library, &original).unwrap();
    assert!(matches!(
        storage::reserve(&mut owner, library, &reference()),
        Err(AccountError::Conflict)
    ));
    assert!(matches!(
        storage::reserve(&mut owner, Uuid::new_v4(), &original),
        Err(AccountError::Conflict)
    ));
    drop(owner);
    let mut owner = ReaderStore::open(&data).unwrap();
    let binding = storage::load(&mut owner).unwrap().unwrap();
    assert_eq!(binding.library, library);
    assert_eq!(binding.reference, original);
    let coordinator =
        Coordinator::new(Arc::new(Mutex::new(owner)), None, roots(temp.path())).unwrap();
    let status = serde_json::to_string(&coordinator.status().unwrap()).unwrap();
    assert!(coordinator.status().unwrap().retained_library);
    assert!(!coordinator.status().unwrap().custody_write_pending);
    assert!(!status.contains(original.opaque_reference()));
    assert_eq!(
        coordinator.status().unwrap().active_library_id,
        Some(library)
    );
    drop(coordinator);
    let mut owner = ReaderStore::open(&data).unwrap();
    owner
        .with_library_connection(|db| {
            db.execute("UPDATE native_sync_accounts_schema SET version=3", [])
        })
        .unwrap();
    assert!(matches!(
        storage::load(&mut owner),
        Err(AccountError::Schema)
    ));
    let retained: String = owner
        .with_library_connection(|db| {
            db.query_row(
                "SELECT credential_reference FROM native_sync_account_binding",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(retained, original.opaque_reference());
}

#[test]
fn exact_custody_retry_status_survives_interruption_cancellation_and_observation() {
    let shared = state::Shared::new(None, true, false);
    assert!(!shared.status().unwrap().custody_write_pending);
    let id = Uuid::new_v4();
    {
        let mut view = shared.view.lock().unwrap();
        view.custody_write_pending = true;
        view.start(id, None, None);
    }
    shared.invalidate(Some(id)).unwrap();
    let cancelled = shared.status().unwrap();
    assert!(cancelled.custody_write_pending);
    assert!(matches!(
        cancelled.latest_job.unwrap().state,
        JobState::Cancelled
    ));
    // A later job and status-only read do not erase the actor's exact record.
    let next = Uuid::new_v4();
    {
        let mut view = shared.view.lock().unwrap();
        view.running = None;
        view.start(next, None, None);
    }
    let command = actor::Command {
        id: next,
        epoch: shared.view.lock().unwrap().epoch,
        action: actor::Action::Account(AccountIntent::SignIn {}),
    };
    shared.finish(&command, Err(AccountError::Interrupted));
    assert!(shared.status().unwrap().custody_write_pending);
    shared.invalidate(None).unwrap();
    assert!(shared.status().unwrap().custody_write_pending);
    // A new actor starts without process-local pending private material.
    assert!(
        !state::Shared::new(None, true, false)
            .status()
            .unwrap()
            .custody_write_pending
    );
}

struct NoVault(Arc<AtomicUsize>);
impl SyncVault for NoVault {
    fn get(&self, _: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn create(&self, _: &CustodyRecord) -> AuthorityResult<()> {
        Err(AuthorityError::Custody)
    }
    fn compare_exchange(&self, _: u64, _: &CustodyRecord) -> AuthorityResult<()> {
        Err(AuthorityError::Custody)
    }
}
struct BlockingBrowser {
    started: mpsc::SyncSender<()>,
    dropped: Arc<AtomicBool>,
}
struct DropSignal(Arc<AtomicBool>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl NativeBrowser for BlockingBrowser {
    fn open<'a>(
        &'a self,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = AuthorityResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let _drop = DropSignal(self.dropped.clone());
            self.started
                .try_send(())
                .map_err(|_| AuthorityError::Interrupted)?;
            std::future::pending().await
        })
    }
}
fn settled(coordinator: &Coordinator, id: Uuid) -> Job {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        // Cancellation status is visible before the actor has dropped its future.
        if coordinator.shared.view.lock().unwrap().running.is_none() {
            return coordinator.operation(id).unwrap().unwrap();
        }
        assert!(Instant::now() < until, "account actor did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn explicit_status_inspects_retained_custody_without_login_and_caches_absence() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(&temp.path().join("owner")).unwrap();
    let library = Uuid::new_v4();
    let retained = reference();
    storage::reserve(&mut store, library, &retained).unwrap();
    let owner = Arc::new(Mutex::new(store));
    let origin = "https://synthetic-account.example.test";
    let registration = ReviewedOidcRegistration::new(
        origin,
        "synthetic-native",
        OidcEndpoints {
            authorization: format!("{origin}/authorize"),
            token: format!("{origin}/token"),
            jwks: format!("{origin}/jwks"),
        },
        origin,
        "https://mdreader.dev/sync",
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (started, browser_calls) = mpsc::sync_channel(1);
    let coordinator = Arc::new(
        Coordinator::new(
            owner.clone(),
            Some(NativeSetup {
                factory: NativeAuthorityFactory::new(
                    OidcClient::new(registration).unwrap(),
                    SyncVaultClient::new(Arc::new(NoVault(calls.clone()))).unwrap(),
                    SessionController::new(),
                ),
                transport: Transport::new(origin).unwrap(),
                browser: Arc::new(BlockingBrowser {
                    started,
                    dropped: Arc::new(AtomicBool::new(false)),
                }),
                registered_redirect: "http://127.0.0.1:43891/mdreader-sync/callback".into(),
                recovery: None,
            }),
            roots(temp.path()),
        )
        .unwrap(),
    );
    assert!(coordinator.status().unwrap().custody.is_none());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "construction/snapshot did vault IO"
    );
    // Explicit metadata inspection must not borrow the SQLite writer while the
    // native vault worker runs, and missing custody must never become new keys.
    let guard = owner.lock().unwrap();
    for _ in 0..2 {
        assert!(matches!(
            coordinator.execute(rpc::AccountRequest::Status {}),
            Err(AccountError::CustodyUnavailable)
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "known absence was reread");
    assert!(browser_calls.try_recv().is_err());
    assert!(coordinator.status().unwrap().latest_job.is_none());
    drop(guard);
    coordinator.disconnect().unwrap();
    assert!(matches!(
        coordinator.execute(rpc::AccountRequest::Status {}),
        Err(AccountError::CustodyUnavailable)
    ));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "new session did not reset inspection"
    );
    assert_eq!(
        storage::load(&mut owner.lock().unwrap())
            .unwrap()
            .unwrap()
            .reference,
        retained
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let frozen = runtime
        .block_on(coordinator.freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_secs(1),
        ))
        .unwrap();
    assert!(matches!(
        coordinator.execute(rpc::AccountRequest::Status {}),
        Err(AccountError::Busy)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    drop(frozen);
}
#[test]
fn cancellation_releases_callback_owner_and_fences_late_or_stale_jobs() {
    let temp = tempfile::tempdir().unwrap();
    let mut owner = ReaderStore::open(&temp.path().join("owner")).unwrap();
    storage::reserve(&mut owner, Uuid::new_v4(), &reference()).unwrap();
    let owner = Arc::new(Mutex::new(owner));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let origin = "https://synthetic-account.example.test";
    let registration = ReviewedOidcRegistration::new(
        origin,
        "synthetic-native",
        OidcEndpoints {
            authorization: format!("{origin}/authorize"),
            token: format!("{origin}/token"),
            jwks: format!("{origin}/jwks"),
        },
        origin,
        "https://mdreader.dev/sync",
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let (started, receive) = mpsc::sync_channel(1);
    let setup = NativeSetup {
        factory: NativeAuthorityFactory::new(
            OidcClient::new(registration).unwrap(),
            SyncVaultClient::new(Arc::new(NoVault(calls.clone()))).unwrap(),
            SessionController::new(),
        ),
        transport: Transport::new(origin).unwrap(),
        browser: Arc::new(BlockingBrowser {
            started,
            dropped: dropped.clone(),
        }),
        registered_redirect: format!("http://127.0.0.1:{port}/mdreader-sync/callback"),
        recovery: None,
    };
    let coordinator =
        Arc::new(Coordinator::new(owner.clone(), Some(setup), roots(temp.path())).unwrap());
    let first = coordinator.begin(AccountIntent::SignIn {}).unwrap();
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        owner.try_lock().is_ok(),
        "callback wait retained SQLite owner"
    );
    assert!(matches!(
        coordinator.begin(AccountIntent::SyncNow {}),
        Err(AccountError::Busy)
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let operation = Uuid::new_v4();
    let epoch = Uuid::new_v4();
    // Preservation cannot equate a timeout to logout or drained callback work.
    assert!(matches!(
        runtime.block_on(coordinator.freeze(
            operation,
            epoch,
            Instant::now() + Duration::from_millis(20)
        )),
        Err(AccountError::Interrupted)
    ));
    assert!(!dropped.load(Ordering::SeqCst));
    assert!(matches!(
        coordinator.operation(first).unwrap().unwrap().state,
        JobState::Running { .. }
    ));
    coordinator.cancel(first).unwrap();
    assert!(matches!(
        settled(&coordinator, first).state,
        JobState::Cancelled
    ));
    assert!(dropped.load(Ordering::SeqCst));
    let free = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
    drop(free);
    let frozen = runtime
        .block_on(coordinator.freeze(operation, epoch, Instant::now() + Duration::from_secs(1)))
        .unwrap();
    frozen.revalidate(&coordinator, operation, epoch).unwrap();
    assert!(matches!(
        frozen.revalidate(&coordinator, Uuid::new_v4(), epoch),
        Err(AccountError::Conflict)
    ));
    assert!(matches!(
        coordinator.begin(AccountIntent::SignIn {}),
        Err(AccountError::Busy)
    ));
    assert!(matches!(coordinator.disconnect(), Err(AccountError::Busy)));
    assert!(owner.try_lock().is_ok(), "freeze retained SQLite owner");
    drop(frozen);
    let second = coordinator.begin(AccountIntent::SignIn {}).unwrap();
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        coordinator.cancel(first),
        Err(AccountError::Conflict)
    ));
    assert!(matches!(
        coordinator.operation(second).unwrap().unwrap().state,
        JobState::Running { .. }
    ));
    coordinator.shared.finish(
        &actor::Command {
            id: first,
            epoch: 0,
            action: actor::Action::Account(AccountIntent::SignIn {}),
        },
        Ok(Outcome::Connected),
    );
    assert!(matches!(
        coordinator.operation(second).unwrap().unwrap().state,
        JobState::Running { .. }
    ));
    coordinator.cancel(second).unwrap();
    assert!(matches!(
        settled(&coordinator, second).state,
        JobState::Cancelled
    ));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "unverified login read custody"
    );
}

#[test]
fn unconfigured_local_owner_rejects_secret_inputs_and_bounds_jobs() {
    let temp = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(
        ReaderStore::open(&temp.path().join("owner")).unwrap(),
    ));
    let coordinator = Coordinator::new(owner.clone(), None, roots(temp.path())).unwrap();
    assert!(!coordinator.status().unwrap().configured);
    assert!(matches!(
        coordinator.begin(AccountIntent::SignIn {}),
        Err(AccountError::Unconfigured)
    ));
    assert!(owner.try_lock().is_ok());
    assert!(serde_json::from_str::<AccountIntent>(
        r#"{"action":"sign_in","access_token":"synthetic-secret"}"#
    )
    .is_err());
    assert!(matches!(
        AccountIntent::ApproveDevice {
            packet: "x".repeat(8193)
        }
        .validate(),
        Err(AccountError::Limit)
    ));
    let shared = state::Shared::new(None, false, false);
    for _ in 0..10 {
        let command = actor::Command {
            id: Uuid::new_v4(),
            epoch: 0,
            action: actor::Action::Account(AccountIntent::SignIn {}),
        };
        shared.view.lock().unwrap().start(command.id, None, None);
        shared.finish(&command, Err(AccountError::Unconfigured));
    }
    assert_eq!(shared.view.lock().unwrap().jobs.len(), 8);
}

#[test]
fn prior_account_binding_migrates_and_retained_switch_is_exact_bounded_and_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("retained-owner");
    let mut owner = ReaderStore::open(&data).unwrap();
    let original = reference();
    let old_library = Uuid::new_v4();
    owner
        .with_library_connection(|db| {
            db.execute_batch(include_str!("migrations/001-account-binding.sql"))?;
            db.execute("INSERT INTO native_sync_accounts_schema VALUES(1,1)", [])?;
            db.execute(
                "INSERT INTO native_sync_account_binding VALUES(1,?1,?2)",
                rusqlite::params![old_library.to_string(), original.opaque_reference()],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .unwrap();
    let old = storage::load(&mut owner).unwrap().unwrap();
    assert_eq!(old.reference, original);
    let existing_library = Uuid::new_v4();
    retained::observe(&mut owner, &old, existing_library).unwrap();
    let next = reference();
    assert!(matches!(
        retained::switch(&mut owner, &old, Uuid::new_v4(), &next),
        Err(AccountError::Conflict)
    ));
    assert_eq!(
        storage::load(&mut owner).unwrap().unwrap().reference,
        original
    );
    let current = retained::switch(&mut owner, &old, existing_library, &next).unwrap();
    assert!(matches!(
        retained::switch(&mut owner, &old, existing_library, &reference()),
        Err(AccountError::Conflict)
    ));
    drop(owner);
    let mut owner = ReaderStore::open(&data).unwrap();
    assert_eq!(storage::load(&mut owner).unwrap().unwrap().reference, next);
    assert_eq!(retained::hint(&mut owner, &current).unwrap(), None);
    let archived: String = owner
        .with_library_connection(|db| {
            db.query_row(
                "SELECT credential_reference FROM native_sync_retained_bindings WHERE ordinal=1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(archived, original.opaque_reference());
    // Reach the bound through real admissions; a ninth switch must roll back.
    let mut current = current;
    for _ in 1..8 {
        let target = Uuid::new_v4();
        retained::observe(&mut owner, &current, target).unwrap();
        current = retained::switch(&mut owner, &current, target, &reference()).unwrap();
    }
    let target = Uuid::new_v4();
    retained::observe(&mut owner, &current, target).unwrap();
    assert!(matches!(
        retained::switch(&mut owner, &current, target, &reference()),
        Err(AccountError::Limit)
    ));
    assert_eq!(
        storage::load(&mut owner).unwrap().unwrap().reference,
        current.reference
    );
    assert_eq!(retained::hint(&mut owner, &current).unwrap(), Some(target));
}
