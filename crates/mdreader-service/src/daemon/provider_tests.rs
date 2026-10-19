//! Real coordinator command, mutation journal and daemon lease. The provider
//! factory returns a typed timeout; no credential, network or provider write.
use super::*;
use crate::daemon::{Admission, Error as DrainError, StartupOwner};
use mdreader_core::ReaderStore;
use std::sync::atomic::{AtomicBool, Ordering};

struct CleanupOwner {
    store: Arc<Mutex<ReaderStore>>,
    cleanup: Arc<AtomicBool>,
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Owner for CleanupOwner {
    fn with_connection<T>(&self, action: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        if self.cleanup.swap(false, Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        self.store.with_connection(action)
    }
}
struct TimeoutFactory(Arc<AtomicBool>);
#[async_trait::async_trait]
impl ProviderFactory for TimeoutFactory {
    async fn describe(
        &self,
        _: LibraryId,
        _: SourceId,
        _: DisplayText,
        _: CredentialReference,
        _: &IoContext,
    ) -> Result<SourceConfiguration> {
        Err(ProviderError::Unsupported)
    }
    async fn open(
        &self,
        _: SourceConfiguration,
        _: Arc<dyn ProviderAuthority>,
        _: Vec<Metadata>,
        _: &IoContext,
    ) -> Result<Arc<dyn DocumentProvider>> {
        self.0.store(true, Ordering::SeqCst);
        Err(ProviderError::Timeout)
    }
}
#[test]
fn accepted_provider_command_keeps_lease_through_lost_reply_and_unknown_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let startup = StartupOwner::acquire(directory.path()).unwrap();
    let store = Arc::new(Mutex::new(ReaderStore::open(directory.path()).unwrap()));
    let queries = Arc::new(Mutex::new(
        ReaderStore::open_readonly(directory.path()).unwrap(),
    ));
    let admission = Admission::new(Some(startup.clone()));
    let daemon =
        crate::daemon::Coordinator::new(admission, startup, store.clone(), queries).unwrap();
    let descriptor = store
        .with_connection(|connection| {
            let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
            let descriptor = SourceDescriptor::new(
                SourceBinding::new(
                    library_id(connection)?,
                    SourceId::new("source")?,
                    AccountBinding::new("synthetic-account")?,
                    Uuid::new_v4(),
                    scope.digest(),
                )?,
                ProviderKind::GoogleDrive,
                DisplayText::new("Synthetic")?,
                scope,
                ProviderCapabilities {
                    read: true,
                    list: true,
                    changes: true,
                    guarded_write: true,
                    assets: false,
                },
                SourceHealth::Ready,
                Coverage::NeverIndexed,
            )?;
            admit(
                connection,
                &descriptor,
                &OpaqueValue::new("synthetic-custody-reference")?,
                [7; 32],
            )?;
            Ok(descriptor)
        })
        .unwrap();
    let (entered, observed) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let cleanup = Arc::new(AtomicBool::new(false));
    let owner = Arc::new(CleanupOwner {
        store: store.clone(),
        cleanup: cleanup.clone(),
        entered,
        release: Mutex::new(blocked),
    });
    let coordinator = Coordinator::new(
        owner,
        Arc::new(UnsupportedProjection),
        Arc::new(TimeoutFactory(cleanup)),
    )
    .unwrap();
    let operation = store
        .with_connection(|connection| {
            let version = DocumentVersion {
                provider_revision: OpaqueValue::new("remote-v1")?,
                content_hash: None,
                conditional_write_token: Some(OpaqueValue::new("validator-v1")?),
            };
            let operation = journal::prepare(
                connection,
                descriptor.binding(),
                &RecordKey::new("selected")?,
                &version,
                &MarkdownBytes::new(b"# Original".to_vec())?,
            )?;
            let _permit = journal::begin_dispatch(connection, descriptor.binding(), operation)?;
            Ok(operation)
        })
        .unwrap();
    let issued = daemon.enter().unwrap();
    let (reply, abandoned) = mpsc::sync_channel(1);
    drop(abandoned);
    coordinator
        .sender
        .try_send(Command {
            agent_fence: None,
            request: ProviderRequest::DispatchSave {
                source_id: descriptor.binding().source_id().clone(),
                operation_id: operation,
            },
            reply,
            deadline: Instant::now() + Duration::from_secs(3),
            cancellation: Cancellation::default(),
            effect: Some(issued.fork().unwrap()),
        })
        .unwrap_or_else(|_| panic!("empty bounded command queue"));
    drop(issued);
    observed.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        store.try_lock().is_ok(),
        "freeze waits without holding the actual owner mutex"
    );
    assert!(matches!(
        daemon.freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_millis(20)
        ),
        Err(DrainError::Deadline)
    ));
    release.send(()).unwrap();
    let frozen = daemon
        .freeze(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap();
    let pending = store
        .with_connection(|c| journal::load(c, descriptor.binding(), operation))
        .unwrap();
    assert_eq!(pending.state, journal::MutationState::Unknown);
    assert_eq!(pending.body.as_bytes(), b"# Original");
    drop(frozen);
    drop(coordinator);
    // The actual same SQLite owner retains the uncertain mutation across reopen.
    drop(daemon);
    drop(store);
    let mut reopened = ReaderStore::open(directory.path()).unwrap();
    reopened.with_library_connection(|c| {
        assert_eq!(
            journal::load(c, descriptor.binding(), operation)
                .unwrap()
                .state,
            journal::MutationState::Unknown
        );
    });
}
