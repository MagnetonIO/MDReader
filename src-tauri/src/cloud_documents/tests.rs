use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use catdb_connector_dropbox::{
    DropboxAccessToken, DropboxAuthorizationMode, DropboxContentScope, DropboxCursorKey,
    DropboxDocumentError, DropboxHttpRequest, DropboxHttpResponse, DropboxRevision,
    DropboxRuntimeSecrets, DropboxSecretReference, DropboxSource, DropboxSourceConfig,
    DropboxTransport, DropboxTransportError, PinnedDropboxSourceSpec,
};
use catdb_connector_google_drive::{
    DriveAccessToken, DriveContentScope, DriveCursorKey, DriveDocumentError, DriveHttpMethod,
    DriveHttpRequest, DriveHttpResponse, DriveRevision, DriveRuntimeSecrets, DriveSecretReference,
    DriveSelection, DriveSource, DriveSourceConfig, DriveTransport, DriveTransportError,
    PinnedDriveSourceSpec,
};
use catdb_connectors::{CancellationToken, ReadContext, SourceError, SourceId};

use super::source::{
    DocumentLifecycle, DriveMarkdownDocumentSource, DropboxMarkdownDocumentSource,
    GuardedMarkdownDocumentOperations, GuardedMarkdownWriteReceipt,
};
use super::{
    map_drive_document_error, map_dropbox_document_error, map_lifecycle_error, CloudDocumentError,
    CloudIoContext, MarkdownDocumentSource, ProviderRevision, SourceRecordKey,
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

#[derive(Clone)]
struct DriveUnknownWriteTransport {
    calls: Arc<Mutex<usize>>,
    mutation_dispatches: Arc<Mutex<usize>>,
}

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

#[derive(Clone)]
struct DropboxUnknownWriteTransport {
    calls: Arc<Mutex<usize>>,
    mutation_dispatches: Arc<Mutex<usize>>,
}

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
