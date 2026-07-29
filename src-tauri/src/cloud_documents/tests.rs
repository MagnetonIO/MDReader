use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
use std::collections::BTreeMap;

use async_trait::async_trait;
#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
use catdb_connector_dropbox::{
    DropboxAccessToken, DropboxAuthorizationMode, DropboxContentScope, DropboxCursorKey,
    DropboxDocumentError, DropboxHttpRequest, DropboxHttpResponse, DropboxRevision,
    DropboxRuntimeSecrets, DropboxSecretReference, DropboxSource, DropboxSourceConfig,
    DropboxTransport, DropboxTransportError, PinnedDropboxSourceSpec,
};
#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
use catdb_connector_google_drive::{
    DriveAccessToken, DriveContentScope, DriveCursorKey, DriveDocumentError, DriveHttpMethod,
    DriveHttpRequest, DriveHttpResponse, DriveRevision, DriveRuntimeSecrets, DriveSecretReference,
    DriveSelection, DriveSource, DriveSourceConfig, DriveTransport, DriveTransportError,
    PinnedDriveSourceSpec,
};
use catdb_connectors::{CancellationToken, DataSource, ReadContext, SourceError, SourceId};
use catdb_query::{EntityQuery, EntityTarget};

#[cfg(feature = "google-drive-connector")]
use super::map_drive_document_error;
#[cfg(feature = "dropbox-connector")]
use super::map_dropbox_document_error;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use super::map_lifecycle_error;
#[cfg(feature = "google-drive-connector")]
use super::source::DriveMarkdownDocumentSource;
#[cfg(feature = "dropbox-connector")]
use super::source::DropboxMarkdownDocumentSource;
use super::source::{
    DocumentLifecycle, GuardedMarkdownDocumentOperations, GuardedMarkdownWriteReceipt,
};
use super::{
    CloudDocumentError, CloudIoContext, CloudSyncCoordinator, MarkdownChangeEvent,
    MarkdownChangePage, MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage,
    ProviderCursor, ProviderRevision, SourceRecordKey,
};
use crate::cloud_accounts::{
    CloudAuthorizationProfile, CloudSourceError, NativeMarkdownWriteAcceptance,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestLifecycleState {
    Active,
    Paused,
}

#[derive(Clone)]
struct TestDocumentLifecycle {
    state: Arc<Mutex<TestLifecycleState>>,
}

impl TestDocumentLifecycle {
    fn active() -> Self {
        Self {
            state: Arc::new(Mutex::new(TestLifecycleState::Active)),
        }
    }

    fn pause(&self) {
        *self.state.lock().expect("test lifecycle lock") = TestLifecycleState::Paused;
    }

    fn state(&self) -> TestLifecycleState {
        *self.state.lock().expect("test lifecycle lock")
    }
}

#[async_trait]
impl DocumentLifecycle for TestDocumentLifecycle {
    async fn validate_document_authority(&self) -> Result<(), CloudDocumentError> {
        match self.state() {
            TestLifecycleState::Active => Ok(()),
            TestLifecycleState::Paused => Err(CloudDocumentError::SourcePaused),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TestMutation {
    key: String,
    revision: String,
    body_bytes: usize,
}

struct TestDocumentOperations {
    source_id: SourceId,
    mutations: Arc<Mutex<Vec<TestMutation>>>,
    lifecycle_to_pause_after_effect: Option<TestDocumentLifecycle>,
}

impl TestDocumentOperations {
    fn new(source_id: &str) -> Self {
        Self {
            source_id: SourceId::new(source_id),
            mutations: Arc::new(Mutex::new(Vec::new())),
            lifecycle_to_pause_after_effect: None,
        }
    }

    fn pausing_after_effect(source_id: &str, lifecycle: TestDocumentLifecycle) -> Self {
        Self {
            source_id: SourceId::new(source_id),
            mutations: Arc::new(Mutex::new(Vec::new())),
            lifecycle_to_pause_after_effect: Some(lifecycle),
        }
    }

    fn mutations(&self) -> Vec<TestMutation> {
        self.mutations.lock().expect("test mutation lock").clone()
    }
}

#[async_trait]
impl GuardedMarkdownDocumentOperations for TestDocumentOperations {
    fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    async fn update_markdown(
        &self,
        key: &str,
        expected_revision: &str,
        body: &[u8],
        _context: &ReadContext,
    ) -> Result<GuardedMarkdownWriteReceipt, CloudDocumentError> {
        self.mutations
            .lock()
            .expect("test mutation lock")
            .push(TestMutation {
                key: key.to_string(),
                revision: expected_revision.to_string(),
                body_bytes: body.len(),
            });
        if let Some(lifecycle) = &self.lifecycle_to_pause_after_effect {
            lifecycle.pause();
        }
        Ok(GuardedMarkdownWriteReceipt::new(
            "provider-record",
            "provider-revision",
        ))
    }
}

fn cloud_context() -> CloudIoContext {
    CloudIoContext {
        deadline: Instant::now() + Duration::from_secs(1),
        page_rows: 8,
        byte_limit: 8 * 1024,
        cancellation: CancellationToken::new(),
    }
}

fn expected_revision() -> ProviderRevision {
    ProviderRevision {
        opaque: "expected-revision".to_string(),
        content_hash: None,
    }
}

mod read {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };

    use async_trait::async_trait;
    use catdb_connectors::SourceId;

    use super::super::{
        service::{CloudDocumentReadAuthority, CloudDocumentService, CloudEditAuthorityCodec},
        store::{CloudDocumentStore, CloudDocumentVersionPins},
        CloudCacheState, CloudDocumentError, CloudIoContext, MarkdownChangeEvent,
        MarkdownChangePage, MarkdownDocumentRevision, MarkdownDocumentSource, MarkdownMetadata,
        MarkdownMetadataPage, MarkdownWriteReceipt, ProviderCursor, ProviderRevision,
        SourceRecordKey,
    };

    struct PointReadProvider {
        source_id: SourceId,
        read_keys: Mutex<Vec<String>>,
        offline: AtomicBool,
    }

    struct TestReadAuthority {
        validations: AtomicUsize,
        reject_at: Option<usize>,
    }

    impl TestReadAuthority {
        fn active() -> Arc<Self> {
            Arc::new(Self {
                validations: AtomicUsize::new(0),
                reject_at: None,
            })
        }

        fn retiring_after_provider_read() -> Arc<Self> {
            Arc::new(Self {
                validations: AtomicUsize::new(0),
                reject_at: Some(2),
            })
        }
    }

    #[async_trait]
    impl CloudDocumentReadAuthority for TestReadAuthority {
        async fn revalidate(&self) -> Result<(), CloudDocumentError> {
            let ordinal = self.validations.fetch_add(1, Ordering::SeqCst) + 1;
            if self.reject_at.is_some_and(|reject_at| ordinal >= reject_at) {
                Err(CloudDocumentError::Unauthorized)
            } else {
                Ok(())
            }
        }
    }

    impl PointReadProvider {
        fn new(source_id: &str) -> Self {
            Self {
                source_id: SourceId::new(source_id),
                read_keys: Mutex::new(Vec::new()),
                offline: AtomicBool::new(false),
            }
        }

        fn read_keys(&self) -> Vec<String> {
            self.read_keys.lock().expect("read keys").clone()
        }

        fn mark_offline(&self) {
            self.offline.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl MarkdownDocumentSource for PointReadProvider {
        fn source_id(&self) -> &SourceId {
            &self.source_id
        }

        async fn start_change_tracking(
            &self,
            _context: &CloudIoContext,
        ) -> Result<ProviderCursor, CloudDocumentError> {
            Err(CloudDocumentError::ProtocolFailed)
        }

        async fn list_page(
            &self,
            _cursor: Option<&ProviderCursor>,
            _context: &CloudIoContext,
        ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
            Err(CloudDocumentError::ProtocolFailed)
        }

        async fn read_document(
            &self,
            key: &SourceRecordKey,
            _context: &CloudIoContext,
        ) -> Result<MarkdownDocumentRevision, CloudDocumentError> {
            self.read_keys
                .lock()
                .expect("read keys")
                .push(key.as_str().to_string());
            if self.offline.load(Ordering::SeqCst) {
                return Err(CloudDocumentError::OfflineUncached);
            }
            Ok(MarkdownDocumentRevision {
                key: key.clone(),
                revision: ProviderRevision {
                    opaque: "r1".to_string(),
                    content_hash: None,
                },
                body: b"# seven".to_vec(),
                observed_at_ms: 11,
            })
        }

        async fn write_document(
            &self,
            _key: &SourceRecordKey,
            _expected: &ProviderRevision,
            _body: &[u8],
            _context: &CloudIoContext,
        ) -> Result<MarkdownWriteReceipt, CloudDocumentError> {
            Err(CloudDocumentError::ProtocolFailed)
        }

        async fn read_changes(
            &self,
            _cursor: &ProviderCursor,
            _context: &CloudIoContext,
        ) -> Result<MarkdownChangePage, CloudDocumentError> {
            Err(CloudDocumentError::ProtocolFailed)
        }

        async fn materialize_changes(
            &self,
            _changes: &[MarkdownChangeEvent],
            _known: &[super::super::KnownMarkdownMetadata],
            _context: &CloudIoContext,
        ) -> Result<Vec<MarkdownMetadata>, CloudDocumentError> {
            Err(CloudDocumentError::ProtocolFailed)
        }
    }

    fn publish_record(
        store: &CloudDocumentStore,
        source_id: &SourceId,
        revision: &str,
    ) -> CloudDocumentVersionPins {
        let pins =
            CloudDocumentVersionPins::new([1; 16], [2; 16], [3; 16]).expect("valid version pins");
        let generation = store
            .begin_initial_sync(source_id, &ProviderCursor::new(b"start"), &pins, 1)
            .expect("begin initial sync");
        store
            .stage_page(
                &generation,
                &pins,
                &MarkdownMetadataPage {
                    rows: vec![MarkdownMetadata {
                        key: SourceRecordKey::new("record-7"),
                        name: "seven.md".to_string(),
                        display_path: "/seven.md".to_string(),
                        media_type: Some("text/markdown".to_string()),
                        modified_at_ms: Some(7),
                        size_bytes: Some(7),
                        revision: ProviderRevision {
                            opaque: revision.to_string(),
                            content_hash: None,
                        },
                        deleted: false,
                    }],
                    next_cursor: None,
                    observed_at_ms: 2,
                },
            )
            .expect("stage metadata");
        store
            .publish_initial_sync(&generation, &ProviderCursor::new(b"published"), 2)
            .expect("publish metadata");
        pins
    }

    #[tokio::test]
    async fn opening_a_published_row_downloads_only_that_record_and_caches_its_revision() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.catdb")).expect("store"),
        );
        let source_id = SourceId::new("point-read-source");
        let pins = publish_record(&store, &source_id, "r1");
        let provider = Arc::new(PointReadProvider::new(source_id.as_str()));
        let service = CloudDocumentService::new(
            Arc::clone(&store),
            Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>,
            pins,
            [4; 16],
            [5; 32],
            TestReadAuthority::active(),
        );

        let first = service
            .read_cloud_markdown(&source_id, "record-7")
            .await
            .expect("provider read");
        let second = service
            .read_cloud_markdown(&source_id, "record-7")
            .await
            .expect("cached read");

        assert_eq!(first.content, "# seven");
        assert_eq!(first.cache_state, CloudCacheState::ProviderFresh);
        assert!(first.edit_token.is_some());
        assert_eq!(second.cache_state, CloudCacheState::Cached);
        assert!(second.edit_token.is_some());
        assert_eq!(provider.read_keys(), ["record-7"]);
    }

    #[tokio::test]
    async fn offline_read_uses_only_an_exact_revision_cache_hit() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.catdb")).expect("store"),
        );
        let source_id = SourceId::new("offline-point-read-source");
        let pins = publish_record(&store, &source_id, "r1");
        let provider = Arc::new(PointReadProvider::new(source_id.as_str()));
        let service = CloudDocumentService::new(
            Arc::clone(&store),
            Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>,
            pins.clone(),
            [4; 16],
            [5; 32],
            TestReadAuthority::active(),
        );
        service
            .read_cloud_markdown(&source_id, "record-7")
            .await
            .expect("seed exact revision cache");
        provider.mark_offline();

        let cached = service
            .read_cloud_markdown(&source_id, "record-7")
            .await
            .expect("exact revision cache hit");
        assert_eq!(cached.content, "# seven");
        assert_eq!(provider.read_keys(), ["record-7"]);

        publish_record(&store, &source_id, "r2");
        assert_eq!(
            service.read_cloud_markdown(&source_id, "record-7").await,
            Err(CloudDocumentError::OfflineUncached)
        );
        assert_eq!(provider.read_keys(), ["record-7", "record-7"]);
    }

    #[tokio::test]
    async fn source_replacement_during_provider_read_neither_caches_nor_returns_the_body() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.catdb")).expect("store"),
        );
        let source_id = SourceId::new("retiring-point-read-source");
        let pins = publish_record(&store, &source_id, "r1");
        let provider = Arc::new(PointReadProvider::new(source_id.as_str()));
        let service = CloudDocumentService::new(
            Arc::clone(&store),
            Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>,
            pins.clone(),
            [4; 16],
            [5; 32],
            TestReadAuthority::retiring_after_provider_read(),
        );

        assert_eq!(
            service.read_cloud_markdown(&source_id, "record-7").await,
            Err(CloudDocumentError::Unauthorized)
        );
        assert_eq!(provider.read_keys(), ["record-7"]);
        assert!(store
            .get_body(
                &source_id,
                &pins,
                &SourceRecordKey::new("record-7"),
                &ProviderRevision {
                    opaque: "r1".to_string(),
                    content_hash: None,
                },
                20,
            )
            .expect("inspect body cache")
            .is_none());
    }

    #[test]
    fn edit_authority_rejects_forgery_expiry_and_anchor_mismatches() {
        let codec = CloudEditAuthorityCodec::new([9; 32]);
        let library_id = [8; 16];
        let source_id = SourceId::new("authority-source");
        let source_version = [7; 16];
        let record_key = SourceRecordKey::new("record-7");
        let revision = ProviderRevision {
            opaque: "r1".to_string(),
            content_hash: None,
        };
        let body = b"# seven";
        let token = codec
            .issue(
                1_000,
                &library_id,
                &source_id,
                &source_version,
                &record_key,
                &revision,
                body,
            )
            .expect("issue authority");

        codec
            .verify(
                &token,
                1_000 + 30 * 60 * 1_000,
                &library_id,
                &source_id,
                &source_version,
                &record_key,
                &revision,
                body,
            )
            .expect("authority remains valid through its exact expiry");
        assert_eq!(
            codec.verify(
                &format!("{token}A"),
                1_001,
                &library_id,
                &source_id,
                &source_version,
                &record_key,
                &revision,
                body,
            ),
            Err(CloudDocumentError::Unauthorized)
        );
        assert_eq!(
            codec.verify(
                &token,
                1_000 + 30 * 60 * 1_000 + 1,
                &library_id,
                &source_id,
                &source_version,
                &record_key,
                &revision,
                body,
            ),
            Err(CloudDocumentError::Unauthorized)
        );
        assert_eq!(
            codec.verify(
                &token,
                1_001,
                &library_id,
                &source_id,
                &[6; 16],
                &record_key,
                &revision,
                body,
            ),
            Err(CloudDocumentError::Unauthorized)
        );
        assert_eq!(
            codec.verify(
                &token,
                1_001,
                &library_id,
                &source_id,
                &source_version,
                &SourceRecordKey::new("record-8"),
                &revision,
                body,
            ),
            Err(CloudDocumentError::Unauthorized)
        );
        assert_eq!(
            codec.verify(
                &token,
                1_001,
                &library_id,
                &source_id,
                &source_version,
                &record_key,
                &revision,
                b"# changed",
            ),
            Err(CloudDocumentError::Unauthorized)
        );
    }

    #[test]
    fn cache_publication_refuses_a_revision_retired_during_the_provider_read() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = CloudDocumentStore::open(directory.path().join("cloud.catdb")).expect("store");
        let source_id = SourceId::new("revision-race-source");
        let pins = publish_record(&store, &source_id, "r2");

        assert!(store
            .put_body(
                &source_id,
                &pins,
                &MarkdownDocumentRevision {
                    key: SourceRecordKey::new("record-7"),
                    revision: ProviderRevision {
                        opaque: "r1".to_string(),
                        content_hash: None,
                    },
                    body: b"# stale".to_vec(),
                    observed_at_ms: 9,
                },
            )
            .is_err());
    }
}

