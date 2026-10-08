use super::*;
use std::{fs, sync::mpsc};

fn setup() -> (tempfile::TempDir, Arc<Mutex<ReaderStore>>, Arc<Coordinator>) {
    let directory = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(ReaderStore::open(directory.path()).unwrap()));
    let coordinator = Arc::new(Coordinator::new(owner.clone(), directory.path()).unwrap());
    (directory, owner, coordinator)
}
#[test]
fn opened_owner_replacement_is_rejected_before_spool_admission() {
    let directory = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(ReaderStore::open(directory.path()).unwrap()));
    let named = directory.path().join("library.sqlite");
    fs::rename(&named, directory.path().join("original.sqlite")).unwrap();
    fs::File::create(&named).unwrap();
    assert!(matches!(
        Coordinator::new(owner, directory.path()),
        Err(Error::Conflict)
    ));
    assert!(!directory.path().join("captures").exists());

    // The same original-object proof remains live after Coordinator creation.
    let (directory, _, coordinator) = setup();
    let named = directory.path().join("library.sqlite");
    fs::rename(&named, directory.path().join("original.sqlite")).unwrap();
    fs::File::create(&named).unwrap();
    assert!(matches!(coordinator.enter(), Err(Error::Conflict)));
    assert!(matches!(coordinator.enter(), Err(Error::Unavailable)));
}
#[test]
fn actual_issued_effect_settles_before_freeze_and_timeout_resumes_without_proof() {
    let (directory, owner, coordinator) = setup();
    let effect = coordinator.enter().unwrap();
    assert!(matches!(
        coordinator.freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_millis(10)
        ),
        Err(Error::Deadline)
    ));
    // A bounded failure before quiescence did not cancel or drop the real worker.
    assert!(coordinator.enter().is_ok());
    let (sent, receive) = mpsc::channel();
    let (finish, finished) = mpsc::channel();
    let selected = coordinator.clone();
    let operation = Uuid::new_v4();
    let epoch = Uuid::new_v4();
    let worker = std::thread::spawn(move || {
        let guard = selected
            .freeze(operation, epoch, Instant::now() + Duration::from_secs(2))
            .unwrap();
        sent.send(()).unwrap();
        finished.recv().unwrap();
        guard
            .revalidate(&selected, &selected.owner, operation, epoch)
            .unwrap();
    });
    let until = Instant::now() + Duration::from_secs(1);
    while coordinator.enter().is_ok() {
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    // Freeze never retains the ReaderStore mutex while draining issued work.
    assert!(owner.try_lock().is_ok());
    fs::write(
        directory.path().join("accepted-effect"),
        b"durable result survives reply loss",
    )
    .unwrap();
    File::open(directory.path().join("accepted-effect"))
        .unwrap()
        .sync_all()
        .unwrap();
    File::open(directory.path()).unwrap().sync_all().unwrap();
    drop(effect);
    receive.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(coordinator.enter(), Err(Error::Busy)));
    finish.send(()).unwrap();
    worker.join().unwrap();
    assert!(coordinator.enter().is_ok());
    assert_eq!(
        fs::read(directory.path().join("accepted-effect")).unwrap(),
        b"durable result survives reply loss"
    );
}
#[test]
fn replaced_namespace_quarantines_and_retains_the_real_process_lock() {
    use fs2::FileExt;
    let (directory, owner, coordinator) = setup();
    let operation = Uuid::new_v4();
    let epoch = Uuid::new_v4();
    let guard = coordinator
        .freeze(operation, epoch, Instant::now() + Duration::from_secs(2))
        .unwrap();
    let other_directory = tempfile::tempdir().unwrap();
    let other = Arc::new(Mutex::new(
        ReaderStore::open(other_directory.path()).unwrap(),
    ));
    assert_eq!(
        guard.revalidate(&coordinator, &other, operation, epoch),
        Err(Error::Conflict)
    );
    assert_eq!(
        guard.revalidate(&coordinator, &owner, Uuid::new_v4(), epoch),
        Err(Error::Conflict)
    );
    let captures = directory.path().join("captures");
    fs::rename(captures.join("pending"), captures.join("old-pending")).unwrap();
    fs::create_dir(captures.join("pending")).unwrap();
    assert_eq!(
        guard.revalidate(&coordinator, &owner, operation, epoch),
        Err(Error::Conflict)
    );
    drop(guard);
    assert!(matches!(coordinator.enter(), Err(Error::Unavailable)));
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(captures.join("queue.lock"))
        .unwrap();
    assert!(
        lock.try_lock_exclusive().is_err(),
        "quarantine must retain OS exclusion, not only a service flag"
    );
    // Do not adopt a replacement made before freeze and omit the old namespace.
    let (other_profile, _, before_freeze) = setup();
    let captures = other_profile.path().join("captures");
    fs::rename(captures.join("pending"), captures.join("retained-pending")).unwrap();
    fs::create_dir(captures.join("pending")).unwrap();
    assert!(matches!(
        before_freeze.freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_secs(1)
        ),
        Err(Error::Conflict)
    ));
}
