//! Synthetic adapters prove registry/admission/storage composition only. They
//! do not establish installed renderer, daemon, SDK or hook writer quiescence.
use super::*;
use mdreader_profile::{Disposition, Exclusion, REQUIRED_PARTICIPANTS};
use rusqlite::Connection;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    thread,
    time::Instant,
};
use uuid::Uuid;

struct TestState {
    denied: Option<(Uuid, Uuid)>,
    db: Option<Connection>,
    replace_root_after_backup: bool,
    fail_freeze_after_root_replacement: bool,
}
struct TestParticipant {
    kind: Participant,
    root: RootBinding,
    state: Arc<Mutex<TestState>>,
    releases: Arc<AtomicUsize>,
}
impl LiveParticipant for TestParticipant {
    fn participant(&self) -> Participant {
        self.kind
    }
    fn root(&self) -> &RootBinding {
        &self.root
    }
    fn freeze(&self, scope: &FreezeScope<'_>) -> Result<Box<dyn FrozenParticipant>> {
        if scope.root() != &self.root || Instant::now() >= scope.deadline() {
            return Err(Error::NotQuiescent);
        }
        let mut state = self.state.lock().unwrap();
        if state.denied.is_some() {
            return Err(Error::Conflict);
        }
        if state.fail_freeze_after_root_replacement {
            let path = self.root.locator.restore()?;
            fs::rename(&path, path.with_file_name("retained-original")).unwrap();
            fs::create_dir(&path).unwrap();
            return Err(Error::Storage);
        }
        // Actual flush is deliberately AFTER the initial inventory. Native
        // preserve must capture its final stamps only after all these finish.
        if self.kind == Participant::RendererSession {
            fs::write(
                self.root.locator.restore()?.join("settings.json"),
                b"latest persisted state",
            )
            .map_err(|_| Error::Storage)?;
        }
        state.denied = Some((scope.operation(), scope.owner_epoch()));
        Ok(Box::new(TestFrozen {
            root: self.root.clone(),
            state: self.state.clone(),
            releases: self.releases.clone(),
        }))
    }
}
struct TestFrozen {
    root: RootBinding,
    state: Arc<Mutex<TestState>>,
    releases: Arc<AtomicUsize>,
}
impl FrozenParticipant for TestFrozen {
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> Result<()> {
        let state = self.state.lock().unwrap();
        if scope.root() != &self.root
            || state.denied != Some((scope.operation(), scope.owner_epoch()))
        {
            return Err(Error::NotQuiescent);
        }
        let metadata = fs::metadata(self.root.locator.restore()?).map_err(|_| Error::Conflict)?;
        if (metadata.dev(), metadata.ino()) != (self.root.device, self.root.inode) {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    fn with_database(
        &mut self,
        relative: &Path,
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()> {
        if relative != Path::new("library.sqlite") {
            return Err(Error::Invalid);
        }
        let state = self.state.lock().unwrap();
        action(state.db.as_ref().ok_or(Error::Invalid)?)?;
        if state.replace_root_after_backup {
            let path = self.root.locator.restore()?;
            fs::rename(&path, path.with_file_name("retained-original")).unwrap();
            fs::create_dir(&path).unwrap();
        }
        Ok(())
    }
}
impl Drop for TestFrozen {
    fn drop(&mut self) {
        self.state.lock().unwrap().denied = None;
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}
struct Fixture {
    temp: tempfile::TempDir,
    coordinator: NativeProfileTransition,
    choices: Vec<InventorySelection>,
    daemon: Arc<Mutex<TestState>>,
    late: Arc<Mutex<TestState>>,
    releases: Arc<AtomicUsize>,
}
impl Fixture {
    fn new(complete: bool) -> Self {
        let temp = tempfile::Builder::new()
            .tempdir_in(std::env::temp_dir().canonicalize().unwrap())
            .unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = temp.path().join("original");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(root.join("settings.json"), b"old state").unwrap();
        let db = Connection::open(root.join("library.sqlite")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE private_unknown(raw BLOB); INSERT INTO private_unknown VALUES(X'00FF');").unwrap();
        let choices = fs::read_dir(&root)
            .unwrap()
            .map(|entry| {
                let relative = PathBuf::from(entry.unwrap().file_name());
                let disposition = if relative == Path::new("library.sqlite") {
                    Disposition::Sqlite
                } else if relative == Path::new("settings.json") {
                    Disposition::Raw
                } else {
                    Disposition::Excluded(Exclusion::SqliteRecoverySidecar)
                };
                InventorySelection {
                    relative,
                    disposition,
                }
            })
            .collect::<Vec<_>>();
        let inventory = Inventory::capture(&root, &choices, &Limits::default()).unwrap();
        let mut coordinator = NativeProfileTransition::new(&inventory);
        let releases = Arc::new(AtomicUsize::new(0));
        let daemon = Arc::new(Mutex::new(TestState {
            denied: None,
            db: Some(db),
            replace_root_after_backup: false,
            fail_freeze_after_root_replacement: false,
        }));
        let late = Arc::new(Mutex::new(TestState {
            denied: None,
            db: None,
            replace_root_after_backup: false,
            fail_freeze_after_root_replacement: false,
        }));
        if complete {
            for kind in REQUIRED_PARTICIPANTS {
                let state = if kind == Participant::ReaderDaemon {
                    daemon.clone()
                } else if kind == Participant::LegacyCloudAndEnterprise {
                    late.clone()
                } else {
                    Arc::new(Mutex::new(TestState {
                        denied: None,
                        db: None,
                        replace_root_after_backup: false,
                        fail_freeze_after_root_replacement: false,
                    }))
                };
                let databases = if kind == Participant::ReaderDaemon {
                    vec![PathBuf::from("library.sqlite")]
                } else {
                    vec![]
                };
                coordinator
                    .register(
                        Arc::new(TestParticipant {
                            kind,
                            root: inventory.binding().clone(),
                            state,
                            releases: releases.clone(),
                        }),
                        databases,
                    )
                    .unwrap();
            }
        }
        Self {
            temp,
            coordinator,
            choices,
            daemon,
            late,
            releases,
        }
    }
}

#[test]
fn missing_participants_and_issued_effects_deny_snapshot_until_actual_completion() {
    let missing = Fixture::new(false);
    assert_eq!(
        missing.coordinator.missing_participants(),
        REQUIRED_PARTICIPANTS
    );
    assert!(matches!(
        missing.coordinator.preserve(
            &missing.choices,
            &missing.temp.path().join("absent"),
            &Limits::default(),
            Duration::from_secs(1)
        ),
        Err(Error::NotQuiescent)
    ));
    assert!(!missing.temp.path().join("absent").exists());
    let f = Fixture::new(true);
    let held = f.coordinator.effect(Participant::ReaderDaemon).unwrap();
    assert!(matches!(
        f.coordinator.preserve(
            &f.choices,
            &f.temp.path().join("timeout"),
            &Limits::default(),
            Duration::from_millis(2)
        ),
        Err(Error::NotQuiescent)
    ));
    assert!(!f.temp.path().join("timeout").exists());
    assert_eq!(f.releases.load(Ordering::SeqCst), 0);
    let coordinator = Arc::new(f.coordinator);
    let worker_owner = coordinator.clone();
    let choices = f.choices.clone();
    let destination = f.temp.path().join("archive");
    let worker = thread::spawn(move || {
        worker_owner
            .preserve(
                &choices,
                &destination,
                &Limits::default(),
                Duration::from_secs(1),
            )
            .map(|_| ())
    });
    let deadline = Instant::now() + Duration::from_millis(200);
    loop {
        match coordinator.effect(Participant::RendererSession) {
            Err(Error::NotQuiescent) => break,
            Ok(lease) => drop(lease),
            Err(error) => panic!("unexpected fixed error: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "freeze failed to deny new effects"
        );
        thread::yield_now();
    }
    assert_eq!(f.releases.load(Ordering::SeqCst), 0);
    drop(held); // Only actual worker completion permits the freeze to advance.
    worker.join().unwrap().unwrap();
    assert_eq!(f.releases.load(Ordering::SeqCst), 8);
    drop(coordinator.effect(Participant::ReaderDaemon).unwrap());
    assert!(matches!(coordinator.activate(), Err(Error::NotQuiescent)));
    assert!(matches!(coordinator.prepare(), Err(Error::NotQuiescent)));
    assert!(matches!(coordinator.rollback(), Err(Error::NotQuiescent)));
}

#[test]
fn all_real_capabilities_span_flush_inventory_and_backup_and_reject_root_replacement() {
    let f = Fixture::new(true);
    let snapshot = f
        .coordinator
        .preserve(
            &f.choices,
            &f.temp.path().join("archive"),
            &Limits::default(),
            Duration::from_secs(1),
        )
        .unwrap();
    let raw = snapshot
        .manifest()
        .entries
        .iter()
        .find(|entry| entry.disposition == Disposition::Raw)
        .unwrap();
    assert_eq!(
        fs::read(
            f.temp
                .path()
                .join("archive")
                .join(raw.member.as_ref().unwrap())
        )
        .unwrap(),
        b"latest persisted state"
    );
    assert_eq!(f.releases.load(Ordering::SeqCst), 8);
    let bad = Fixture::new(true);
    bad.daemon.lock().unwrap().replace_root_after_backup = true;
    assert!(matches!(
        bad.coordinator.preserve(
            &bad.choices,
            &bad.temp.path().join("not-published"),
            &Limits::default(),
            Duration::from_secs(1)
        ),
        Err(Error::Conflict)
    ));
    assert!(!bad.temp.path().join("not-published").exists());
    assert_eq!(bad.releases.load(Ordering::SeqCst), 8);
    assert!(matches!(
        bad.coordinator.effect(Participant::ReaderDaemon),
        Err(Error::NotQuiescent)
    ));
    assert!(bad
        .temp
        .path()
        .join("retained-original/library.sqlite")
        .exists());
    // A later acquisition fails before the callback. Earlier acquired owners
    // still require final proof before their release can resume admission.
    let partial = Fixture::new(true);
    partial
        .late
        .lock()
        .unwrap()
        .fail_freeze_after_root_replacement = true;
    assert!(matches!(
        partial.coordinator.preserve(
            &partial.choices,
            &partial.temp.path().join("partial"),
            &Limits::default(),
            Duration::from_secs(1)
        ),
        Err(Error::Conflict)
    ));
    assert!(!partial.temp.path().join("partial").exists());
    assert_eq!(partial.releases.load(Ordering::SeqCst), 4);
    assert!(matches!(
        partial.coordinator.effect(Participant::ReaderDaemon),
        Err(Error::NotQuiescent)
    ));
}