struct SyncFakeProvider {
    source_id: SourceId,
    start_cursor: ProviderCursor,
    list_pages: Mutex<VecDeque<Result<MarkdownMetadataPage, CloudDocumentError>>>,
    change_pages: Mutex<VecDeque<Result<MarkdownChangePage, CloudDocumentError>>>,
    start_error: Option<CloudDocumentError>,
    starts: AtomicUsize,
    list_calls: AtomicUsize,
    change_calls: AtomicUsize,
    block_next_change: std::sync::atomic::AtomicBool,
    entered_change: tokio::sync::Notify,
}

impl SyncFakeProvider {
    fn initial_list_race() -> Self {
        Self::scripted(
            "cloud-sync-test-source",
            b"c0",
            [
                Ok(sync_page(&["a.md"], Some(b"p2"))),
                Ok(sync_page(&["b.md"], None)),
            ],
            [Ok(MarkdownChangePage {
                changes: vec![sync_deleted_change("a.md"), sync_upsert_change("c.md")],
                next_cursor: ProviderCursor(b"c1".to_vec()),
                has_more: false,
                observed_at_ms: 3,
            })],
        )
    }

    fn scripted(
        source_id: &str,
        start_cursor: &[u8],
        list_pages: impl IntoIterator<Item = Result<MarkdownMetadataPage, CloudDocumentError>>,
        change_pages: impl IntoIterator<Item = Result<MarkdownChangePage, CloudDocumentError>>,
    ) -> Self {
        Self {
            source_id: SourceId::new(source_id),
            start_cursor: ProviderCursor(start_cursor.to_vec()),
            list_pages: Mutex::new(list_pages.into_iter().collect()),
            change_pages: Mutex::new(change_pages.into_iter().collect()),
            start_error: None,
            starts: AtomicUsize::new(0),
            list_calls: AtomicUsize::new(0),
            change_calls: AtomicUsize::new(0),
            block_next_change: std::sync::atomic::AtomicBool::new(false),
            entered_change: tokio::sync::Notify::new(),
        }
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    fn enqueue_change(&self, page: Result<MarkdownChangePage, CloudDocumentError>) {
        self.change_pages
            .lock()
            .expect("fake change pages")
            .push_back(page);
    }

    fn block_next_change(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.block_next_change.store(true, Ordering::SeqCst);
        self.entered_change.notified()
    }

    fn failing_start(source_id: &str, error: CloudDocumentError) -> Self {
        let mut provider = Self::scripted(source_id, b"c0", std::iter::empty(), std::iter::empty());
        provider.start_error = Some(error);
        provider
    }
}

#[async_trait]
impl MarkdownDocumentSource for SyncFakeProvider {
    fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    async fn start_change_tracking(
        &self,
        _context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = &self.start_error {
            return Err(error.clone());
        }
        Ok(self.start_cursor.clone())
    }

    async fn list_page(
        &self,
        _cursor: Option<&ProviderCursor>,
        _context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        self.list_pages
            .lock()
            .expect("fake list pages")
            .pop_front()
            .unwrap_or(Err(CloudDocumentError::ProtocolFailed))
    }

    async fn read_document(
        &self,
        _key: &SourceRecordKey,
        _context: &CloudIoContext,
    ) -> Result<super::MarkdownDocumentRevision, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn write_document(
        &self,
        _key: &SourceRecordKey,
        _expected: &ProviderRevision,
        _body: &[u8],
        _context: &CloudIoContext,
    ) -> Result<super::MarkdownWriteReceipt, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn read_changes(
        &self,
        _cursor: &ProviderCursor,
        context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        self.change_calls.fetch_add(1, Ordering::SeqCst);
        if self.block_next_change.swap(false, Ordering::SeqCst) {
            self.entered_change.notify_waiters();
            context.cancellation.cancelled().await;
            return Err(CloudDocumentError::Cancelled);
        }
        self.change_pages
            .lock()
            .expect("fake change pages")
            .pop_front()
            .unwrap_or(Err(CloudDocumentError::ProtocolFailed))
    }

