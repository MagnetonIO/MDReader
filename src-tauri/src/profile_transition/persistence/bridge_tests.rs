//! Real JSON owner + actual registry/storage orchestration. The other six
//! synthetic participants prove no installed SDK/service/vault quiescence.
use super::{bridge::*, *};
use crate::profile_transition::{
    FreezeScope, FrozenParticipant, LiveParticipant, NativeProfileTransition,
};
use mdreader_profile::{
    Disposition, Exclusion, Inventory, InventorySelection, Limits, Participant, RootBinding,
    RootLocator, REQUIRED_PARTICIPANTS,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex, OnceLock, Weak,
    },
    time::Duration,
};

struct Channel {
    sender: Mutex<mpsc::Sender<FlushRequest>>,
    released: AtomicUsize,
    transition: OnceLock<Weak<NativeProfileTransition>>,
}
impl FlushChannel for Channel {
    fn request(&self, _: &str, request: &FlushRequest) -> mdreader_profile::Result<()> {
        self.sender.lock().unwrap().send(request.clone()).unwrap();
        Ok(())
    }
    fn released(&self, _: &str, _: &FlushRequest) -> mdreader_profile::Result<()> {
        let transition = self.transition.get().unwrap().upgrade().unwrap();
        let _lease = transition
            .effect(Participant::RendererSession)
            .expect("resumption notice must follow real global admission");
        self.released.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct Other {
    root: RootBinding,
    kind: Participant,
    invalidate: Arc<AtomicBool>,
}
impl LiveParticipant for Other {
    fn participant(&self) -> Participant {
        self.kind
    }
    fn root(&self) -> &RootBinding {
        &self.root
    }
    fn freeze(&self, _: &FreezeScope<'_>) -> mdreader_profile::Result<Box<dyn FrozenParticipant>> {
        if self.kind == Participant::UnsavedEditor && self.invalidate.load(Ordering::SeqCst) {
            fs::write(
                self.root.locator.restore()?.join("settings.json"),
                b"{\"settings\":{\"theme\":\"dark\"}}",
            )
            .unwrap();
        }
        Ok(Box::new(Other {
            root: self.root.clone(),
            kind: self.kind,
            invalidate: Arc::clone(&self.invalidate),
        }))
    }
}
impl FrozenParticipant for Other {
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> mdreader_profile::Result<()> {
        if scope.root() != &self.root {
            return Err(mdreader_profile::Error::Conflict);
        }
        Ok(())
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> mdreader_profile::Result<()>,
    ) -> mdreader_profile::Result<()> {
        Err(mdreader_profile::Error::Invalid)
    }
}

#[test]
fn scoped_renderer_flush_archives_exact_originals_and_releases_only_after_bundle() {
    let temp = tempfile::Builder::new()
        .tempdir_in(std::env::temp_dir().canonicalize().unwrap())
        .unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let profile = temp.path().join("profile");
    fs::create_dir(&profile).unwrap();
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o700)).unwrap();
    let original = b" {\"session\":{\"version\":1,\"removed_original\":true}} \n";
    fs::write(profile.join("session.json"), original).unwrap();
    fs::write(
        profile.join("settings.json"),
        b"{\"settings\":{\"theme\":\"dark\"}}",
    )
    .unwrap();
    let owner = Arc::new(JsonOwner::new(SourceRoot::capture(profile.clone()).unwrap()).unwrap());
    let identity = owner.load("main").unwrap().identity;
    let metadata = fs::metadata(&profile).unwrap();
    let root = RootBinding {
        locator: RootLocator::capture(&profile).unwrap(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    let choices = vec![
        InventorySelection {
            relative: "session.json".into(),
            disposition: Disposition::Raw,
        },
        InventorySelection {
            relative: "settings.json".into(),
            disposition: Disposition::Raw,
        },
        InventorySelection {
            relative: ".mdreader-json-owner.lock".into(),
            disposition: Disposition::Excluded(Exclusion::RuntimeEndpointOrLock),
        },
    ];
    let limits = Limits::default();
    let inventory = Inventory::capture(&profile, &choices, &limits).unwrap();
    let mut transition = NativeProfileTransition::new(&inventory);
    let (sender, receiver) = mpsc::channel();
    let channel = Arc::new(Channel {
        sender: Mutex::new(sender),
        released: AtomicUsize::new(0),
        transition: OnceLock::new(),
    });
    let bridge = FlushBridge::new(Arc::clone(&owner), root.clone(), channel.clone()).unwrap();
    for participant in bridge.participants() {
        transition.register(participant, Vec::new()).unwrap();
    }
    let invalidate = Arc::new(AtomicBool::new(false));
    for kind in REQUIRED_PARTICIPANTS.into_iter().filter(|kind| {
        !matches!(
            kind,
            Participant::RendererSession | Participant::RendererSettings
        )
    }) {
        transition
            .register(
                Arc::new(Other {
                    root: root.clone(),
                    kind,
                    invalidate: Arc::clone(&invalidate),
                }),
                Vec::new(),
            )
            .unwrap();
    }
    let destination = temp.path().join("preservation");
    let transition = Arc::new(transition);
    assert!(channel.transition.set(Arc::downgrade(&transition)).is_ok());
    let runtime = Arc::clone(&transition);
    let selected = choices.clone();
    let bounds = limits.clone();
    let target = destination.clone();
    let worker = std::thread::spawn(move || {
        runtime.preserve_bundle(&selected, &target, &bounds, Duration::from_secs(2))
    });
    let request = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(request.identity.owner_epoch, identity.owner_epoch);
    assert_eq!(channel.released.load(Ordering::SeqCst), 0);
    let submission = |request: FlushRequest| FlushSubmission {
        request,
        session_generation: 1,
        settings_generation: 1,
        session_json: r#"{"version":1,"root":null,"activeTabId":null,"tabs":[],"recent":[]}"#
            .into(),
        settings_json: r#"{"theme":"light"}"#.into(),
    };
    let mut forged = request.clone();
    forged.request_id = Uuid::new_v4().to_string();
    assert!(matches!(
        bridge.submit("main", submission(forged)),
        Err(PersistenceError::Conflict)
    ));
    assert_eq!(fs::read(profile.join("session.json")).unwrap(), original);
    bridge.submit("main", submission(request)).unwrap();
    let bundle = worker.join().unwrap().unwrap();
    assert_eq!(
        fs::read(destination.join("session.preflush")).unwrap(),
        original
    );
    assert_eq!(channel.released.load(Ordering::SeqCst), 1);
    bundle.revalidate(&Limits::default()).unwrap();
    owner
        .write(
            "main",
            &identity,
            JsonKind::Settings,
            2,
            r#"{"theme":"dark"}"#,
        )
        .unwrap();
    // A later participant invalidates the actual persisted JSON receipt. Final
    // validation quarantines admission and never emits a second release notice.
    invalidate.store(true, Ordering::SeqCst);
    let rejected = temp.path().join("rejected");
    let target = rejected.clone();
    let runtime = Arc::clone(&transition);
    let worker = std::thread::spawn(move || {
        runtime.preserve_bundle(&choices, &target, &limits, Duration::from_secs(2))
    });
    let request = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut latest = submission(request);
    latest.session_generation = 2;
    latest.settings_generation = 3;
    bridge.submit("main", latest).unwrap();
    // The first bad receipt quarantines the shared capability. The final pass
    // reports the resulting non-quiescent owner rather than resuming writers.
    let outcome = worker.join().unwrap().err();
    assert!(
        matches!(outcome, Some(mdreader_profile::Error::NotQuiescent)),
        "{outcome:?}"
    );
    assert!(!rejected.exists());
    assert_eq!(channel.released.load(Ordering::SeqCst), 1);
    assert!(transition.effect(Participant::RendererSession).is_err());
}
