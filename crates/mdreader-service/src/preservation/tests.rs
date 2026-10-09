//! Actual Runtime, SQLite owners and authenticated TCP control. Accounts are
//! explicitly unconfigured; this does not test identity/vault network custody.
use super::*;
use crate::{daemon::StartupOwner, runtime::Runtime, *};
use mdreader_core::ReaderStore;
use std::{
    net::Shutdown,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::Instant,
};

struct Server {
    path: std::path::PathBuf,
    runtime: Arc<Runtime>,
    _owner: Arc<Mutex<ReaderStore>>,
    stop: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
    // Cleanup follows all actual reader/capture/startup owners.
    _directory: tempfile::TempDir,
}
impl Server {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let path = directory.path().canonicalize().unwrap();
        #[cfg(windows)]
        let path = {
            let path = directory.path().join("private service");
            mdreader_capture::PrivateDirectory::create(&path).unwrap();
            path.canonicalize().unwrap()
        };
        let startup = StartupOwner::acquire(&path).unwrap();
        #[cfg(unix)]
        let store = ReaderStore::open(&path).unwrap();
        #[cfg(windows)]
        let store = ReaderStore::open_private(&path).unwrap();
        let store = Arc::new(Mutex::new(store));
        let runtime = Arc::new(
            Runtime::new_with_startup(
                store.clone(),
                path.join("models"),
                ServiceOptions {
                    disable_model: true,
                },
                NativeAdapters {
                    factory: None,
                    accounts: Some((
                        Ok(None),
                        accounts::CacheRoots {
                            documents: path.join("device-markdown-v1"),
                            assets: path.join("device-assets-v1"),
                        },
                    )),
                },
                Some(startup),
            )
            .unwrap(),
        );
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = Endpoint {
            protocol: PROTOCOL,
            port: listener.local_addr().unwrap().port(),
            token: "ab".repeat(32),
            pid: std::process::id(),
        };
        #[cfg(unix)]
        std::fs::write(path.join(ENDPOINT), encode(&endpoint).unwrap()).unwrap();
        #[cfg(windows)]
        {
            let private = mdreader_capture::PrivateDirectory::open(&path).unwrap();
            let mut file = private
                .create_database_file(std::path::Path::new(ENDPOINT))
                .unwrap();
            file.write_all(&encode(&endpoint).unwrap()).unwrap();
            file.sync_all().unwrap();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let serving = runtime.clone();
        let worker = thread::spawn(move || {
            let mut issued = Vec::new();
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let runtime = serving.clone();
                        let token = endpoint.token.clone();
                        issued.push(thread::spawn(move || {
                            let _ = ipc::connection(stream, &token, &runtime);
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("listener: {error}"),
                }
            }
            for worker in issued {
                worker.join().unwrap();
            }
        });
        Self {
            _directory: directory,
            path,
            runtime,
            _owner: store,
            stop,
            listener: Some(worker),
        }
    }
    fn client(&self) -> Client {
        Client::connect(self.path.as_path()).unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.runtime.stop_for_shutdown();
        self.stop.store(true, Ordering::Release);
        self.listener.take().unwrap().join().unwrap();
    }
}
fn until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !check() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
}
fn lose_reply(attempt: &Attempt, request: ControlRequest) {
    let envelope = Envelope {
        protocol: PROTOCOL,
        token: attempt.endpoint.token.to_string(),
        request: Request::Preservation { request },
    };
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, attempt.endpoint.port)).unwrap();
    let mut bytes = encode(&envelope).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).unwrap();
    stream.shutdown(Shutdown::Both).unwrap();
}
#[test]
fn real_tcp_uncertain_begin_and_release_reconcile_without_rebinding() {
    let server = Server::new();
    let client = server.client();
    let mut attempt = client
        .attempt(Uuid::new_v4(), Uuid::new_v4(), 10_000)
        .unwrap();
    lose_reply(
        &attempt,
        ControlRequest::Begin {
            scope: attempt.scope.clone(),
            duration_ms: attempt.duration_ms,
        },
    );
    until(|| {
        attempt
            .reconcile()
            .is_ok_and(|status| status.phase == Phase::Held)
    });
    let lease = attempt.lease.unwrap();
    assert!(crate::call(server.path.as_path(), &Request::ListSources).is_err());
    assert_eq!(attempt.begin().unwrap().lease, Some(lease));
    // Client destruction does not auto-resume. Recreate only an attempt on the
    // already-cached client/boot, with the exact same operation and owner epoch.
    let scope = attempt.scope.clone();
    drop(attempt);
    assert!(server.runtime.execute(Request::ListSources).is_err());
    let mut attempt = client
        .attempt(scope.operation, scope.owner_epoch, 10_000)
        .unwrap();
    assert_eq!(attempt.reconcile().unwrap().lease, Some(lease));
    lose_reply(
        &attempt,
        ControlRequest::Release {
            scope: scope.clone(),
            lease,
        },
    );
    until(|| {
        attempt
            .reconcile()
            .is_ok_and(|status| status.phase == Phase::Released)
    });
    assert_eq!(attempt.begin().unwrap().phase, Phase::Released);
    assert!(crate::call(server.path.as_path(), &Request::ListSources).is_ok());
}
#[test]
fn stale_boot_scope_and_endpoint_never_release_held_owner() {
    let server = Server::new();
    let client = server.client();
    let mut attempt = client
        .attempt(Uuid::new_v4(), Uuid::new_v4(), 10_000)
        .unwrap();
    assert_eq!(attempt.begin().unwrap().phase, Phase::Held);
    let mut scope = attempt.scope.clone();
    scope.boot = Uuid::new_v4();
    assert!(matches!(
        attempt.endpoint.call(ControlRequest::Reconcile { scope }),
        Err(Error::StaleOwner)
    ));
    let mut scope = attempt.scope.clone();
    scope.owner_epoch = Uuid::new_v4();
    assert!(matches!(
        attempt.endpoint.call(ControlRequest::Reconcile { scope }),
        Err(Error::StaleOwner)
    ));
    assert!(matches!(
        attempt.endpoint.call(ControlRequest::Release {
            scope: attempt.scope.clone(),
            lease: Uuid::new_v4()
        }),
        Err(Error::NotHeld)
    ));
    assert_eq!(attempt.revalidate().unwrap().phase, Phase::Held);
    let original = std::fs::read(server.path.as_path().join(ENDPOINT)).unwrap();
    std::fs::rename(
        server.path.as_path().join(ENDPOINT),
        server.path.as_path().join("old-endpoint"),
    )
    .unwrap();
    std::fs::write(server.path.as_path().join(ENDPOINT), original).unwrap();
    assert_eq!(attempt.release(), Err(Error::StaleOwner));
    assert!(server.runtime.execute(Request::ListSources).is_err());
}
#[test]
fn expired_real_session_retains_guards_and_denies_ordinary_work() {
    let server = Server::new();
    let client = server.client();
    let mut attempt = client.attempt(Uuid::new_v4(), Uuid::new_v4(), 500).unwrap();
    assert_eq!(attempt.begin().unwrap().phase, Phase::Held);
    until(|| {
        attempt
            .reconcile()
            .is_ok_and(|status| status.phase == Phase::Quarantined)
    });
    assert_eq!(attempt.release(), Err(Error::Quarantined));
    assert!(crate::call(server.path.as_path(), &Request::ListSources).is_err());
    assert_eq!(
        client
            .attempt(Uuid::new_v4(), Uuid::new_v4(), 500)
            .unwrap()
            .begin(),
        Err(Error::Busy)
    );
}

#[cfg(windows)]
#[test]
fn windows_shutdown_retains_real_service_custody_until_actual_owner_teardown() {
    let server = Server::new();
    let client = server.client();
    let private = mdreader_capture::PrivateDirectory::open(&server.path).unwrap();
    let lock = private
        .open_lock(std::path::Path::new("reader.lock"))
        .unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&lock).is_err());
    let mut attempt = client
        .attempt(Uuid::new_v4(), Uuid::new_v4(), 10_000)
        .unwrap();
    assert_eq!(attempt.begin().unwrap().phase, Phase::Held);
    attempt.shutdown().unwrap();
    assert!(server.runtime.shutdown_requested());
    assert_eq!(attempt.release(), Err(Error::NotHeld));
    assert!(server.runtime.execute(Request::ListSources).is_err());
    // A shutdown acknowledgement is not closure: the real writer is still held.
    assert!(fs2::FileExt::try_lock_exclusive(&lock).is_err());
}

#[cfg(windows)]
#[path = "windows_backup_tests.rs"]
mod windows_backup;