    async fn materialize_changes(
        &self,
        changes: &[MarkdownChangeEvent],
        known: &[super::KnownMarkdownMetadata],
        _context: &CloudIoContext,
    ) -> Result<Vec<MarkdownMetadata>, CloudDocumentError> {
        Ok(changes
            .iter()
            .filter_map(|change| match change {
                MarkdownChangeEvent::Upsert { key } => Some(sync_metadata(key.as_str())),
                MarkdownChangeEvent::Deleted { key } => known
                    .iter()
                    .find(|metadata| metadata.key.as_str() == key.as_str())
                    .map(|_| sync_deleted(key.as_str())),
            })
            .collect())
    }
}

struct BlockingListSyncProvider {
    source_id: SourceId,
    entered_list: tokio::sync::Notify,
    list_calls: AtomicUsize,
}

impl BlockingListSyncProvider {
    fn new(source_id: &str) -> Self {
        Self {
            source_id: SourceId::new(source_id),
            entered_list: tokio::sync::Notify::new(),
            list_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl MarkdownDocumentSource for BlockingListSyncProvider {
    fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    async fn start_change_tracking(
        &self,
        _context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError> {
        Ok(ProviderCursor(b"c0".to_vec()))
    }

    async fn list_page(
        &self,
        _cursor: Option<&ProviderCursor>,
        context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        self.entered_list.notify_waiters();
        context.cancellation.cancelled().await;
        Err(CloudDocumentError::Cancelled)
    }

    async fn read_document(
        &self,
        _key: &SourceRecordKey,
        _context: &CloudIoContext,
    ) -> Result<super::MarkdownDocumentRevision, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn write_document(
        &self,
        _key: &SourceRecordKey,
        _expected: &ProviderRevision,
        _body: &[u8],
        _context: &CloudIoContext,
    ) -> Result<super::MarkdownWriteReceipt, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn read_changes(
        &self,
        _cursor: &ProviderCursor,
        _context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn materialize_changes(
        &self,
        _changes: &[MarkdownChangeEvent],
        _known: &[super::KnownMarkdownMetadata],
        _context: &CloudIoContext,
    ) -> Result<Vec<MarkdownMetadata>, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }
}

struct BlockingChangeSyncProvider {
    source_id: SourceId,
    entered_changes: tokio::sync::Notify,
}

impl BlockingChangeSyncProvider {
    fn new(source_id: &str) -> Self {
        Self {
            source_id: SourceId::new(source_id),
            entered_changes: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait]
impl MarkdownDocumentSource for BlockingChangeSyncProvider {
    fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    async fn start_change_tracking(
        &self,
        _context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn list_page(
        &self,
        _cursor: Option<&ProviderCursor>,
        _context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn read_document(
        &self,
        _key: &SourceRecordKey,
        _context: &CloudIoContext,
    ) -> Result<super::MarkdownDocumentRevision, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn write_document(
        &self,
        _key: &SourceRecordKey,
        _expected: &ProviderRevision,
        _body: &[u8],
        _context: &CloudIoContext,
    ) -> Result<super::MarkdownWriteReceipt, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }

    async fn read_changes(
        &self,
        _cursor: &ProviderCursor,
        context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        self.entered_changes.notify_waiters();
        context.cancellation.cancelled().await;
        Err(CloudDocumentError::Cancelled)
    }

    async fn materialize_changes(
        &self,
        _changes: &[MarkdownChangeEvent],
        _known: &[super::KnownMarkdownMetadata],
        _context: &CloudIoContext,
    ) -> Result<Vec<MarkdownMetadata>, CloudDocumentError> {
        Err(CloudDocumentError::ProtocolFailed)
    }
}

fn sync_metadata(name: &str) -> MarkdownMetadata {
    MarkdownMetadata {
        key: SourceRecordKey::new(name),
        name: name.to_string(),
        display_path: format!("/documents/{name}"),
        media_type: Some("text/markdown".to_string()),
        modified_at_ms: Some(1),
        size_bytes: Some(1),
        revision: ProviderRevision {
            opaque: format!("revision-{name}"),
            content_hash: None,
        },
        deleted: false,
    }
}

fn sync_deleted(name: &str) -> MarkdownMetadata {
    let mut metadata = sync_metadata(name);
    metadata.deleted = true;
    metadata
}

fn sync_upsert_change(name: &str) -> MarkdownChangeEvent {
    MarkdownChangeEvent::Upsert {
        key: SourceRecordKey::new(name),
    }
}

fn sync_deleted_change(name: &str) -> MarkdownChangeEvent {
    MarkdownChangeEvent::Deleted {
        key: SourceRecordKey::new(name),
    }
}

fn sync_page(names: &[&str], next_cursor: Option<&[u8]>) -> MarkdownMetadataPage {
    MarkdownMetadataPage {
        rows: names.iter().map(|name| sync_metadata(name)).collect(),
        next_cursor: next_cursor.map(|cursor| ProviderCursor(cursor.to_vec())),
        observed_at_ms: 1,
    }
}

fn sync_pins() -> super::store::CloudDocumentVersionPins {
    super::store::CloudDocumentVersionPins::new(b"schema-v1", b"mapping-v1", b"source-v1")
        .expect("fixed sync pins")
}

#[tokio::test]
async fn large_materialized_library_returns_a_bounded_page_without_full_snapshot_loading() {
    let directory = tempfile::tempdir().expect("temporary SQLite directory");
    let store = Arc::new(
        super::store::CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
            .expect("open cloud store"),
    );
    let source_id = SourceId::new("published-drive-source");
    let generation = store
        .begin_initial_sync(
            &source_id,
            &ProviderCursor(b"initial".to_vec()),
            &sync_pins(),
            1,
        )
        .expect("begin published snapshot");
    for offset in (0..100_000).step_by(256) {
        let end = (offset + 256).min(100_000);
        store
            .stage_page(
                &generation,
                &sync_pins(),
                &MarkdownMetadataPage {
                    rows: (offset..end)
                        .map(|index| sync_metadata(&format!("row-{index:06}.md")))
                        .collect(),
                    next_cursor: None,
                    observed_at_ms: 1,
                },
            )
            .expect("stage bounded metadata page");
    }
    store
        .publish_initial_sync(&generation, &ProviderCursor(b"published".to_vec()), 2)
        .expect("publish materialization");
    store.reject_next_full_published_listing();

    let source = super::materialized::MaterializedMarkdownSource::google_drive(
        Arc::clone(&store),
        source_id,
    );
    let plan = source
        .compile(
            &EntityQuery {
                target: EntityTarget::Entity("mdreader.Document".to_string()),
                filters: Vec::new(),
            },
            &["id".to_string(), "name".to_string()],
        )
        .expect("compile local SQLite page");
    let context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        1_000,
        1024 * 1024,
        50,
        CancellationToken::new(),
    )
    .expect("bounded read context");
    let first = source
        .execute_page(&plan, None, &context)
        .await
        .expect("first materialized page");

    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.rows[0].key, "row-000000.md");
    let cursor = first
        .next_cursor
        .expect("continuation for the next local page");
    let second = source
        .execute_page(&plan, Some(&cursor), &context)
        .await
        .expect("second materialized page");
    assert_eq!(second.rows.len(), 50);
    assert_eq!(second.rows[0].key, "row-000050.md");
}

#[test]
fn materialized_metadata_refuses_body_projection_until_the_on_demand_read_exists() {
    let directory = tempfile::tempdir().expect("temporary SQLite directory");
    let store = Arc::new(
        super::store::CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
            .expect("open cloud store"),
    );
    let source = super::materialized::MaterializedMarkdownSource::google_drive(
        store,
        SourceId::new("metadata-only-drive-source"),
    );

    let error = source
        .compile(
            &EntityQuery {
                target: EntityTarget::Entity("mdreader.Document".to_string()),
                filters: Vec::new(),
            },
            &["body".to_string()],
        )
        .expect_err("metadata materialization must not invent a provider body");
    assert!(matches!(
        error,
        SourceError::UnsupportedFilter { attr, .. } if attr == "body"
    ));
}

#[tokio::test]
async fn materialized_source_without_a_published_snapshot_is_a_typed_unavailable_source() {
    let directory = tempfile::tempdir().expect("temporary SQLite directory");
    let store = Arc::new(
        super::store::CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
            .expect("open cloud store"),
    );
    let source = super::materialized::MaterializedMarkdownSource::google_drive(
        store,
        SourceId::new("not-yet-published-drive-source"),
    );
    let context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(1),
        50,
        1024,
        50,
        CancellationToken::new(),
    )
    .expect("bounded read context");

    assert!(matches!(
        source.freshness(&context).await,
        Err(SourceError::Unavailable { .. })
    ));
}

#[tokio::test]
async fn initial_sync_replays_changes_from_the_position_taken_before_listing() {
    let directory = tempfile::tempdir().expect("temporary SQLite directory");
    let store = Arc::new(
        super::store::CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
            .expect("open cloud store"),
    );
    let provider = Arc::new(SyncFakeProvider::initial_list_race());
    let source_id = provider.source_id().clone();
    let coordinator = CloudSyncCoordinator::new(Arc::clone(&store), sync_pins());

    coordinator.activate(provider).await.expect("activate sync");
    coordinator
        .wait_until_idle(&source_id)
        .await
        .expect("initial sync completes");

    let names = store
        .list_published(&source_id)
        .expect("published metadata")
        .into_iter()
        .filter(|document| !document.deleted)
        .map(|document| document.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["b.md", "c.md"]);
    assert_eq!(
        store.load_change_cursor(&source_id).expect("change cursor"),
        Some(ProviderCursor(b"c1".to_vec()))
    );
}

mod sync {
    use super::super::{
        store::{CloudDocumentStore, PersistedCloudWriteRepair},
        CloudSyncCoordinator, CloudSyncPhase, CloudWriteRepairExecutor,
    };
    use super::*;

    struct RecordingRepairExecutor {
        local_publications: AtomicUsize,
        provider_mutations: AtomicUsize,
    }

    impl RecordingRepairExecutor {
        fn new() -> Self {
            Self {
                local_publications: AtomicUsize::new(0),
                provider_mutations: AtomicUsize::new(0),
            }
        }

        fn local_publications(&self) -> usize {
            self.local_publications.load(Ordering::SeqCst)
        }

        fn provider_mutations(&self) -> usize {
            self.provider_mutations.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CloudWriteRepairExecutor for RecordingRepairExecutor {
        async fn resume_local_publication(
            &self,
            _repair: &PersistedCloudWriteRepair,
        ) -> Result<(), CloudDocumentError> {
            self.local_publications.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct PendingRepairExecutor;

    #[async_trait]
    impl CloudWriteRepairExecutor for PendingRepairExecutor {
        async fn resume_local_publication(
            &self,
            _repair: &PersistedCloudWriteRepair,
        ) -> Result<(), CloudDocumentError> {
            Err(CloudDocumentError::LocalPublicationPending)
        }
    }

    struct BlockingRepairExecutor {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    impl BlockingRepairExecutor {
        fn new() -> Self {
            Self {
                entered: Arc::new(tokio::sync::Notify::new()),
                release: Arc::new(tokio::sync::Notify::new()),
            }
        }
    }

    #[async_trait]
    impl CloudWriteRepairExecutor for BlockingRepairExecutor {
        async fn resume_local_publication(
            &self,
            _repair: &PersistedCloudWriteRepair,
        ) -> Result<(), CloudDocumentError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    fn published_names(store: &CloudDocumentStore, source_id: &SourceId) -> Vec<String> {
        store
            .list_published(source_id)
            .expect("published metadata")
            .into_iter()
            .filter(|document| !document.deleted)
            .map(|document| document.name)
            .collect()
    }

    fn publish_before_sync(store: &CloudDocumentStore, source_id: &SourceId, name: &str) {
        let generation = store
            .begin_initial_sync(
                source_id,
                &ProviderCursor(b"previous".to_vec()),
                &sync_pins(),
                1,
            )
            .expect("begin previous snapshot");
        store
            .stage_page(&generation, &sync_pins(), &sync_page(&[name], None))
            .expect("stage previous snapshot");
        store
            .publish_initial_sync(&generation, &ProviderCursor(b"previous".to_vec()), 2)
            .expect("publish previous snapshot");
    }

    fn publish_many_before_sync(store: &CloudDocumentStore, source_id: &SourceId, rows: usize) {
        let generation = store
            .begin_initial_sync(
                source_id,
                &ProviderCursor(b"previous".to_vec()),
                &sync_pins(),
                1,
            )
            .expect("begin previous snapshot");
        for offset in (0..rows).step_by(256) {
            let end = (offset + 256).min(rows);
            store
                .stage_page(
                    &generation,
                    &sync_pins(),
                    &MarkdownMetadataPage {
                        rows: (offset..end)
                            .map(|index| sync_metadata(&format!("existing-{index:04}.md")))
                            .collect(),
                        next_cursor: None,
                        observed_at_ms: 1,
                    },
                )
                .expect("stage bounded published rows");
        }
        store
            .publish_initial_sync(&generation, &ProviderCursor(b"previous".to_vec()), 2)
            .expect("publish previous snapshot");
    }

    fn coordinator(store: Arc<CloudDocumentStore>) -> CloudSyncCoordinator {
        CloudSyncCoordinator::new(store, sync_pins())
    }

    #[tokio::test]
    async fn refresh_after_idle_applies_a_later_change_and_advances_the_committed_cursor() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::scripted(
            "refresh-source",
            b"c0",
            [Ok(sync_page(&["before.md"], None))],
            [Ok(MarkdownChangePage {
                changes: Vec::new(),
                next_cursor: ProviderCursor(b"c1".to_vec()),
                has_more: false,
                observed_at_ms: 2,
            })],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate initial sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("initial sync settles");
        provider.enqueue_change(Ok(MarkdownChangePage {
            changes: vec![sync_upsert_change("later.md")],
            next_cursor: ProviderCursor(b"c2".to_vec()),
            has_more: false,
            observed_at_ms: 3,
        }));

        coordinator
            .refresh(&source_id)
            .await
            .expect("schedule later refresh");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("later refresh settles");

        assert_eq!(
            published_names(&store, &source_id),
            ["before.md", "later.md"]
        );
        assert_eq!(
            store
                .load_change_cursor(&source_id)
                .expect("advanced cursor"),
            Some(ProviderCursor(b"c2".to_vec()))
        );
    }

    #[tokio::test]
    async fn overlapping_refresh_requests_keep_one_incremental_owner() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::scripted(
            "overlapping-refresh-source",
            b"c0",
            [Ok(sync_page(&["before.md"], None))],
            [Ok(MarkdownChangePage {
                changes: Vec::new(),
                next_cursor: ProviderCursor(b"c1".to_vec()),
                has_more: false,
                observed_at_ms: 2,
            })],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate initial sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("initial sync settles");
        let entered_refresh = provider.block_next_change();

        coordinator
            .refresh(&source_id)
            .await
            .expect("start first refresh");
        entered_refresh.await;
        coordinator
            .refresh(&source_id)
            .await
            .expect("coalesce overlapping refresh");
        coordinator.pause(&source_id).await.expect("cancel refresh");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("cancelled refresh settles");

        assert_eq!(
            provider.change_calls.load(Ordering::SeqCst),
            2,
            "initial replay plus exactly one refresh provider call"
        );
    }

    async fn bounded_incremental_replay_never_reads_the_whole_published_snapshot(source: &str) {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::scripted(
            source,
            b"unused",
            [],
            [Ok(MarkdownChangePage {
                changes: vec![sync_upsert_change("replayed.md")],
                next_cursor: ProviderCursor(b"advanced".to_vec()),
                has_more: false,
                observed_at_ms: 3,
            })],
        ));
        let source_id = provider.source_id().clone();
        publish_many_before_sync(&store, &source_id, 1_001);
        store.reject_next_full_published_listing();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate committed-cursor replay");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("bounded replay settles");

        let replayed = store
            .published_for_keys(&source_id, &[SourceRecordKey::new("replayed.md")])
            .expect("bounded replay result");
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].name, "replayed.md");
        assert_eq!(
            store
                .load_change_cursor(&source_id)
                .expect("advanced cursor"),
            Some(ProviderCursor(b"advanced".to_vec()))
        );
    }

    #[tokio::test]
    async fn google_drive_incremental_replay_over_one_thousand_rows_stays_bounded() {
        bounded_incremental_replay_never_reads_the_whole_published_snapshot("google-drive-replay")
            .await;
    }

    #[tokio::test]
    async fn dropbox_incremental_replay_over_one_thousand_rows_stays_bounded() {
        bounded_incremental_replay_never_reads_the_whole_published_snapshot("dropbox-replay").await;
    }

    #[tokio::test]
    async fn provider_deadline_wins_when_a_provider_future_never_returns() {
        let cancellation = CancellationToken::new();
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            super::super::sync::provider_call(
                &cancellation,
                Instant::now() - Duration::from_millis(1),
                std::future::pending::<Result<(), CloudDocumentError>>(),
            ),
        )
        .await
        .expect("the coordinator deadline must settle a non-cooperative provider");

        assert_eq!(result, Err(CloudDocumentError::Timeout));
    }

    #[tokio::test]
    async fn failed_initial_listing_keeps_the_previous_snapshot_and_cursor_published() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::scripted(
            "failed-list-source",
            b"c0",
            [Err(CloudDocumentError::ProtocolFailed)],
            [Err(CloudDocumentError::ResetCursor)],
        ));
        let source_id = provider.source_id().clone();
        publish_before_sync(&store, &source_id, "before.md");
        let coordinator = coordinator(Arc::clone(&store));

        coordinator.activate(provider).await.expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("failure settles");

        assert_eq!(published_names(&store, &source_id), ["before.md"]);
        assert_eq!(
            store
                .load_change_cursor(&source_id)
                .expect("previous cursor"),
            Some(ProviderCursor(b"previous".to_vec()))
        );
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Backoff {
                error: CloudDocumentError::ProtocolFailed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn reset_cursor_discards_staging_and_runs_one_fresh_initial_generation() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::scripted(
            "reset-source",
            b"c0",
            [
                Ok(sync_page(&["first.md"], None)),
                Ok(sync_page(&["replacement.md"], None)),
            ],
            [
                Err(CloudDocumentError::ResetCursor),
                Ok(MarkdownChangePage {
                    changes: Vec::new(),
                    next_cursor: ProviderCursor(b"c2".to_vec()),
                    has_more: false,
                    observed_at_ms: 4,
                }),
            ],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("reset rebuild completes");

        assert_eq!(provider.starts(), 2);
        assert_eq!(published_names(&store, &source_id), ["replacement.md"]);
        assert_eq!(
            store
                .load_change_cursor(&source_id)
                .expect("replacement cursor"),
            Some(ProviderCursor(b"c2".to_vec()))
        );
    }

    #[tokio::test]
    async fn pause_cancels_an_in_flight_provider_call_without_publishing_or_calling_changes() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(BlockingListSyncProvider::new("paused-source"));
        let source_id = provider.source_id().clone();
        let entered = provider.entered_list.notified();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate sync");
        entered.await;
        coordinator.pause(&source_id).await.expect("pause sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("pause settles");

        assert_eq!(provider.list_calls.load(Ordering::SeqCst), 1);
        assert!(published_names(&store, &source_id).is_empty());
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Paused)
        ));
    }

    #[tokio::test]
    async fn pause_serializes_with_the_post_provider_store_boundary_and_prevents_late_publication()
    {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::scripted(
            "store-boundary-source",
            b"c0",
            [Ok(sync_page(&["late.md"], None))],
            [Ok(MarkdownChangePage {
                changes: Vec::new(),
                next_cursor: ProviderCursor(b"c1".to_vec()),
                has_more: false,
                observed_at_ms: 2,
            })],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));
        let barrier = coordinator.block_durable_effect(2);
        let stage_entered = barrier.entered();

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate sync");
        stage_entered.await;

        let pausing = {
            let coordinator = coordinator.clone();
            let source_id = source_id.clone();
            tokio::spawn(async move { coordinator.pause(&source_id).await })
        };
        tokio::task::yield_now().await;
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Initializing { .. })
        ));
        assert!(
            !pausing.is_finished(),
            "pause must wait for the store boundary"
        );

