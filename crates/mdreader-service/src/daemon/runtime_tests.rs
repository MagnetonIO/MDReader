//! Actual Runtime/SQLite/process-lease fixtures; only the native model boundary
//! is synthetic. No private profiles, model downloads or all-owner certificate.
use super::*;
use crate::daemon::{Coordinator, Error, StartupOwner};
use std::time::Instant;
use uuid::Uuid;

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "bounded owner progress");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn runtime(directory: &std::path::Path, model: Option<ModelActor>) -> Runtime {
    let startup = StartupOwner::acquire(directory).unwrap();
    let owner = Arc::new(Mutex::new(ReaderStore::open(directory).unwrap()));
    Runtime::with_owned_model(owner, directory.join("models"), model, Some(startup)).unwrap()
}
fn freezing(daemon: Arc<Coordinator>) -> mpsc::Receiver<crate::daemon::FrozenReaderDaemon> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        send.send(
            daemon
                .freeze(
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    Instant::now() + Duration::from_secs(3),
                )
                .unwrap(),
        )
        .unwrap();
    });
    receive
}
#[test]
fn runtime_denies_real_dispatch_and_drains_descendants_without_owner_mutex() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path(), None);
    let daemon = runtime.daemon.as_ref().unwrap().clone();
    let issued = daemon.enter().unwrap();
    let receive = freezing(daemon.clone());
    until(|| matches!(daemon.enter(), Err(Error::Busy)));
    // An already-issued command can transfer responsibility after denial.
    let descendant = issued.fork().unwrap();
    drop(issued);
    assert!(receive.try_recv().is_err());
    until(|| runtime.store.try_lock().is_ok() && runtime.queries.try_lock().is_ok());
    assert!(runtime.execute(Request::ListSources).is_err());
    assert!(runtime
        .execute(Request::SetBookmark {
            source_id: "unadmitted".into(),
            path: "note.md".into(),
            line: 9,
        })
        .is_err());
    assert!(runtime.execute(Request::Ping).is_ok());
    drop(descendant);
    let frozen = receive.recv_timeout(Duration::from_secs(3)).unwrap();
    frozen
        .with_library(|connection| {
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM sources", [], |row| row
                        .get::<_, u64>(0))
                    .unwrap(),
                0
            );
            assert!(
                connection
                    .execute("CREATE TABLE forbidden(value INTEGER)", [])
                    .is_err(),
                "snapshot callback must borrow the real readonly connection"
            );
            Ok(())
        })
        .unwrap();
    drop(frozen);
    assert!(runtime.execute(Request::ListSources).is_ok());
    let standalone = tempfile::tempdir().unwrap();
    let standalone_owner = Arc::new(Mutex::new(ReaderStore::open(standalone.path()).unwrap()));
    let standalone =
        Runtime::with_model(standalone_owner, standalone.path().join("models"), None).unwrap();
    assert!(matches!(standalone.daemon, Err(Error::Unsupported)));
}
struct Engine {
    release: Option<mpsc::Receiver<()>>,
    entered: mpsc::Sender<()>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
impl model::Engine for Engine {
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(release) = self.release.take() {
            self.entered.send(()).unwrap();
            release.recv().unwrap();
        }
        Ok(texts.iter().map(|_| vec![1.0; 384]).collect())
    }
}
#[test]
fn timed_out_factory_remains_issued_and_resumed_scheduler_rejects_buffered_vectors() {
    let directory = tempfile::tempdir().unwrap();
    let (release, blocked) = mpsc::channel();
    let (entered, observed) = mpsc::channel();
    let model = ModelActor::with_factory(Duration::from_millis(5), move || {
        entered.send(()).unwrap();
        blocked.recv().unwrap();
        Err(ReaderError::ModelUnavailable)
    });
    let runtime = runtime(directory.path(), Some(model));
    let daemon = runtime.daemon.as_ref().unwrap().clone();
    let issued = daemon.enter().unwrap();
    runtime
        .model
        .as_ref()
        .unwrap()
        .ensure_loaded_owned(Some(&issued));
    drop(issued);
    observed.recv_timeout(Duration::from_secs(3)).unwrap();
    until(|| runtime.model.as_ref().unwrap().ensure_loaded() == ModelState::RestartRequired);
    assert!(matches!(
        daemon.freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_millis(20)
        ),
        Err(Error::Deadline)
    ));
    // Abandoning Runtime/tickets must not release its process lease while
    // the actual native factory can still write cache files.
    drop(daemon);
    drop(runtime);
    assert!(matches!(
        StartupOwner::acquire(directory.path()),
        Err(Error::Busy)
    ));
    release.send(()).unwrap();
    until(|| StartupOwner::acquire(directory.path()).is_ok());

    let directory = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    std::fs::write(
        notes.path().join("note.md"),
        "# Buffered\nExact scoped evidence",
    )
    .unwrap();
    let startup = StartupOwner::acquire(directory.path()).unwrap();
    let mut owner = ReaderStore::open(directory.path()).unwrap();
    let source = owner
        .add_local_source(
            "Synthetic",
            mdreader_core::SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap()
        .id;
    owner.queue_index(&source).unwrap();
    let (release, blocked) = mpsc::channel();
    let (entered, observed) = mpsc::channel();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recorded = calls.clone();
    let model = ModelActor::with_factory(Duration::from_secs(1), move || {
        Ok(Box::new(Engine {
            release: Some(blocked),
            entered,
            calls: recorded,
        }))
    });
    let runtime = Runtime::with_owned_model(
        Arc::new(Mutex::new(owner)),
        directory.path().join("models"),
        Some(model),
        Some(startup),
    )
    .unwrap();
    observed.recv_timeout(Duration::from_secs(3)).unwrap();
    let daemon = runtime.daemon.as_ref().unwrap().clone();
    let receive = freezing(daemon.clone());
    until(|| matches!(daemon.enter(), Err(Error::Busy)));
    release.send(()).unwrap();
    let frozen = receive.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        runtime
            .store
            .lock()
            .unwrap()
            .index_status(std::slice::from_ref(&source))
            .unwrap()
            .embedded,
        0
    );
    assert!(!runtime
        .store
        .lock()
        .unwrap()
        .pending_index_jobs()
        .unwrap()
        .is_empty());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(frozen);
    until(|| {
        runtime
            .store
            .lock()
            .unwrap()
            .pending_index_jobs()
            .unwrap()
            .is_empty()
    });
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the old buffered result must not install after the resume epoch changes"
    );
    assert_eq!(
        runtime
            .store
            .lock()
            .unwrap()
            .index_status(&[source])
            .unwrap()
            .embedded,
        1
    );
}
#[test]
fn frozen_owner_quarantines_index_or_process_lease_replacement() {
    for name in ["index.sqlite", "reader.lock"] {
        let directory = tempfile::tempdir().unwrap();
        let runtime = runtime(directory.path(), None);
        let daemon = runtime.daemon.as_ref().unwrap().clone();
        let operation = Uuid::new_v4();
        let epoch = Uuid::new_v4();
        let frozen = daemon
            .freeze(operation, epoch, Instant::now() + Duration::from_secs(3))
            .unwrap();
        let named = directory.path().join(name);
        std::fs::rename(&named, directory.path().join(format!("original-{name}"))).unwrap();
        std::fs::File::create(&named).unwrap();
        assert_eq!(
            frozen.revalidate(&daemon, operation, epoch),
            Err(Error::Conflict)
        );
        drop(frozen);
        assert!(runtime.execute(Request::ListSources).is_err());
    }
}
