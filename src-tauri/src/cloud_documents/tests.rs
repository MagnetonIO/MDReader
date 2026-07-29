use std::{
    sync::{Arc, Mutex},
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
#[cfg(feature = "google-drive-connector")]
use catdb_connectors::SourceError;
use catdb_connectors::{CancellationToken, ReadContext, SourceId};

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
    CloudDocumentError, CloudIoContext, MarkdownDocumentSource, ProviderRevision, SourceRecordKey,
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
    fn version_one_schema_with_correct_columns_but_missing_load_bearing_shape_is_refused() {
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
            .pending_write_repairs(&source)
            .expect("enumerate source pending repairs");
        assert_eq!(pending.len(), 1);
        let pending = &pending[0];
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
            .pending_write_repairs(&foreign)
            .expect("foreign repair enumeration")
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