        barrier.release();
        pausing.await.expect("pause task").expect("pause sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("pause settles");

        assert!(published_names(&store, &source_id).is_empty());
        assert_eq!(provider.change_calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Paused)
        ));
    }

    #[tokio::test]
    async fn replacement_epoch_cannot_publish_a_detached_initial_owner() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let old = Arc::new(BlockingListSyncProvider::new("replacement-source"));
        let source_id = old.source_id().clone();
        let replacement = Arc::new(SyncFakeProvider::scripted(
            "replacement-source",
            b"c1",
            [Ok(sync_page(&["replacement.md"], None))],
            [Ok(MarkdownChangePage {
                changes: Vec::new(),
                next_cursor: ProviderCursor(b"c2".to_vec()),
                has_more: false,
                observed_at_ms: 2,
            })],
        ));
        let entered = old.entered_list.notified();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&old) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate old owner");
        entered.await;
        coordinator
            .activate(Arc::clone(&replacement) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate replacement owner");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("replacement settles");

        assert_eq!(published_names(&store, &source_id), ["replacement.md"]);
        assert_eq!(
            store
                .load_change_cursor(&source_id)
                .expect("replacement cursor"),
            Some(ProviderCursor(b"c2".to_vec()))
        );
    }

    #[tokio::test]
    async fn retire_cancels_then_removes_only_the_disconnected_source_state() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let retiring = Arc::new(BlockingChangeSyncProvider::new("retiring-source"));
        let retiring_id = retiring.source_id().clone();
        let replacement_id = SourceId::new("replacement-source");
        publish_before_sync(&store, &retiring_id, "retiring.md");
        publish_before_sync(&store, &replacement_id, "replacement.md");
        let entered = retiring.entered_changes.notified();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&retiring) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate retiring source");
        entered.await;
        coordinator
            .retire(&retiring_id)
            .await
            .expect("retire source");
        coordinator
            .wait_until_idle(&retiring_id)
            .await
            .expect("retirement settles");

        assert!(published_names(&store, &retiring_id).is_empty());
        assert_eq!(published_names(&store, &replacement_id), ["replacement.md"]);
        assert!(matches!(
            coordinator.status(&retiring_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Retired)
        ));
    }

    #[tokio::test]
    async fn a_repeated_list_cursor_enters_a_typed_backoff_without_publication() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::scripted(
            "repeated-cursor-source",
            b"c0",
            [
                Ok(sync_page(&["first.md"], Some(b"p1"))),
                Ok(sync_page(&["second.md"], Some(b"p1"))),
            ],
            [],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator.activate(provider).await.expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("failure settles");

        assert!(published_names(&store, &source_id).is_empty());
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Backoff {
                error: CloudDocumentError::ProtocolFailed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn an_oversized_provider_page_enters_a_typed_backoff_before_staging() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let names = (0..257)
            .map(|index| format!("row-{index}.md"))
            .collect::<Vec<_>>();
        let rows = names
            .iter()
            .map(|name| sync_metadata(name))
            .collect::<Vec<_>>();
        let provider = Arc::new(SyncFakeProvider::scripted(
            "budget-source",
            b"c0",
            [Ok(MarkdownMetadataPage {
                rows,
                next_cursor: None,
                observed_at_ms: 1,
            })],
            [],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator.activate(provider).await.expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("budget failure settles");

        assert!(published_names(&store, &source_id).is_empty());
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Backoff {
                error: CloudDocumentError::TooLarge,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_metadata_page_over_one_mebibyte_is_rejected_even_below_the_row_limit() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let mut oversized = sync_metadata("byte-budget.md");
        oversized.name = "m".repeat(1024 * 1024);
        let provider = Arc::new(SyncFakeProvider::scripted(
            "page-byte-budget-source",
            b"c0",
            [Ok(MarkdownMetadataPage {
                rows: vec![oversized],
                next_cursor: None,
                observed_at_ms: 1,
            })],
            [],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator.activate(provider).await.expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("byte budget failure settles");

        assert!(published_names(&store, &source_id).is_empty());
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Backoff {
                error: CloudDocumentError::TooLarge,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn more_than_256_advancing_metadata_pages_fail_before_publication() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let pages = (0..256)
            .map(|index| {
                Ok(MarkdownMetadataPage {
                    rows: Vec::new(),
                    next_cursor: Some(ProviderCursor(format!("page-{index}").into_bytes())),
                    observed_at_ms: 1,
                })
            })
            .collect::<Vec<_>>();
        let provider = Arc::new(SyncFakeProvider::scripted(
            "page-count-budget-source",
            b"c0",
            pages,
            [],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("page count failure settles");

        assert_eq!(provider.list_calls.load(Ordering::SeqCst), 256);
        assert!(published_names(&store, &source_id).is_empty());
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Backoff {
                error: CloudDocumentError::TooLarge,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn individually_valid_pages_cannot_exceed_the_cumulative_sixteen_mebibyte_budget() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let large_name = "m".repeat(900 * 1024);
        let pages = (0..19)
            .map(|index| {
                let mut metadata = sync_metadata("cumulative-byte-budget.md");
                metadata.name = large_name.clone();
                Ok(MarkdownMetadataPage {
                    rows: vec![metadata],
                    next_cursor: (index < 18)
                        .then(|| ProviderCursor(format!("page-{index}").into_bytes())),
                    observed_at_ms: 1,
                })
            })
            .collect::<Vec<_>>();
        let provider = Arc::new(SyncFakeProvider::scripted(
            "cumulative-byte-budget-source",
            b"c0",
            pages,
            [],
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate(Arc::clone(&provider) as Arc<dyn MarkdownDocumentSource>)
            .await
            .expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("cumulative byte failure settles");

        assert_eq!(provider.list_calls.load(Ordering::SeqCst), 19);
        assert!(published_names(&store, &source_id).is_empty());
        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Backoff {
                error: CloudDocumentError::TooLarge,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn startup_repair_discards_staging_without_provider_io_or_losing_a_publication() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store = Arc::new(CloudDocumentStore::open(&path).expect("open cloud store"));
        let source_id = SourceId::new("startup-source");
        publish_before_sync(&store, &source_id, "published.md");
        let abandoned = store
            .begin_initial_sync(
                &source_id,
                &ProviderCursor(b"staging".to_vec()),
                &sync_pins(),
                3,
            )
            .expect("begin abandoned stage");
        store
            .stage_page(&abandoned, &sync_pins(), &sync_page(&["staged.md"], None))
            .expect("stage abandoned row");
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .startup_repair(Vec::<SourceId>::new(), &PendingRepairExecutor)
            .await
            .expect("startup repair");

        assert_eq!(published_names(&store, &source_id), ["published.md"]);
        let connection = rusqlite::Connection::open(&path).expect("inspect SQLite state");
        let staging: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mdreader_cloud_snapshot WHERE state = 'staging'",
                [],
                |row| row.get(0),
            )
            .expect("count staging snapshots");
        assert_eq!(staging, 0);
    }

    #[tokio::test]
    async fn startup_repair_resumes_a_provider_accepted_receipt_once_without_a_provider_mutation() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let source_id = SourceId::new("restart-repair-source");
        let repair = super::super::store::CloudWriteRepair::new(
            b"restart-operation",
            &source_id,
            SourceRecordKey::new("repaired.md"),
            ProviderRevision {
                opaque: "provider-r2".to_string(),
                content_hash: None,
            },
            [7; 32],
            br#"{\"provider\":\"accepted\"}"#,
            4,
        )
        .expect("valid provider receipt");
        let store = CloudDocumentStore::open(&path).expect("open initial store");
        store
            .record_provider_acceptance(&repair)
            .expect("persist provider acceptance");
        drop(store);

        let reopened = Arc::new(CloudDocumentStore::open(&path).expect("reopen after restart"));
        let coordinator = coordinator(Arc::clone(&reopened));
        let executor = RecordingRepairExecutor::new();

        coordinator
            .startup_repair([source_id.clone()], &executor)
            .await
            .expect("resume local publication");
        coordinator
            .startup_repair([source_id.clone()], &executor)
            .await
            .expect("second startup is idempotent");

        assert_eq!(executor.local_publications(), 1);
        assert_eq!(executor.provider_mutations(), 0);
        assert!(reopened
            .load_write_repair(&source_id, repair.operation_id())
            .expect("load repaired receipt")
            .expect("durable receipt")
            .is_published());
    }

    #[tokio::test]
    async fn startup_repair_keeps_a_deferred_receipt_and_exposes_its_typed_status() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let source_id = SourceId::new("pending-repair-source");
        let repair = super::super::store::CloudWriteRepair::new(
            b"pending-operation",
            &source_id,
            SourceRecordKey::new("pending.md"),
            ProviderRevision {
                opaque: "provider-r2".to_string(),
                content_hash: None,
            },
            [8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            4,
        )
        .expect("valid provider receipt");
        let store = Arc::new(CloudDocumentStore::open(&path).expect("open cloud store"));
        store
            .record_provider_acceptance(&repair)
            .expect("persist provider acceptance");
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .startup_repair([source_id.clone()], &PendingRepairExecutor)
            .await
            .expect("bounded pending repair");

        assert!(store
            .load_write_repair(&source_id, repair.operation_id())
            .expect("load pending receipt")
            .expect("durable receipt")
            .is_provider_accepted());
        assert!(matches!(
            coordinator.status(&source_id).expect("typed status"),
            Some(status)
                if matches!(status.phase(), CloudSyncPhase::NeverSynced)
                    && matches!(
                        status.startup_repair_error(),
                        Some(CloudDocumentError::LocalPublicationPending)
                    )
        ));
    }

    #[tokio::test]
    async fn successful_startup_repair_clears_an_older_pending_status_for_the_same_source() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let source_id = SourceId::new("cleared-pending-repair-source");
        let repair = super::super::store::CloudWriteRepair::new(
            b"cleared-pending-operation",
            &source_id,
            SourceRecordKey::new("cleared-pending.md"),
            ProviderRevision {
                opaque: "provider-r2".to_string(),
                content_hash: None,
            },
            [10; 32],
            br#"{\"provider\":\"accepted\"}"#,
            4,
        )
        .expect("valid provider receipt");
        let store = Arc::new(CloudDocumentStore::open(&path).expect("open cloud store"));
        store
            .record_provider_acceptance(&repair)
            .expect("persist provider acceptance");
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .startup_repair([source_id.clone()], &PendingRepairExecutor)
            .await
            .expect("record pending repair status");
        assert!(matches!(
            coordinator.status(&source_id).expect("pending status"),
            Some(status)
                if matches!(
                    status.startup_repair_error(),
                    Some(CloudDocumentError::LocalPublicationPending)
                )
        ));

        let executor = RecordingRepairExecutor::new();
        coordinator
            .startup_repair([source_id.clone()], &executor)
            .await
            .expect("resolve the same durable receipt");

        assert_eq!(executor.local_publications(), 1);
        assert!(store
            .load_write_repair(&source_id, repair.operation_id())
            .expect("load repaired receipt")
            .expect("durable receipt")
            .is_published());
        assert_eq!(
            coordinator.status(&source_id).expect("cleared status"),
            None,
            "a later successful repair must not leave stale pending status"
        );
    }

    #[tokio::test]
    async fn an_older_startup_repair_cannot_clear_a_newer_source_cancellation_owner() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let source_id = SourceId::new("overlapping-repair-source");
        let repair = super::super::store::CloudWriteRepair::new(
            b"overlapping-operation",
            &source_id,
            SourceRecordKey::new("overlapping.md"),
            ProviderRevision {
                opaque: "provider-r2".to_string(),
                content_hash: None,
            },
            [9; 32],
            br#"{\"provider\":\"accepted\"}"#,
            4,
        )
        .expect("valid provider receipt");
        let store = Arc::new(CloudDocumentStore::open(&path).expect("open cloud store"));
        store
            .record_provider_acceptance(&repair)
            .expect("persist provider acceptance");
        let coordinator = coordinator(Arc::clone(&store));
        let executor = Arc::new(BlockingRepairExecutor::new());

        let first_entered = executor.entered.notified();
        let first_coordinator = coordinator.clone();
        let first_source_id = source_id.clone();
        let first_executor = Arc::clone(&executor);
        let first = tokio::spawn(async move {
            first_coordinator
                .startup_repair([first_source_id], first_executor.as_ref())
                .await
        });
        first_entered.await;

        let second_entered = executor.entered.notified();
        let second_coordinator = coordinator.clone();
        let second_source_id = source_id.clone();
        let second_executor = Arc::clone(&executor);
        let second = tokio::spawn(async move {
            second_coordinator
                .startup_repair([second_source_id], second_executor.as_ref())
                .await
        });
        second_entered.await;
        first
            .await
            .expect("first startup task joins")
            .expect("first startup cancellation is retained as a typed status");

        assert!(matches!(
            coordinator.pause(&source_id).await,
            Err(super::super::CloudSyncError::UnknownSource)
        ));
        executor.release.notify_waiters();
        second
            .await
            .expect("second startup task joins")
            .expect("second startup cancellation is retained as a typed status");

        assert!(store
            .load_write_repair(&source_id, repair.operation_id())
            .expect("load unresolved receipt")
            .expect("durable receipt")
            .is_provider_accepted());
        assert!(matches!(
            coordinator.status(&source_id).expect("typed status"),
            Some(status)
                if matches!(status.startup_repair_error(), Some(CloudDocumentError::SourcePaused))
        ));
    }

    #[tokio::test]
    async fn retry_status_preserves_the_typed_provider_error_and_retry_after() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let provider = Arc::new(SyncFakeProvider::failing_start(
            "rate-limited-source",
            CloudDocumentError::RateLimited {
                retry_after_ms: Some(321),
            },
        ));
        let source_id = provider.source_id().clone();
        let coordinator = coordinator(store);
        let before_ms = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("system clock")
            .as_millis() as i64;

        coordinator.activate(provider).await.expect("activate sync");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("rate limit settles");

        assert!(matches!(
            coordinator.status(&source_id).expect("status"),
            Some(status) if matches!(status.phase(), CloudSyncPhase::Backoff {
                error: CloudDocumentError::RateLimited { retry_after_ms: Some(321) },
                retry_at_ms,
            } if *retry_at_ms >= before_ms + 321)
        ));
    }

    #[tokio::test]
    async fn distinct_source_owners_publish_with_their_own_immutable_pins() {
        let directory = tempfile::tempdir().expect("temporary SQLite directory");
        let store = Arc::new(
            CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
                .expect("open cloud store"),
        );
        let first = Arc::new(SyncFakeProvider::scripted(
            "pinned-first",
            b"first-cursor",
            [Ok(sync_page(&["first.md"], None))],
            [Ok(MarkdownChangePage {
                changes: Vec::new(),
                next_cursor: ProviderCursor(b"first-next".to_vec()),
                has_more: false,
                observed_at_ms: 2,
            })],
        ));
        let second = Arc::new(SyncFakeProvider::scripted(
            "pinned-second",
            b"second-cursor",
            [Ok(sync_page(&["second.md"], None))],
            [Ok(MarkdownChangePage {
                changes: Vec::new(),
                next_cursor: ProviderCursor(b"second-next".to_vec()),
                has_more: false,
                observed_at_ms: 2,
            })],
        ));
        let first_pins = sync_pins();
        let second_pins = super::super::store::CloudDocumentVersionPins::new(
            b"schema-v2",
            b"mapping-v2",
            b"source-v2",
        )
        .expect("second fixed pins");
        let first_id = first.source_id().clone();
        let second_id = second.source_id().clone();
        let coordinator = coordinator(Arc::clone(&store));

        coordinator
            .activate_with_pins(
                Arc::clone(&first) as Arc<dyn MarkdownDocumentSource>,
                first_pins.clone(),
            )
            .await
            .expect("activate first source");
        coordinator
            .activate_with_pins(
                Arc::clone(&second) as Arc<dyn MarkdownDocumentSource>,
                second_pins.clone(),
            )
            .await
            .expect("activate second source");
        coordinator
            .wait_until_idle(&first_id)
            .await
            .expect("first source settles");
        coordinator
            .wait_until_idle(&second_id)
            .await
            .expect("second source settles");

        assert_eq!(
            store.list_published(&first_id).expect("first row")[0].pins,
            first_pins
        );
        assert_eq!(
            store.list_published(&second_id).expect("second row")[0].pins,
            second_pins
        );
    }
}

#[cfg(feature = "google-drive-connector")]
#[tokio::test]
async fn read_only_v1_refuses_even_with_accepted_native_provider_state() {
    let lifecycle = TestDocumentLifecycle::active();
    let operations = Arc::new(TestDocumentOperations::new("drive-test-source"));
    let source = DriveMarkdownDocumentSource::with_test_operations(
        operations.clone(),
        Arc::new(lifecycle),
        CloudAuthorizationProfile::ReadOnlyV1,
        NativeMarkdownWriteAcceptance::Accepted,
    );

    let result = source
        .write_document(
            &SourceRecordKey::new("record"),
            &expected_revision(),
            b"# edit",
            &cloud_context(),
        )
        .await;

    assert!(matches!(result, Err(CloudDocumentError::ReconnectRequired)));
    assert!(operations.mutations().is_empty());
}

#[cfg(feature = "google-drive-connector")]
#[tokio::test]
async fn drive_v2_pending_and_accepted_states_have_distinct_provider_effects() {
    let pending_operations = Arc::new(TestDocumentOperations::new("drive-pending"));
    let pending = DriveMarkdownDocumentSource::with_test_operations(
        pending_operations.clone(),
        Arc::new(TestDocumentLifecycle::active()),
        CloudAuthorizationProfile::MarkdownReadWriteV2,
        NativeMarkdownWriteAcceptance::Pending,
    );
    let pending_result = pending
        .write_document(
            &SourceRecordKey::new("record"),
            &expected_revision(),
            b"# edit",
            &cloud_context(),
        )
        .await;
    assert!(matches!(
        pending_result,
        Err(CloudDocumentError::WriteCapabilityPendingAcceptance)
    ));
    assert!(pending_operations.mutations().is_empty());

    let accepted_operations = Arc::new(TestDocumentOperations::new("drive-accepted"));
    let accepted = DriveMarkdownDocumentSource::with_test_operations(
        accepted_operations.clone(),
        Arc::new(TestDocumentLifecycle::active()),
        CloudAuthorizationProfile::MarkdownReadWriteV2,
        NativeMarkdownWriteAcceptance::Accepted,
    );
    let receipt = accepted
        .write_document(
            &SourceRecordKey::new("record"),
            &expected_revision(),
            b"# edit",
            &cloud_context(),
        )
        .await
        .expect("accepted Drive source publishes the guarded receipt");

    assert_eq!(receipt.key.as_str(), "provider-record");
    assert_eq!(receipt.revision.opaque, "provider-revision");
    assert_eq!(receipt.size_bytes, 6);
    assert_eq!(
        accepted_operations.mutations(),
        vec![TestMutation {
            key: "record".to_string(),
            revision: "expected-revision".to_string(),
            body_bytes: 6,
        }]
    );
}

#[cfg(feature = "dropbox-connector")]
#[tokio::test]
async fn dropbox_v2_pending_and_accepted_states_have_distinct_provider_effects() {
    let pending_operations = Arc::new(TestDocumentOperations::new("dropbox-pending"));
    let pending = DropboxMarkdownDocumentSource::with_test_operations(
        pending_operations.clone(),
        Arc::new(TestDocumentLifecycle::active()),
        CloudAuthorizationProfile::MarkdownReadWriteV2,
        NativeMarkdownWriteAcceptance::Pending,
    );
    let pending_result = pending
        .write_document(
            &SourceRecordKey::new("record"),
            &expected_revision(),
            b"# edit",
            &cloud_context(),
        )
        .await;
    assert!(matches!(
        pending_result,
        Err(CloudDocumentError::WriteCapabilityPendingAcceptance)
    ));
    assert!(pending_operations.mutations().is_empty());

    let accepted_operations = Arc::new(TestDocumentOperations::new("dropbox-accepted"));
    let accepted = DropboxMarkdownDocumentSource::with_test_operations(
        accepted_operations.clone(),
        Arc::new(TestDocumentLifecycle::active()),
        CloudAuthorizationProfile::MarkdownReadWriteV2,
        NativeMarkdownWriteAcceptance::Accepted,
    );
    let receipt = accepted
        .write_document(
            &SourceRecordKey::new("record"),
            &expected_revision(),
            b"# edit",
            &cloud_context(),
        )
        .await
        .expect("accepted Dropbox source publishes the guarded receipt");

    assert_eq!(receipt.key.as_str(), "provider-record");
    assert_eq!(receipt.revision.opaque, "provider-revision");
    assert_eq!(receipt.size_bytes, 6);
    assert_eq!(
        accepted_operations.mutations(),
        vec![TestMutation {
            key: "record".to_string(),
            revision: "expected-revision".to_string(),
            body_bytes: 6,
        }]
    );
}

#[cfg(feature = "google-drive-connector")]
#[tokio::test]
async fn adapter_refuses_before_the_provider_effect_when_lifecycle_authority_is_lost() {
    let lifecycle = TestDocumentLifecycle::active();
    lifecycle.pause();
    let operations = Arc::new(TestDocumentOperations::new("drive-pre-effect"));
    let source = DriveMarkdownDocumentSource::with_test_operations(
        operations.clone(),
        Arc::new(lifecycle.clone()),
        CloudAuthorizationProfile::MarkdownReadWriteV2,
        NativeMarkdownWriteAcceptance::Accepted,
    );

    let result = source
        .write_document(
            &SourceRecordKey::new("record"),
            &expected_revision(),
            b"# edit",
            &cloud_context(),
        )
        .await;

    assert!(matches!(result, Err(CloudDocumentError::SourcePaused)));
    assert_eq!(lifecycle.state(), TestLifecycleState::Paused);
    assert!(operations.mutations().is_empty());
}

#[cfg(feature = "dropbox-connector")]
#[tokio::test]
async fn adapter_loses_authority_after_effect_without_publishing_a_receipt() {
    let lifecycle = TestDocumentLifecycle::active();
    let operations = Arc::new(TestDocumentOperations::pausing_after_effect(
        "dropbox-post-effect",
        lifecycle.clone(),
    ));
    let source = DropboxMarkdownDocumentSource::with_test_operations(
        operations.clone(),
        Arc::new(lifecycle.clone()),
        CloudAuthorizationProfile::MarkdownReadWriteV2,
        NativeMarkdownWriteAcceptance::Accepted,
    );

    let result = source
        .write_document(
            &SourceRecordKey::new("record"),
            &expected_revision(),
            b"# edit",
            &cloud_context(),
        )
        .await;

    assert!(matches!(
        result,
        Err(CloudDocumentError::WriteOutcomeUnknown)
    ));
    assert_eq!(lifecycle.state(), TestLifecycleState::Paused);
    assert_eq!(
        operations.mutations(),
        vec![TestMutation {
            key: "record".to_string(),
            revision: "expected-revision".to_string(),
            body_bytes: 6,
        }]
    );
}

#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Clone)]
struct DriveUnknownWriteTransport {
    calls: Arc<Mutex<usize>>,
    mutation_dispatches: Arc<Mutex<usize>>,
}

#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[async_trait]
impl DriveTransport for DriveUnknownWriteTransport {
    async fn send(
        &self,
        request: DriveHttpRequest,
    ) -> Result<DriveHttpResponse, DriveTransportError> {
        if request.method() == DriveHttpMethod::Patch {
            *self
                .mutation_dispatches
                .lock()
                .expect("test transport lock") += 1;
        }
        let call = {
            let mut calls = self.calls.lock().expect("test transport lock");
            let call = *calls;
            *calls += 1;
            call
        };
        match call {
            0 => DriveHttpResponse::new(
                200,
                BTreeMap::from([("ETag".to_string(), "metadata-revision".to_string())]),
                serde_json::to_vec(&serde_json::json!({
                    "id": "record",
                    "name": "note.md",
                    "mimeType": "text/markdown",
                    "modifiedTime": "2026-07-20T12:34:56Z",
                    "parents": ["provider-root"],
                    "trashed": false
                }))
                .expect("test metadata"),
            ),
            1 => DriveHttpResponse::new(503, BTreeMap::new(), Vec::new()),
            _ => Err(DriveTransportError::InvalidResponse),
        }
    }
}

#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Clone)]
struct DropboxUnknownWriteTransport {
    calls: Arc<Mutex<usize>>,
    mutation_dispatches: Arc<Mutex<usize>>,
}

#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[async_trait]
impl DropboxTransport for DropboxUnknownWriteTransport {
    async fn send(
        &self,
        request: DropboxHttpRequest,
    ) -> Result<DropboxHttpResponse, DropboxTransportError> {
        if request.path() == "/2/files/upload" {
            *self
                .mutation_dispatches
                .lock()
                .expect("test transport lock") += 1;
        }
        let call = {
            let mut calls = self.calls.lock().expect("test transport lock");
            let call = *calls;
            *calls += 1;
            call
        };
        match call {
            0 => DropboxHttpResponse::new(
                200,
                BTreeMap::new(),
                serde_json::to_vec(&serde_json::json!({
                    ".tag": "file",
                    "name": "note.md",
                    "path_lower": "/knowledge/note.md",
                    "path_display": "/knowledge/note.md",
                    "id": "id:note.md",
                    "client_modified": "2026-07-20T12:34:56Z",
                    "server_modified": "2026-07-20T12:34:57Z",
                    "rev": "metadata-revision",
                    "size": 22,
                    "content_hash": "hash"
                }))
                .expect("test metadata"),
            ),
            1 => DropboxHttpResponse::new(503, BTreeMap::new(), Vec::new()),
            _ => Err(DropboxTransportError::InvalidResponse),
        }
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn provider_context() -> ReadContext {
    ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(1),
        8,
        8 * 1024,
        8,
        CancellationToken::new(),
    )
    .expect("valid test context")
}

#[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[tokio::test]
async fn provider_write_outcome_unknowns_map_to_the_closed_adapter_error() {
    let drive_calls = Arc::new(Mutex::new(0));
    let drive_mutation_dispatches = Arc::new(Mutex::new(0));
    let drive_transport = DriveUnknownWriteTransport {
        calls: drive_calls.clone(),
        mutation_dispatches: drive_mutation_dispatches.clone(),
    };
    let drive = DriveSource::open_with_transport(
        PinnedDriveSourceSpec::new(
            DriveSourceConfig::new(
                "drive-source",
                "mdreader",
                "Document",
                vec![DriveSelection::file("record")],
                DriveSecretReference::new("test-drive-secret").expect("test secret reference"),
            )
            .expect("test Drive configuration")
            .with_content_scope(DriveContentScope::Markdown),
            "test-schema",
        )
        .expect("test Drive spec"),
        DriveRuntimeSecrets::new(
            DriveAccessToken::new("test-access-token").expect("test access token"),
            DriveCursorKey::new([7; 32]),
        ),
        Arc::new(drive_transport),
    )
    .expect("test Drive source");
    let drive_error = drive
        .update_markdown(
            "record",
            &DriveRevision::new("expected-revision").expect("test revision"),
            b"# edit",
            &provider_context(),
        )
        .await
        .expect_err("post-dispatch Drive failure has an unknown write outcome");
    assert!(matches!(
        drive_error,
        DriveDocumentError::WriteOutcomeUnknown { .. }
    ));
    assert_eq!(
        map_drive_document_error(drive_error),
        CloudDocumentError::WriteOutcomeUnknown
    );
    assert_eq!(*drive_calls.lock().expect("test transport lock"), 2);
    assert_eq!(
        *drive_mutation_dispatches
            .lock()
            .expect("test transport lock"),
        1
    );

    let dropbox_calls = Arc::new(Mutex::new(0));
    let dropbox_mutation_dispatches = Arc::new(Mutex::new(0));
    let dropbox_transport = DropboxUnknownWriteTransport {
        calls: dropbox_calls.clone(),
        mutation_dispatches: dropbox_mutation_dispatches.clone(),
    };
    let dropbox = DropboxSource::open_with_transport(
        PinnedDropboxSourceSpec::new(
            DropboxSourceConfig::new(
                "dropbox-source",
                "mdreader",
                "Document",
                "/knowledge",
                DropboxAuthorizationMode::FullDropbox,
                DropboxSecretReference::new("test-dropbox-secret").expect("test secret reference"),
            )
            .expect("test Dropbox configuration")
            .with_content_scope(DropboxContentScope::Markdown),
            "test-schema",
        )
        .expect("test Dropbox spec"),
        DropboxRuntimeSecrets::new(
            DropboxAccessToken::new("test-access-token").expect("test access token"),
            DropboxCursorKey::new([9; 32]),
        ),
        Arc::new(dropbox_transport),
    )
    .expect("test Dropbox source");
    let dropbox_error = dropbox
        .update_markdown(
            "id:note.md",
            &DropboxRevision::new("expected-revision").expect("test revision"),
            b"# edit",
            &provider_context(),
        )
        .await
        .expect_err("post-dispatch Dropbox failure has an unknown write outcome");
    assert!(matches!(
        dropbox_error,
        DropboxDocumentError::WriteOutcomeUnknown { .. }
    ));
    assert_eq!(
        map_dropbox_document_error(dropbox_error),
        CloudDocumentError::WriteOutcomeUnknown
    );
    assert_eq!(*dropbox_calls.lock().expect("test transport lock"), 2);
    assert_eq!(
        *dropbox_mutation_dispatches
            .lock()
            .expect("test transport lock"),
        1
    );
}

#[cfg(feature = "google-drive-connector")]
#[test]
fn drive_adapter_maps_closed_document_failures_without_provider_detail() {
    let source_id = SourceId::new("drive-source");

    assert_eq!(
        map_drive_document_error(
            catdb_connector_google_drive::DriveDocumentError::Authentication {
                source_id: source_id.clone(),
            }
        ),
        CloudDocumentError::Unauthorized
    );
    assert_eq!(
        map_drive_document_error(catdb_connector_google_drive::DriveDocumentError::Conflict {
            source_id: source_id.clone(),
        }),
        CloudDocumentError::Conflict
    );
    assert_eq!(
        map_drive_document_error(catdb_connector_google_drive::DriveDocumentError::Source {
            error: SourceError::DeadlineExceeded,
        }),
        CloudDocumentError::Timeout
    );
    assert_eq!(
        map_drive_document_error(catdb_connector_google_drive::DriveDocumentError::Source {
            error: SourceError::Cancelled,
        }),
        CloudDocumentError::Cancelled
    );
}

#[cfg(feature = "dropbox-connector")]
#[test]
fn dropbox_adapter_maps_closed_document_failures_without_provider_detail() {
    let source_id = SourceId::new("dropbox-source");

    assert_eq!(
        map_dropbox_document_error(catdb_connector_dropbox::DropboxDocumentError::NotFound {
            source_id: source_id.clone(),
        }),
        CloudDocumentError::NotFound
    );
    assert_eq!(
        map_dropbox_document_error(catdb_connector_dropbox::DropboxDocumentError::InvalidUtf8 {
            source_id: source_id.clone(),
        }),
        CloudDocumentError::InvalidUtf8
    );
    assert_eq!(
        map_dropbox_document_error(catdb_connector_dropbox::DropboxDocumentError::RateLimited {
            source_id,
            retry_after_ms: Some(125),
        }),
        CloudDocumentError::RateLimited {
            retry_after_ms: Some(125),
        }
    );
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[test]
fn lifecycle_pause_and_replacement_fail_closed_at_the_adapter_boundary() {
    assert_eq!(
        map_lifecycle_error(CloudSourceError::SourcePaused),
        CloudDocumentError::SourcePaused
    );
    assert_eq!(
        map_lifecycle_error(CloudSourceError::QueryRetired),
        CloudDocumentError::SourceDisconnected
    );
}

#[cfg(any(
    all(feature = "google-drive-connector", not(feature = "dropbox-connector")),
    all(feature = "dropbox-connector", not(feature = "google-drive-connector"))
))]
#[test]
fn lifecycle_connector_unavailability_stays_distinct_at_the_adapter_boundary() {
    assert_eq!(
        map_lifecycle_error(CloudSourceError::ConnectorUnavailable),
        CloudDocumentError::ConnectorUnavailable
    );
}

mod store {
    use std::collections::HashSet;

    use tempfile::tempdir;

    use super::super::store::{
        CloudDocumentStore, CloudDocumentStoreError, CloudDocumentVersionPins, CloudWriteRepair,
    };
    use super::super::{
        MarkdownDocumentRevision, MarkdownMetadata, MarkdownMetadataPage, ProviderCursor,
        ProviderRevision, SourceRecordKey,
    };
    use catdb_connectors::SourceId;

    fn source_id() -> SourceId {
        SourceId::new("cloud-store-test-source")
    }

    fn cursor(value: &[u8]) -> ProviderCursor {
        ProviderCursor(value.to_vec())
    }

    fn pins() -> CloudDocumentVersionPins {
        CloudDocumentVersionPins::new(b"schema-v1", b"mapping-v1", b"source-v1")
            .expect("valid fixed version pins")
    }

    fn page(names: &[&str]) -> MarkdownMetadataPage {
        MarkdownMetadataPage {
            rows: names
                .iter()
                .map(|name| MarkdownMetadata {
                    key: SourceRecordKey::new(*name),
                    name: (*name).to_owned(),
                    display_path: format!("/documents/{name}"),
                    media_type: Some("text/markdown".to_owned()),
                    modified_at_ms: Some(1),
                    size_bytes: Some(1),
                    revision: ProviderRevision {
                        opaque: format!("revision-{name}"),
                        content_hash: None,
                    },
                    deleted: false,
                })
                .collect(),
            next_cursor: None,
            observed_at_ms: 1,
        }
    }

    fn page_with_revisions(rows: &[(&str, &str)]) -> MarkdownMetadataPage {
        MarkdownMetadataPage {
            rows: rows
                .iter()
                .map(|(name, revision)| MarkdownMetadata {
                    key: SourceRecordKey::new(*name),
                    name: (*name).to_owned(),
                    display_path: format!("/documents/{name}"),
                    media_type: Some("text/markdown".to_owned()),
                    modified_at_ms: Some(1),
                    size_bytes: Some(1),
                    revision: ProviderRevision {
                        opaque: (*revision).to_owned(),
                        content_hash: None,
                    },
                    deleted: false,
                })
                .collect(),
            next_cursor: None,
            observed_at_ms: 1,
        }
    }

    fn tombstone_page(name: &str, revision: &str) -> MarkdownMetadataPage {
        let mut page = page_with_revisions(&[(name, revision)]);
        page.rows[0].deleted = true;
        page
    }

    fn publish_page(
        store: &CloudDocumentStore,
        source: &SourceId,
        page: &MarkdownMetadataPage,
        change_cursor: &[u8],
    ) {
        let generation = store
            .begin_initial_sync(source, &cursor(change_cursor), &pins(), 1)
            .expect("begin snapshot");
        store
            .stage_page(&generation, &pins(), page)
            .expect("stage rows");
        store
            .publish_initial_sync(&generation, &cursor(change_cursor), 2)
            .expect("publish snapshot");
    }

    fn document(
        name: &str,
        revision: &str,
        body: &[u8],
        observed_at_ms: i64,
    ) -> MarkdownDocumentRevision {
        MarkdownDocumentRevision {
            key: SourceRecordKey::new(name),
            revision: ProviderRevision {
                opaque: revision.to_owned(),
                content_hash: None,
            },
            body: body.to_vec(),
            observed_at_ms,
        }
    }

    #[test]
    fn incomplete_initial_sync_never_replaces_the_published_snapshot() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud-materializations.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        let old_generation = store
            .begin_initial_sync(&source, &cursor(b"before"), &pins(), 1)
            .expect("begin old snapshot");
        store
            .stage_page(&old_generation, &pins(), &page(&["old.md"]))
            .expect("stage old row");
        store
            .publish_initial_sync(&old_generation, &cursor(b"before"), 2)
            .expect("publish old snapshot");

        let incomplete_generation = store
            .begin_initial_sync(&source, &cursor(b"before"), &pins(), 3)
            .expect("begin replacement snapshot");
        store
            .stage_page(&incomplete_generation, &pins(), &page(&["new-a.md"]))
            .expect("stage incomplete row");
        drop(store);

        let reopened = CloudDocumentStore::open(&path).expect("reopen store");
        let published = reopened.list_published(&source).expect("published rows");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].name, "old.md");
    }

    #[test]
    fn replacing_an_incomplete_initial_sync_discards_its_staging_generation() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud-materializations.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        let abandoned = store
            .begin_initial_sync(&source, &cursor(b"before"), &pins(), 1)
            .expect("begin abandoned sync");
        store
            .stage_page(&abandoned, &pins(), &page(&["partial.md"]))
            .expect("stage abandoned row");
        store
            .begin_initial_sync(&source, &cursor(b"before"), &pins(), 2)
            .expect("replace incomplete sync");

        let connection = rusqlite::Connection::open(&path).expect("inspect staging snapshots");
        let staging_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mdreader_cloud_snapshot
                 WHERE source_id = ?1 AND state = 'staging'",
                [source.as_str().as_bytes()],
                |row| row.get(0),
            )
            .expect("count staging snapshots");
        assert_eq!(staging_count, 1);
    }

    #[test]
    fn incremental_page_and_cursor_commit_together() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud-materializations.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        let generation = store
            .begin_initial_sync(&source, &cursor(b"previous"), &pins(), 1)
            .expect("begin snapshot");
        store
            .stage_page(&generation, &pins(), &page(&["before.md"]))
            .expect("stage row");
        store
            .publish_initial_sync(&generation, &cursor(b"previous"), 2)
            .expect("publish snapshot");
        store.inject_commit_failure_after_rows();

        assert!(store
            .apply_change_page(&source, &pins(), &page(&["after.md"]), &cursor(b"next"), 3)
            .is_err());
        assert_eq!(
            store
                .load_change_cursor(&source)
                .expect("cursor after failure"),
            Some(cursor(b"previous"))
        );
        let published = store.list_published(&source).expect("published rows");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].name, "before.md");
    }

    #[test]
    fn incremental_page_reuses_the_published_generation_without_copying_unchanged_metadata() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud-materializations.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        publish_page(&store, &source, &page(&["unchanged.md"]), b"previous");

        store
            .apply_change_page(
                &source,
                &pins(),
                &page(&["changed.md"]),
                &cursor(b"next"),
                3,
            )
            .expect("apply incremental page");
        let connection = rusqlite::Connection::open(&path).expect("inspect materializations");
        let snapshot_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mdreader_cloud_snapshot WHERE source_id = ?1",
                [source.as_str().as_bytes()],
                |row| row.get(0),
            )
            .expect("count source snapshots");
        let unchanged_row_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mdreader_cloud_document WHERE record_key = ?1",
                [b"unchanged.md".as_slice()],
                |row| row.get(0),
            )
            .expect("count unchanged materialization rows");
        assert_eq!(snapshot_count, 1);
        assert_eq!(unchanged_row_count, 1);
        let published = store.list_published(&source).expect("published rows");
        assert_eq!(published.len(), 2);
        assert!(published.iter().any(|row| row.name == "unchanged.md"));
        assert!(published.iter().any(|row| row.name == "changed.md"));
    }

    #[test]
    fn rejects_empty_cursor_and_revision_without_publishing_any_rows() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();

        assert!(matches!(
            store.begin_initial_sync(&source, &cursor(b""), &pins(), 1),
            Err(CloudDocumentStoreError::InvalidCursor)
        ));

        let generation = store
            .begin_initial_sync(&source, &cursor(b"cursor"), &pins(), 1)
            .expect("begin valid snapshot");
        let invalid = page_with_revisions(&[("note.md", "")]);
        assert!(matches!(
            store.stage_page(&generation, &pins(), &invalid),
            Err(CloudDocumentStoreError::InvalidRevision)
        ));
        assert!(store
            .list_published(&source)
            .expect("published rows")
            .is_empty());
    }

    #[test]
    fn foreign_source_cannot_change_a_published_snapshot() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        publish_page(&store, &source, &page(&["kept.md"]), b"previous");
        let foreign = SourceId::new("different-cloud-source");

        assert!(matches!(
            store.apply_change_page(
                &foreign,
                &pins(),
                &page(&["injected.md"]),
                &cursor(b"next"),
                3,
            ),
            Err(CloudDocumentStoreError::ForeignSource)
        ));
        let published = store.list_published(&source).expect("published rows");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].name, "kept.md");
    }

    #[test]
    fn interrupted_migration_refuses_to_guess_at_partial_schema() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let connection = rusqlite::Connection::open(&path).expect("open partial database");
        connection
            .execute_batch(
                "CREATE TABLE mdreader_cloud_snapshot (source_id BLOB NOT NULL); PRAGMA user_version = 0;",
            )
            .expect("manufacture interrupted migration");
        drop(connection);

        assert!(matches!(
            CloudDocumentStore::open(&path),
            Err(CloudDocumentStoreError::MigrationInterrupted)
        ));
    }

    #[test]
    fn cloud_tables_share_a_catalog_without_reading_or_replacing_its_schema_marker() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud-library.catdb");
        let connection = rusqlite::Connection::open(&path).expect("open catalog database");
        connection
            .execute_batch(
                "CREATE TABLE catdb_catalog_sentinel (id INTEGER PRIMARY KEY);
                 PRAGMA user_version = 73;",
            )
            .expect("seed foreign catalog schema marker");
        drop(connection);

        CloudDocumentStore::open(&path).expect("open cloud tables alongside catalog");

        let connection = rusqlite::Connection::open(&path).expect("inspect shared database");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("catalog schema marker");
        let cloud_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name GLOB 'mdreader_cloud_*'",
                [],
                |row| row.get(0),
            )
            .expect("cloud table count");
        assert_eq!(version, 73, "cloud storage never owns the catalog marker");
        assert_eq!(cloud_tables, 5);
    }

    #[test]
    fn a_partial_cloud_schema_is_rejected_without_mutating_a_foreign_catalog_marker() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud-library.catdb");
        let connection = rusqlite::Connection::open(&path).expect("open partial database");
        connection
            .execute_batch(
                "CREATE TABLE mdreader_cloud_snapshot (source_id BLOB NOT NULL);
                 PRAGMA user_version = 73;",
            )
            .expect("manufacture interrupted cloud schema");
        drop(connection);

        assert!(matches!(
            CloudDocumentStore::open(&path),
            Err(CloudDocumentStoreError::MigrationInterrupted)
        ));
        let version: i64 = rusqlite::Connection::open(&path)
            .expect("inspect shared database")
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("catalog schema marker");
        assert_eq!(version, 73, "refusal must not alter the catalog marker");
    }

    #[test]
    fn cloud_schema_with_correct_columns_but_missing_load_bearing_shape_is_refused() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let connection = rusqlite::Connection::open(&path).expect("open corrupt schema database");
        connection
            .execute_batch(
                "CREATE TABLE mdreader_cloud_snapshot (
                    source_id TEXT,
                    generation BLOB,
                    state TEXT,
                    observed_at_ms INTEGER,
                    schema_version_id BLOB,
                    mapping_version_id BLOB,
                    source_version_id BLOB
                 );
                 CREATE INDEX mdreader_cloud_one_published_snapshot
                    ON mdreader_cloud_snapshot (source_id);
                 CREATE TABLE mdreader_cloud_document (
                    source_id BLOB,
                    generation BLOB,
                    record_key BLOB,
                    name TEXT,
                    display_path TEXT,
                    media_type TEXT,
                    modified_at_ms INTEGER,
                    size_bytes INTEGER,
                    revision BLOB,
                    content_hash BLOB,
                    deleted INTEGER,
                    schema_version_id BLOB,
                    mapping_version_id BLOB,
                    source_version_id BLOB,
                    observed_at_ms INTEGER
                 );
                 CREATE INDEX mdreader_cloud_document_published
                    ON mdreader_cloud_document (source_id, generation, display_path, record_key);
                 CREATE TABLE mdreader_cloud_sync_state (
                    source_id BLOB,
                    change_cursor BLOB,
                    sync_state TEXT,
                    last_success_ms INTEGER,
                    last_error_kind TEXT,
                    retry_after_ms INTEGER
                 );
                 CREATE TABLE mdreader_cloud_body_cache (
                    source_id BLOB,
                    source_version_id BLOB,
                    record_key BLOB,
                    revision BLOB,
                    body BLOB,
                    body_hash BLOB,
                    size_bytes INTEGER,
                    last_access_ms INTEGER
                 );
                 CREATE INDEX mdreader_cloud_body_cache_lru
                    ON mdreader_cloud_body_cache (body_hash);
                 CREATE TABLE mdreader_cloud_write_repair (
                    operation_id BLOB,
                    source_id BLOB,
                    record_key BLOB,
                    accepted_revision BLOB,
                    body_hash BLOB,
                    receipt_json BLOB,
                    state TEXT,
                    created_at_ms INTEGER
                 );
                 PRAGMA user_version = 1;",
            )
            .expect("manufacture version-one schema with correct column names only");
        drop(connection);

        assert!(matches!(
            CloudDocumentStore::open(&path),
            Err(CloudDocumentStoreError::CorruptStore)
        ));
    }

    #[test]
    fn staging_generation_refuses_rows_with_different_version_pins() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        let generation = store
            .begin_initial_sync(&source, &cursor(b"cursor"), &pins(), 1)
            .expect("begin snapshot");
        store
            .stage_page(&generation, &pins(), &page(&["first.md"]))
            .expect("stage first pinned page");
        let different_pins =
            CloudDocumentVersionPins::new(b"schema-v2", b"mapping-v1", b"source-v1")
                .expect("valid differing pins");

        assert!(matches!(
            store.stage_page(&generation, &different_pins, &page(&["second.md"])),
            Err(CloudDocumentStoreError::VersionPinMismatch)
        ));
    }

    #[test]
    fn cache_evicts_the_least_recently_used_body_deterministically() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store =
            CloudDocumentStore::open_with_cache_limit_for_tests(&path, 7).expect("open store");
        let source = source_id();
        publish_page(
            &store,
            &source,
            &page_with_revisions(&[("one.md", "r1"), ("two.md", "r1"), ("three.md", "r1")]),
            b"cursor",
        );

        store
            .put_body(&source, &pins(), &document("one.md", "r1", b"111", 10))
            .expect("cache first body");
        store
            .put_body(&source, &pins(), &document("two.md", "r1", b"222", 20))
            .expect("cache second body");
        assert_eq!(
            store
                .get_body(
                    &source,
                    &pins(),
                    &SourceRecordKey::new("one.md"),
                    &ProviderRevision {
                        opaque: "r1".to_owned(),
                        content_hash: None,
                    },
                    30
                )
                .expect("read most recent body"),
            Some(b"111".to_vec())
        );
        store
            .put_body(&source, &pins(), &document("three.md", "r1", b"333", 40))
            .expect("cache third body and evict LRU");

        assert_eq!(
            store
                .get_body(
                    &source,
                    &pins(),
                    &SourceRecordKey::new("two.md"),
                    &ProviderRevision {
                        opaque: "r1".to_owned(),
                        content_hash: None,
                    },
                    50
                )
                .expect("query evicted body"),
            None
        );
        assert_eq!(
            store
                .get_body(
                    &source,
                    &pins(),
                    &SourceRecordKey::new("one.md"),
                    &ProviderRevision {
                        opaque: "r1".to_owned(),
                        content_hash: None,
                    },
                    50
                )
                .expect("query retained body"),
            Some(b"111".to_vec())
        );
    }

    #[test]
    fn metadata_survives_body_eviction_and_cache_keeps_two_revisions_of_one_record() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store =
            CloudDocumentStore::open_with_cache_limit_for_tests(&path, 4).expect("open store");
        let source = source_id();
        publish_page(
            &store,
            &source,
            &page_with_revisions(&[("versioned.md", "r2"), ("evicted.md", "r1")]),
            b"cursor",
        );

        store
            .put_body(&source, &pins(), &document("versioned.md", "r1", b"a", 10))
            .expect("cache first revision");
        store
            .put_body(&source, &pins(), &document("versioned.md", "r2", b"b", 20))
            .expect("cache second revision");
        let connection = rusqlite::Connection::open(&path).expect("inspect exact revision cache");
        let revision_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mdreader_cloud_body_cache
                 WHERE record_key = ?1",
                [b"versioned.md".as_slice()],
                |row| row.get(0),
            )
            .expect("count independently cached revisions");
        assert_eq!(revision_count, 2);
        drop(connection);
        store
            .put_body(&source, &pins(), &document("evicted.md", "r1", b"cccc", 30))
            .expect("evict old revisions under tight cache limit");

        let published = store.list_published(&source).expect("published metadata");
        assert_eq!(published.len(), 2);
        assert!(published.iter().any(|row| row.name == "versioned.md"));
        assert!(published.iter().any(|row| row.name == "evicted.md"));
        assert_eq!(
            store
                .get_body(
                    &source,
                    &pins(),
                    &SourceRecordKey::new("versioned.md"),
                    &ProviderRevision {
                        opaque: "r1".to_owned(),
                        content_hash: None,
                    },
                    40
                )
                .expect("query evicted revision"),
            None
        );
    }

    #[test]
    fn opaque_source_keys_revisions_and_cursors_are_bound_as_sqlite_blobs() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        publish_page(
            &store,
            &source,
            &page_with_revisions(&[("note.md", "r1")]),
            b"cursor",
        );
        drop(store);
        let connection = rusqlite::Connection::open(&path).expect("inspect materialization types");
        let (snapshot_source, document_key, document_revision, change_cursor): (
            String,
            String,
            String,
            String,
        ) = connection
            .query_row(
                "SELECT typeof(s.source_id), typeof(d.record_key), typeof(d.revision),
                    typeof(state.change_cursor)
                 FROM mdreader_cloud_snapshot s
                 INNER JOIN mdreader_cloud_document d
                   ON d.source_id = s.source_id AND d.generation = s.generation
                 INNER JOIN mdreader_cloud_sync_state state ON state.source_id = s.source_id
                 WHERE s.state = 'published'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read SQLite storage types");
        assert_eq!(snapshot_source, "blob");
        assert_eq!(document_key, "blob");
        assert_eq!(document_revision, "blob");
        assert_eq!(change_cursor, "blob");
    }

    #[test]
    fn oversized_bodies_are_rejected_before_the_cache_changes() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        publish_page(
            &store,
            &source,
            &page_with_revisions(&[("large.md", "r1")]),
            b"cursor",
        );
        let body = vec![0_u8; CloudDocumentStore::MAX_BODY_BYTES + 1];

        assert!(matches!(
            store.put_body(&source, &pins(), &document("large.md", "r1", &body, 1)),
            Err(CloudDocumentStoreError::BodyTooLarge)
        ));
        assert_eq!(
            store
                .get_body(
                    &source,
                    &pins(),
                    &SourceRecordKey::new("large.md"),
                    &ProviderRevision {
                        opaque: "r1".to_owned(),
                        content_hash: None,
                    },
                    2
                )
                .expect("query cache"),
            None
        );
    }

    #[test]
    fn cache_corruption_is_refused_without_returning_the_body() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        publish_page(
            &store,
            &source,
            &page_with_revisions(&[("note.md", "r1")]),
            b"cursor",
        );
        store
            .put_body(&source, &pins(), &document("note.md", "r1", b"body", 1))
            .expect("cache body");
        let connection = rusqlite::Connection::open(&path).expect("open database for corruption");
        connection
            .execute(
                "UPDATE mdreader_cloud_body_cache SET body_hash = zeroblob(32)",
                [],
            )
            .expect("corrupt cache row");
        drop(connection);

        assert!(matches!(
            store.get_body(
                &source,
                &pins(),
                &SourceRecordKey::new("note.md"),
                &ProviderRevision {
                    opaque: "r1".to_owned(),
                    content_hash: None,
                },
                2
            ),
            Err(CloudDocumentStoreError::CorruptStore)
        ));
    }

    #[test]
    fn tombstoned_metadata_never_returns_or_admits_a_cached_body() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        publish_page(
            &store,
            &source,
            &page_with_revisions(&[("removed.md", "r1")]),
            b"cursor",
        );
        store
            .put_body(&source, &pins(), &document("removed.md", "r1", b"body", 1))
            .expect("cache body before deletion");
        store
            .apply_change_page(
                &source,
                &pins(),
                &tombstone_page("removed.md", "r1"),
                &cursor(b"after-tombstone"),
                2,
            )
            .expect("apply tombstone");

        assert_eq!(
            store
                .get_body(
                    &source,
                    &pins(),
                    &SourceRecordKey::new("removed.md"),
                    &ProviderRevision {
                        opaque: "r1".to_owned(),
                        content_hash: None,
                    },
                    3,
                )
                .expect("tombstoned cache lookup"),
            None
        );
        assert!(matches!(
            store.put_body(&source, &pins(), &document("removed.md", "r1", b"new", 4)),
            Err(CloudDocumentStoreError::ForeignSource)
        ));
    }

    #[test]
    fn negative_cached_size_cannot_offset_an_oversized_physical_cache() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store =
            CloudDocumentStore::open_with_cache_limit_for_tests(&path, 4).expect("open store");
        let source = source_id();
        publish_page(
            &store,
            &source,
            &page_with_revisions(&[("corrupt.md", "r1"), ("new.md", "r1")]),
            b"cursor",
        );
        store
            .put_body(&source, &pins(), &document("corrupt.md", "r1", b"aaa", 1))
            .expect("cache first body");
        let connection = rusqlite::Connection::open(&path).expect("open database for corruption");
        connection
            .execute_batch(
                "PRAGMA ignore_check_constraints = ON;
                 UPDATE mdreader_cloud_body_cache SET size_bytes = -2;
                 PRAGMA ignore_check_constraints = OFF;",
            )
            .expect("manufacture a negative stored size");
        drop(connection);

        assert!(matches!(
            store.put_body(&source, &pins(), &document("new.md", "r1", b"bbb", 2)),
            Err(CloudDocumentStoreError::CorruptStore)
        ));
    }

    #[test]
    fn provider_acceptance_receipt_survives_restart_until_publication_is_marked_durable() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let source = source_id();
        let repair = CloudWriteRepair::new(
            b"operation-1",
            &source,
            SourceRecordKey::new("note.md"),
            ProviderRevision {
                opaque: "accepted-r2".to_owned(),
                content_hash: None,
            },
            [7_u8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            9,
        )
        .expect("valid repair receipt");
        let store = CloudDocumentStore::open(&path).expect("open store");
        store
            .record_provider_acceptance(&repair)
            .expect("persist provider acceptance before derived publication");
        drop(store);

        let reopened = CloudDocumentStore::open(&path).expect("reopen store");
        let pending = reopened
            .load_write_repair(&source, repair.operation_id())
            .expect("load persisted receipt")
            .expect("receipt remains pending");
        assert!(pending.is_provider_accepted());
        reopened
            .mark_write_published(&source, repair.operation_id())
            .expect("mark derived publication durable");
        let durable = reopened
            .load_write_repair(&source, repair.operation_id())
            .expect("load durable receipt")
            .expect("receipt is retained after marking publication");
        assert!(durable.is_published());
        assert_eq!(durable.receipt_json(), br#"{\"provider\":\"accepted\"}"#);
    }

    #[test]
    fn pending_write_repairs_page_bounds_equal_timestamp_recovery_without_duplicates() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        let page_limit = CloudDocumentStore::MAX_WRITE_REPAIR_PAGE_ROWS;
        let mut expected = Vec::new();

        for index in 0..=page_limit {
            let operation_id = format!("operation-{index:03}");
            expected.push(operation_id.clone().into_bytes());
            let repair = CloudWriteRepair::new(
                operation_id.as_bytes(),
                &source,
                SourceRecordKey::new(format!("record-{index}")),
                ProviderRevision {
                    opaque: format!("revision-{index}"),
                    content_hash: None,
                },
                [(index % 251) as u8; 32],
                br#"{\"provider\":\"accepted\"}"#,
                9,
            )
            .expect("valid accepted repair");
            store
                .record_provider_acceptance(&repair)
                .expect("persist provider acceptance");
        }

        let first = store
            .pending_write_repairs_page(&source, None, page_limit)
            .expect("load first bounded repair page");
        assert_eq!(first.repairs().len(), page_limit);
        let after = first
            .next_continuation()
            .cloned()
            .expect("a full page with another row has a continuation");
        let second = store
            .pending_write_repairs_page(&source, Some(&after), page_limit)
            .expect("load second bounded repair page");
        assert_eq!(second.repairs().len(), 1);
        assert!(second.next_continuation().is_none());

        let observed = first
            .repairs()
            .iter()
            .chain(second.repairs())
            .map(|repair| repair.operation_id().to_vec())
            .collect::<Vec<_>>();
        let unique = observed.iter().cloned().collect::<HashSet<_>>();
        assert_eq!(unique.len(), expected.len());
        assert_eq!(observed, expected);
    }

    #[test]
    fn pending_write_repairs_page_rejects_invalid_limits_before_sql() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        let connection = rusqlite::Connection::open(&path).expect("open external connection");
        connection
            .execute_batch(
                "ALTER TABLE mdreader_cloud_write_repair RENAME TO unavailable_write_repairs;",
            )
            .expect("make any repair-page query fail");
        drop(connection);

        assert!(matches!(
            store.pending_write_repairs_page(&source, None, 0),
            Err(CloudDocumentStoreError::InvalidWriteRepairPageLimit)
        ));
        assert!(matches!(
            store.pending_write_repairs_page(
                &source,
                None,
                CloudDocumentStore::MAX_WRITE_REPAIR_PAGE_ROWS + 1
            ),
            Err(CloudDocumentStoreError::InvalidWriteRepairPageLimit)
        ));
        assert!(matches!(
            store.pending_write_repairs_page(&source, None, 1),
            Err(CloudDocumentStoreError::Persistence | CloudDocumentStoreError::CorruptStore)
        ));
    }

    #[test]
    fn pending_write_repairs_page_is_source_scoped() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        let foreign = SourceId::new("other-cloud-store-source");
        let owner_repair = CloudWriteRepair::new(
            b"owner-operation",
            &source,
            SourceRecordKey::new("owner.md"),
            ProviderRevision {
                opaque: "owner-revision".to_owned(),
                content_hash: None,
            },
            [1_u8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            9,
        )
        .expect("valid owner repair");
        let foreign_repair = CloudWriteRepair::new(
            b"foreign-operation",
            &foreign,
            SourceRecordKey::new("foreign.md"),
            ProviderRevision {
                opaque: "foreign-revision".to_owned(),
                content_hash: None,
            },
            [2_u8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            9,
        )
        .expect("valid foreign repair");
        store
            .record_provider_acceptance(&owner_repair)
            .expect("persist owner repair");
        store
            .record_provider_acceptance(&foreign_repair)
            .expect("persist foreign repair");

        let page = store
            .pending_write_repairs_page(&source, None, 1)
            .expect("load owner repair page");
        assert_eq!(page.repairs().len(), 1);
        assert_eq!(
            page.repairs()[0].operation_id(),
            owner_repair.operation_id()
        );
        assert!(page.next_continuation().is_none());
    }

    #[test]
    fn pending_write_repairs_page_rejects_a_foreign_source_continuation() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        let foreign = SourceId::new("other-cloud-store-source");

        for operation_id in [b"owner-operation-1".as_slice(), b"owner-operation-2"] {
            let repair = CloudWriteRepair::new(
                operation_id,
                &source,
                SourceRecordKey::new("owner.md"),
                ProviderRevision {
                    opaque: "owner-revision".to_owned(),
                    content_hash: None,
                },
                [1_u8; 32],
                br#"{\"provider\":\"accepted\"}"#,
                9,
            )
            .expect("valid owner repair");
            store
                .record_provider_acceptance(&repair)
                .expect("persist owner repair");
        }
        let foreign_repair = CloudWriteRepair::new(
            b"foreign-operation",
            &foreign,
            SourceRecordKey::new("foreign.md"),
            ProviderRevision {
                opaque: "foreign-revision".to_owned(),
                content_hash: None,
            },
            [2_u8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            9,
        )
        .expect("valid foreign repair");
        store
            .record_provider_acceptance(&foreign_repair)
            .expect("persist foreign repair");

        let owner_page = store
            .pending_write_repairs_page(&source, None, 1)
            .expect("load owner repair page");
        let owner_continuation = owner_page
            .next_continuation()
            .cloned()
            .expect("owner page has a continuation");
        assert!(matches!(
            store.pending_write_repairs_page(&foreign, Some(&owner_continuation), 1),
            Err(CloudDocumentStoreError::ForeignSource)
        ));
    }

    #[test]
    fn persisted_write_repair_debug_redacts_opaque_values() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = SourceId::new("source-sentinel");
        let body_hash_sentinel = [0xA5_u8; 32];
        let first_repair = CloudWriteRepair::new(
            b"operation-sentinel",
            &source,
            SourceRecordKey::new("record-sentinel"),
            ProviderRevision {
                opaque: "revision-sentinel".to_owned(),
                content_hash: None,
            },
            body_hash_sentinel,
            b"receipt-sentinel",
            9,
        )
        .expect("valid sentinel repair");
        store
            .record_provider_acceptance(&first_repair)
            .expect("persist first sentinel repair");

        let persisted = store
            .load_write_repair(&source, first_repair.operation_id())
            .expect("load sentinel repair")
            .expect("persisted sentinel repair");
        let rendered = format!("{persisted:?}");
        let body_hash_debug = format!("{body_hash_sentinel:?}");

        for sentinel in [
            "operation-sentinel",
            "source-sentinel",
            "record-sentinel",
            "revision-sentinel",
            "receipt-sentinel",
            body_hash_debug.as_str(),
        ] {
            assert!(
                !rendered.contains(sentinel),
                "Debug output exposed opaque sentinel {sentinel:?}: {rendered}"
            );
        }
        assert!(rendered.contains("PersistedCloudWriteRepair"));
        assert!(rendered.contains("operation_bytes"));
        assert!(rendered.contains("body_hash_bytes"));
        assert!(rendered.contains("receipt_bytes"));
    }

    #[test]
    fn repair_page_debug_redacts_opaque_values() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = SourceId::new("source-sentinel");
        let first_repair = CloudWriteRepair::new(
            b"operation-sentinel",
            &source,
            SourceRecordKey::new("record-sentinel"),
            ProviderRevision {
                opaque: "revision-sentinel".to_owned(),
                content_hash: None,
            },
            [0xA5_u8; 32],
            b"receipt-sentinel",
            9,
        )
        .expect("valid sentinel repair");
        let second_repair = CloudWriteRepair::new(
            b"operation-sentinel-second",
            &source,
            SourceRecordKey::new("record-sentinel-second"),
            ProviderRevision {
                opaque: "revision-sentinel-second".to_owned(),
                content_hash: None,
            },
            [0x5A_u8; 32],
            b"receipt-sentinel-second",
            10,
        )
        .expect("valid second sentinel repair");
        store
            .record_provider_acceptance(&first_repair)
            .expect("persist first sentinel repair");
        store
            .record_provider_acceptance(&second_repair)
            .expect("persist second sentinel repair");

        let page = store
            .pending_write_repairs_page(&source, None, 1)
            .expect("load bounded sentinel page");
        let continuation = page
            .next_continuation()
            .expect("the first sentinel page has a continuation");
        let rendered = format!("{page:?}\n{continuation:?}");

        for sentinel in [
            "operation-sentinel",
            "source-sentinel",
            "record-sentinel",
            "revision-sentinel",
            "receipt-sentinel",
        ] {
            assert!(
                !rendered.contains(sentinel),
                "Debug output exposed opaque sentinel {sentinel:?}: {rendered}"
            );
        }
        assert!(rendered.contains("RepairPage"));
        assert!(rendered.contains("repair_count"));
        assert!(rendered.contains("RepairContinuation"));
        assert!(rendered.contains("created_at_ms"));
    }

    #[test]
    fn repeated_provider_acceptance_is_idempotent_and_rejects_a_conflicting_operation() {
        let directory = tempdir().expect("temporary SQLite directory");
        let store =
            CloudDocumentStore::open(directory.path().join("cloud.sqlite")).expect("open store");
        let source = source_id();
        let accepted = CloudWriteRepair::new(
            b"operation-duplicate",
            &source,
            SourceRecordKey::new("note.md"),
            ProviderRevision {
                opaque: "accepted-r2".to_owned(),
                content_hash: None,
            },
            [7_u8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            9,
        )
        .expect("valid accepted repair");
        let conflicting = CloudWriteRepair::new(
            b"operation-duplicate",
            &source,
            SourceRecordKey::new("other.md"),
            ProviderRevision {
                opaque: "accepted-r2".to_owned(),
                content_hash: None,
            },
            [7_u8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            9,
        )
        .expect("valid conflicting repair");

        store
            .record_provider_acceptance(&accepted)
            .expect("record first provider acceptance");
        store
            .record_provider_acceptance(&accepted)
            .expect("repeat identical provider acceptance");
        assert!(matches!(
            store.record_provider_acceptance(&conflicting),
            Err(CloudDocumentStoreError::WriteRepairCollision)
        ));
        drop(store);

        let reopened = CloudDocumentStore::open(directory.path().join("cloud.sqlite"))
            .expect("reopen durable journal");
        let pending = reopened
            .pending_write_repairs_page(&source, None, 1)
            .expect("enumerate source pending repairs");
        assert_eq!(pending.repairs().len(), 1);
        let pending = &pending.repairs()[0];
        assert_eq!(pending.operation_id(), accepted.operation_id());
        assert_eq!(pending.source_id_bytes(), source.as_str().as_bytes());
        assert_eq!(pending.record_key().as_str(), "note.md");
        assert_eq!(pending.accepted_revision().opaque, "accepted-r2");
        assert_eq!(pending.body_hash(), &[7_u8; 32]);
        assert_eq!(pending.receipt_json(), br#"{\"provider\":\"accepted\"}"#);
        assert_eq!(pending.created_at_ms(), 9);
    }

    #[test]
    fn repair_reads_enumeration_and_publication_marking_are_source_scoped() {
        let directory = tempdir().expect("temporary SQLite directory");
        let path = directory.path().join("cloud.sqlite");
        let store = CloudDocumentStore::open(&path).expect("open store");
        let source = source_id();
        let foreign = SourceId::new("other-cloud-store-source");
        let repair = CloudWriteRepair::new(
            b"operation-source-scope",
            &source,
            SourceRecordKey::new("note.md"),
            ProviderRevision {
                opaque: "accepted-r2".to_owned(),
                content_hash: None,
            },
            [7_u8; 32],
            br#"{\"provider\":\"accepted\"}"#,
            9,
        )
        .expect("valid repair");
        store
            .record_provider_acceptance(&repair)
            .expect("persist owned repair");
        let connection = rusqlite::Connection::open(&path).expect("inspect repair state");
        let before: (i64, String) = connection
            .query_row(
                "SELECT COUNT(*), MIN(state) FROM mdreader_cloud_write_repair",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read counter and state before foreign access");
        drop(connection);

        assert_eq!(
            store
                .load_write_repair(&foreign, repair.operation_id())
                .expect("foreign repair lookup"),
            None
        );
        assert!(store
            .pending_write_repairs_page(&foreign, None, 1)
            .expect("foreign repair enumeration")
            .repairs()
            .is_empty());
        assert!(matches!(
            store.mark_write_published(&foreign, repair.operation_id()),
            Err(CloudDocumentStoreError::WriteRepairNotFound)
        ));

        let connection = rusqlite::Connection::open(&path).expect("inspect unchanged repair state");
        let after: (i64, String) = connection
            .query_row(
                "SELECT COUNT(*), MIN(state) FROM mdreader_cloud_write_repair",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read counter and state after foreign access");
        assert_eq!(after, before);
        assert!(store
            .load_write_repair(&source, repair.operation_id())
            .expect("owner repair lookup")
            .expect("owner receipt remains present")
            .is_provider_accepted());
    }
}
