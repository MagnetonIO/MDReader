use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use mdreader_connector_google_drive::{
    DriveAccessGrant, DriveAccessToken, DriveAuthorizationProfile, DriveAvailability,
    DriveChangeCursor, DriveChangeEvent, DriveChangeOutcome, DriveConnectorError,
    DriveContentScope, DriveCursorKey, DriveDocumentError, DriveHttpMethod, DriveHttpRequest,
    DriveHttpResponse, DriveOAuthPkce, DriveRefreshCredential, DriveRevision, DriveRuntimeSecrets,
    DriveSecretReference, DriveSecretResolver, DriveSelection, DriveSource, DriveSourceConfig,
    DriveTransport, DriveTransportError, PinnedDriveSourceSpec, GOOGLE_DRIVE_READONLY_SCOPE,
};
use catdb_sdk::connectors::{
    read, CancellationToken, DataSource, ReadContext, SourceCursor, SourceError,
};
use catdb_sdk::core::BaseType;
use catdb_sdk::query::{AttrOp, AttrValue, EntityFilter, EntityQuery, EntityTarget};

#[derive(Clone)]
struct ScriptedTransport {
    state: Arc<Mutex<ScriptState>>,
    auto_selected_folder_metadata: bool,
}

struct ScriptState {
    steps: VecDeque<ScriptStep>,
    requests: Vec<DriveHttpRequest>,
    accepted_writes: usize,
}

enum ScriptStep {
    Response(DriveHttpResponse),
    Pending,
    AcceptedWritePending,
    TransportError(DriveTransportError),
}

impl ScriptedTransport {
    fn new(steps: impl IntoIterator<Item = ScriptStep>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptState {
                steps: steps.into_iter().collect(),
                requests: Vec::new(),
                accepted_writes: 0,
            })),
            auto_selected_folder_metadata: true,
        }
    }

    fn without_selected_folder_metadata(steps: impl IntoIterator<Item = ScriptStep>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptState {
                steps: steps.into_iter().collect(),
                requests: Vec::new(),
                accepted_writes: 0,
            })),
            auto_selected_folder_metadata: false,
        }
    }

    fn requests(&self) -> Vec<DriveHttpRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    fn accepted_writes(&self) -> usize {
        self.state.lock().unwrap().accepted_writes
    }
}

#[async_trait]
impl DriveTransport for ScriptedTransport {
    async fn send(
        &self,
        request: DriveHttpRequest,
    ) -> Result<DriveHttpResponse, DriveTransportError> {
        let supplies_selected_folder_metadata =
            self.auto_selected_folder_metadata && is_selected_folder_metadata_request(&request);
        let step = {
            let mut state = self.state.lock().unwrap();
            state.requests.push(request);
            if supplies_selected_folder_metadata {
                None
            } else {
                let step = state.steps.pop_front().expect("fixture script exhausted");
                if matches!(step, ScriptStep::AcceptedWritePending) {
                    state.accepted_writes += 1;
                }
                Some(step)
            }
        };
        let Some(step) = step else {
            return DriveHttpResponse::new(
                200,
                BTreeMap::new(),
                serde_json::to_vec(&serde_json::json!({
                    "id":"selected-folder",
                    "name":"Selected",
                    "mimeType":"application/vnd.google-apps.folder",
                    "modifiedTime":"2026-07-20T12:34:55Z",
                    "parents":[],
                    "trashed":false
                }))
                .unwrap(),
            );
        };
        match step {
            ScriptStep::Response(response) => Ok(response),
            ScriptStep::Pending => std::future::pending().await,
            ScriptStep::AcceptedWritePending => std::future::pending().await,
            ScriptStep::TransportError(error) => Err(error),
        }
    }
}

#[derive(Clone)]
struct DriveSelectionRaceTransport {
    state: Arc<Mutex<DriveSelectionRaceState>>,
}

struct DriveSelectionRaceState {
    requests: Vec<DriveHttpRequest>,
    moved_after_preflight: bool,
    accepted_writes: usize,
}

impl DriveSelectionRaceTransport {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DriveSelectionRaceState {
                requests: Vec::new(),
                moved_after_preflight: false,
                accepted_writes: 0,
            })),
        }
    }

    fn requests(&self) -> Vec<DriveHttpRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    fn moved_after_preflight(&self) -> bool {
        self.state.lock().unwrap().moved_after_preflight
    }

    fn accepted_writes(&self) -> usize {
        self.state.lock().unwrap().accepted_writes
    }
}

#[async_trait]
impl DriveTransport for DriveSelectionRaceTransport {
    async fn send(
        &self,
        request: DriveHttpRequest,
    ) -> Result<DriveHttpResponse, DriveTransportError> {
        let mut state = self.state.lock().unwrap();
        state.requests.push(request.clone());
        if request.method() == DriveHttpMethod::Patch {
            if state.moved_after_preflight && request.header("If-Match") == Some("rev-1") {
                return DriveHttpResponse::new(412, BTreeMap::new(), b"moved-and-revised".to_vec());
            }
            state.accepted_writes += 1;
            return DriveHttpResponse::new(200, BTreeMap::new(), Vec::new());
        }
        if request
            .path()
            .starts_with("/drive/v3/files/record-1?fields=")
        {
            state.moved_after_preflight = true;
            return DriveHttpResponse::new(
                200,
                BTreeMap::from([("etag".to_string(), "rev-1".to_string())]),
                serde_json::to_vec(&serde_json::json!({
                    "id": "record-1",
                    "name": "note.md",
                    "mimeType": "text/markdown",
                    "modifiedTime": "2026-07-20T12:34:56Z",
                    "parents": ["selected-folder"],
                    "trashed": false
                }))
                .unwrap(),
            );
        }
        if is_selected_folder_metadata_request(&request) {
            return DriveHttpResponse::new(
                200,
                BTreeMap::new(),
                serde_json::to_vec(&serde_json::json!({
                    "id":"selected-folder",
                    "name":"Selected",
                    "mimeType":"application/vnd.google-apps.folder",
                    "modifiedTime":"2026-07-20T12:34:55Z",
                    "parents":[],
                    "trashed":false
                }))
                .unwrap(),
            );
        }
        panic!("unexpected Drive selection-race request: {:?}", request);
    }
}

fn is_selected_folder_metadata_request(request: &DriveHttpRequest) -> bool {
    request
        .path()
        .starts_with("/drive/v3/files/selected-folder?fields=")
}

#[derive(Clone)]
struct ConcurrentUnauthorizedTransport {
    barrier: Arc<tokio::sync::Barrier>,
    requests: Arc<Mutex<Vec<DriveHttpRequest>>>,
}

impl ConcurrentUnauthorizedTransport {
    fn new() -> Self {
        Self {
            barrier: Arc::new(tokio::sync::Barrier::new(2)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Vec<DriveHttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl DriveTransport for ConcurrentUnauthorizedTransport {
    async fn send(
        &self,
        request: DriveHttpRequest,
    ) -> Result<DriveHttpResponse, DriveTransportError> {
        let generation = request.credential_generation();
        let selected_folder_metadata = is_selected_folder_metadata_request(&request);
        self.requests.lock().unwrap().push(request);
        if generation == 0 {
            self.barrier.wait().await;
            return DriveHttpResponse::new(
                401,
                BTreeMap::new(),
                br#"{"error":{"code":401}}"#.to_vec(),
            );
        }
        if selected_folder_metadata {
            return DriveHttpResponse::new(
                200,
                BTreeMap::new(),
                serde_json::to_vec(&serde_json::json!({
                    "id":"selected-folder",
                    "name":"Selected",
                    "mimeType":"application/vnd.google-apps.folder",
                    "modifiedTime":"2026-07-20T12:34:55Z",
                    "parents":[],
                    "trashed":false
                }))
                .unwrap(),
            );
        }
        DriveHttpResponse::new(
            200,
            BTreeMap::new(),
            listing(serde_json::json!([]), None, false),
        )
    }
}

fn response(status: u16, body: impl Into<Vec<u8>>) -> ScriptStep {
    ScriptStep::Response(
        DriveHttpResponse::new(status, BTreeMap::new(), body.into()).expect("bounded fixture"),
    )
}

fn response_with_headers(
    status: u16,
    headers: impl IntoIterator<Item = (&'static str, &'static str)>,
    body: impl Into<Vec<u8>>,
) -> ScriptStep {
    ScriptStep::Response(
        DriveHttpResponse::new(
            status,
            headers
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body.into(),
        )
        .expect("bounded fixture"),
    )
}

fn config() -> DriveSourceConfig {
    DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![DriveSelection::folder("selected-folder")],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
}

fn shared_drive_config() -> DriveSourceConfig {
    config().with_shared_drive("shared-drive-7").unwrap()
}

fn selected_file_config() -> DriveSourceConfig {
    DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![DriveSelection::file("selected-file")],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
}

fn selected_file_with_resource_key_config() -> DriveSourceConfig {
    DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![
            DriveSelection::file_with_resource_key("selected-file", "link-key-7")
                .expect("bounded Picker resource key"),
        ],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
}

fn readable_root_config() -> DriveSourceConfig {
    DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![DriveSelection::root("provider-root")],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
    .with_content_scope(DriveContentScope::ReadableText)
}

fn readable_root_and_file_config() -> DriveSourceConfig {
    DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![
            DriveSelection::root("provider-root"),
            DriveSelection::file("selected-file"),
        ],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
    .with_content_scope(DriveContentScope::ReadableText)
}

fn readable_root_and_folder_config() -> DriveSourceConfig {
    DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![
            DriveSelection::root("provider-root"),
            DriveSelection::folder("selected-folder"),
        ],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
    .with_content_scope(DriveContentScope::ReadableText)
}

fn markdown_root_config() -> DriveSourceConfig {
    DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![DriveSelection::root("provider-root")],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
    .with_content_scope(DriveContentScope::Markdown)
}

fn source(
    transport: ScriptedTransport,
    config: DriveSourceConfig,
) -> (DriveSource, ScriptedTransport) {
    let source = DriveSource::open_with_transport(
        PinnedDriveSourceSpec::new(config, "drive-schema-v1").unwrap(),
        DriveRuntimeSecrets::new(
            DriveAccessToken::new("fixture-access-token").unwrap(),
            DriveCursorKey::new([0x41; 32]),
        ),
        Arc::new(transport.clone()),
    )
    .unwrap();
    (source, transport)
}

fn query() -> EntityQuery {
    EntityQuery {
        target: EntityTarget::Entity("drive.Document".to_string()),
        filters: Vec::new(),
    }
}

fn exact_id_query(id: &str) -> EntityQuery {
    EntityQuery {
        target: EntityTarget::Entity("drive.Document".to_string()),
        filters: vec![EntityFilter {
            attr: "id".to_string(),
            op: AttrOp::Eq,
            value: AttrValue::Text(id.to_string()),
            ty: BaseType::Text,
            origins: Vec::new(),
        }],
    }
}

fn context_with_id(
    query_id: uuid::Uuid,
    page_size: usize,
    row_budget: usize,
    byte_budget: usize,
    cancellation: CancellationToken,
) -> ReadContext {
    ReadContext::new(
        query_id,
        Instant::now() + Duration::from_secs(2),
        row_budget,
        byte_budget,
        page_size,
        cancellation,
    )
    .unwrap()
}

fn context(page_size: usize, row_budget: usize, byte_budget: usize) -> ReadContext {
    context_with_id(
        uuid::Uuid::new_v4(),
        page_size,
        row_budget,
        byte_budget,
        CancellationToken::new(),
    )
}

#[tokio::test]
async fn sync_metadata_uses_the_exact_drive_metadata_endpoint_without_a_body_read() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"sync-note",
        "name":"sync.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:56Z",
        "size":"14",
        "md5Checksum":"sync-checksum",
        "parents":["provider-root"],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::new([response(200, metadata)]),
        markdown_root_config(),
    );

    let row = source
        .metadata_for_sync("sync-note", &context(10, 10, 64 * 1024))
        .await
        .expect("exact metadata");

    assert_eq!(row.key, "sync-note");
    assert_eq!(row.value["name"], "sync.md");
    assert_eq!(row.value["checksum"], "sync-checksum");
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .path()
            .starts_with("/drive/v3/files/sync-note?fields="),
        "sync metadata must use the exact file metadata endpoint"
    );
    assert!(
        !requests[0].path().contains("alt=media"),
        "metadata synchronization must not download a document body"
    );
}

#[tokio::test]
async fn sync_metadata_preserves_typed_drive_provider_failures() {
    let cases = [
        (404, None, "not-found"),
        (401, None, "authentication"),
        (429, Some("7"), "rate-limited"),
        (500, None, "transport"),
    ];
    for (status, retry_after, expected) in cases {
        let step = match retry_after {
            Some(value) => response_with_headers(status, [("retry-after", value)], Vec::new()),
            None => response(status, Vec::new()),
        };
        let (source, transport) = source(ScriptedTransport::new([step]), markdown_root_config());
        let error = source
            .metadata_for_sync("sync-note", &context(10, 10, 64 * 1024))
            .await
            .expect_err("provider failure must not become an empty metadata row");
        assert!(matches!(
            (expected, error),
            ("not-found", SourceError::Read { .. })
                | ("authentication", SourceError::Authentication { .. })
                | (
                    "rate-limited",
                    SourceError::RateLimited {
                        retry_after_ms: Some(7_000),
                        ..
                    }
                )
                | ("transport", SourceError::Transport { .. })
        ));
        assert_eq!(transport.requests().len(), 1, "one exact metadata request");
    }
}

fn plan(source: &DriveSource, projected: &[&str]) -> catdb_sdk::connectors::SourcePlan {
    source
        .compile(
            &query(),
            &projected
                .iter()
                .map(|field| (*field).to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap()
}

fn listing(
    files: serde_json::Value,
    next_page_token: Option<&str>,
    incomplete_search: bool,
) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "files": files,
        "nextPageToken": next_page_token,
        "incompleteSearch": incomplete_search
    }))
    .unwrap()
}

#[tokio::test]
async fn downloads_the_exact_markdown_record_and_revision() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.MD",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(200, b"# one".to_vec()),
    ]);
    let (source, transport) = source(transport, markdown_root_config());

    let document = source
        .download_markdown("record-1", &context(10, 10, 64 * 1024))
        .await
        .expect("download");

    assert_eq!(document.record_key(), "record-1");
    assert_eq!(document.revision().as_str(), "rev-1");
    assert_eq!(document.body(), b"# one");
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn download_keeps_catalog_content_revision_separate_from_etag() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.MD",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "md5Checksum": "different-content-hash",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(200, b"# one".to_vec()),
    ]);
    let (source, transport) = source(transport, markdown_root_config());

    let document = source
        .download_markdown("record-1", &context(10, 10, 64 * 1024))
        .await
        .expect("download");

    assert_eq!(document.record_key(), "record-1");
    assert_eq!(document.revision().as_str(), "different-content-hash");
    assert_eq!(document.body(), b"# one");
    assert_eq!(transport.requests().len(), 2);
    assert_eq!(transport.requests()[1].header("If-Match"), Some("rev-1"));
}

#[tokio::test]
async fn download_refuses_a_metadata_identity_other_than_the_requested_record() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-2",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(200, b"# one".to_vec()),
    ]);
    let (source, transport) = source(transport, markdown_root_config());

    let error = source
        .download_markdown("record-1", &context(10, 10, 64 * 1024))
        .await
        .expect_err("metadata identity must match the requested record");

    assert!(matches!(error, DriveDocumentError::IdentityMismatch { .. }));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn download_refuses_a_record_outside_the_pinned_drive_selection() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["other-folder"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(404, Vec::new()),
    ]);
    let (source, transport) = source(transport, markdown_root_config());

    let error = source
        .download_markdown("record-1", &context(10, 10, 64 * 1024))
        .await
        .expect_err("the point read must remain inside the selected Drive root");

    assert!(matches!(
        error,
        DriveDocumentError::Source {
            error: SourceError::Read { .. }
        }
    ));
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn download_refuses_a_non_markdown_name_before_requesting_the_body() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.txt",
                "mimeType": "text/plain",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(200, b"plain text".to_vec()),
    ]);
    let (source, transport) = source(transport, markdown_root_config());

    let error = source
        .download_markdown("record-1", &context(10, 10, 64 * 1024))
        .await
        .expect_err("only Markdown names are admitted");

    assert!(matches!(error, DriveDocumentError::Protocol { .. }));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn download_refuses_a_markdown_body_larger_than_two_mebibytes() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(200, vec![b'a'; 2 * 1024 * 1024 + 1]),
    ]);
    let (source, _) = source(transport, markdown_root_config());

    let error = source
        .download_markdown("record-1", &context(10, 10, 4 * 1024 * 1024))
        .await
        .expect_err("Markdown bodies are capped at two mebibytes");

    assert!(matches!(error, DriveDocumentError::TooLarge { .. }));
}

#[tokio::test]
async fn download_refuses_a_non_utf8_markdown_body() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.markdown",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(200, vec![0xff, 0xfe]),
    ]);
    let (source, _) = source(transport, markdown_root_config());

    let error = source
        .download_markdown("record-1", &context(10, 10, 64 * 1024))
        .await
        .expect_err("Markdown bodies must be UTF-8");

    assert!(matches!(error, DriveDocumentError::InvalidUtf8 { .. }));
}

#[tokio::test]
async fn download_refuses_a_success_without_revision_authority() {
    let transport = ScriptedTransport::new([response(
        200,
        serde_json::to_vec(&serde_json::json!({
            "id": "record-1",
            "name": "note.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:34:56Z",
            "parents": ["provider-root"],
            "trashed": false
        }))
        .unwrap(),
    )]);
    let (source, transport) = source(transport, markdown_root_config());

    let error = source
        .download_markdown("record-1", &context(10, 10, 64 * 1024))
        .await
        .expect_err("a successful document response still needs revision authority");

    assert!(matches!(error, DriveDocumentError::Protocol { .. }));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn document_download_preserves_deadline_and_cancellation_as_typed_source_errors() {
    let (deadline_source, _) = source(
        ScriptedTransport::new([ScriptStep::Pending]),
        markdown_root_config(),
    );
    let deadline_context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_millis(20),
        10,
        64 * 1024,
        10,
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        deadline_source
            .download_markdown("record-1", &deadline_context)
            .await,
        Err(DriveDocumentError::Timeout)
    );

    let (cancelled_source, _) = source(ScriptedTransport::new([]), markdown_root_config());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled_context = context_with_id(uuid::Uuid::new_v4(), 10, 10, 64 * 1024, cancellation);
    assert_eq!(
        cancelled_source
            .download_markdown("record-1", &cancelled_context)
            .await,
        Err(DriveDocumentError::Cancelled)
    );
}

#[tokio::test]
async fn document_http_failures_remain_structurally_mappable() {
    let (not_found_source, _) = source(
        ScriptedTransport::new([response(404, Vec::new())]),
        markdown_root_config(),
    );
    assert!(matches!(
        not_found_source
            .download_markdown("record-1", &context(10, 10, 64 * 1024))
            .await,
        Err(DriveDocumentError::NotFound { .. })
    ));

    let (authentication_source, _) = source(
        ScriptedTransport::new([response(401, Vec::new())]),
        markdown_root_config(),
    );
    assert!(matches!(
        authentication_source
            .download_markdown("record-1", &context(10, 10, 64 * 1024))
            .await,
        Err(DriveDocumentError::Authentication { .. })
    ));

    let (timeout_source, _) = source(
        ScriptedTransport::new([response(408, Vec::new())]),
        markdown_root_config(),
    );
    assert!(matches!(
        timeout_source
            .download_markdown("record-1", &context(10, 10, 64 * 1024))
            .await,
        Err(DriveDocumentError::Timeout)
    ));

    let (rate_limited_source, _) = source(
        ScriptedTransport::new([response_with_headers(
            429,
            [("retry-after", "13")],
            Vec::new(),
        )]),
        markdown_root_config(),
    );
    assert!(matches!(
        rate_limited_source
            .download_markdown("record-1", &context(10, 10, 64 * 1024))
            .await,
        Err(DriveDocumentError::RateLimited {
            retry_after_ms: Some(13_000),
            ..
        })
    ));
}

#[tokio::test]
async fn stale_revision_is_a_conflict_and_never_creates_a_second_drive_record() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "metadata-rev")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response(412, b"provider-conflict".to_vec()),
    ]);
    let (source, transport) = source(transport, markdown_root_config());

    let error = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect_err("a stale revision must conflict");

    assert!(matches!(error, DriveDocumentError::Conflict { .. }));
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method(), DriveHttpMethod::Patch);
    assert!(requests[1]
        .path()
        .starts_with("/upload/drive/v3/files/record-1?"));
    assert_eq!(requests[1].header("If-Match"), Some("rev-1"));
    assert_eq!(requests[1].body(), b"# edited");
    let rendered = format!("{:?}", requests[1]);
    assert!(!rendered.contains("record-1"));
    assert!(!rendered.contains("# edited"));
}

#[tokio::test]
async fn guarded_drive_update_treats_ambiguous_http_receipts_as_unknown_without_a_retry() {
    let metadata = || {
        response_with_headers(
            200,
            [("ETag", "metadata-rev")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        )
    };

    for status in [408, 504, 500, 503] {
        let transport = ScriptedTransport::new([metadata(), response(status, Vec::new())]);
        let (source, transport) = source(transport, markdown_root_config());

        let error = source
            .update_markdown(
                "record-1",
                &DriveRevision::new("rev-1").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect_err("an ambiguous post-dispatch HTTP response cannot prove the write failed");

        let DriveDocumentError::WriteOutcomeUnknown { reconciliation } = error else {
            panic!("HTTP {status} after a Drive mutation must preserve an unknown outcome");
        };
        assert_eq!(reconciliation.record_fingerprint().len(), 43);
        let requests = transport.requests();
        assert_eq!(requests.len(), 2, "HTTP {status} must not trigger a retry");
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method() == DriveHttpMethod::Patch)
                .count(),
            1,
            "HTTP {status} must dispatch exactly one guarded mutation"
        );
    }
}

#[tokio::test]
async fn guarded_drive_update_keeps_definitive_post_dispatch_statuses_typed() {
    let metadata = || {
        response_with_headers(
            200,
            [("ETag", "metadata-rev")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        )
    };

    for status in [404, 401, 403, 429] {
        let receipt = if status == 429 {
            response_with_headers(status, [("retry-after", "7")], Vec::new())
        } else {
            response(status, Vec::new())
        };
        let (source, transport) = source(
            ScriptedTransport::new([metadata(), receipt]),
            markdown_root_config(),
        );

        let error = source
            .update_markdown(
                "record-1",
                &DriveRevision::new("rev-1").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect_err("a provider-declared rejected mutation remains definite");

        assert!(
            match status {
                404 => matches!(error, DriveDocumentError::NotFound { .. }),
                401 | 403 => matches!(error, DriveDocumentError::Authentication { .. }),
                429 => matches!(
                    error,
                    DriveDocumentError::RateLimited {
                        retry_after_ms: Some(7_000),
                        ..
                    }
                ),
                _ => unreachable!(),
            },
            "HTTP {status} must retain its definitive document outcome"
        );
        assert_eq!(
            transport
                .requests()
                .iter()
                .filter(|request| request.method() == DriveHttpMethod::Patch)
                .count(),
            1,
            "HTTP {status} must not trigger a second mutation"
        );
    }
}

#[tokio::test]
async fn guarded_drive_update_refuses_a_parent_and_revision_move_between_preflight_and_patch() {
    let transport = DriveSelectionRaceTransport::new();
    let source = DriveSource::open_with_transport(
        PinnedDriveSourceSpec::new(
            config().with_content_scope(DriveContentScope::Markdown),
            "drive-schema-v1",
        )
        .unwrap(),
        DriveRuntimeSecrets::new(
            DriveAccessToken::new("fixture-access-token").unwrap(),
            DriveCursorKey::new([0x41; 32]),
        ),
        Arc::new(transport.clone()),
    )
    .unwrap();

    let error = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect_err("the conditional PATCH must reject the moved record");

    assert!(matches!(error, DriveDocumentError::Conflict { .. }));
    assert!(transport.moved_after_preflight());
    let requests = transport.requests();
    let patch = requests
        .iter()
        .find(|request| request.method() == DriveHttpMethod::Patch)
        .expect("one guarded mutation attempt");
    assert!(patch.path().starts_with("/upload/drive/v3/files/record-1?"));
    assert_eq!(patch.header("If-Match"), Some("rev-1"));
    assert_eq!(transport.accepted_writes(), 0);
}

#[tokio::test]
async fn drive_write_after_provider_acceptance_returns_unknown_on_cancellation_or_deadline() {
    let metadata = || {
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        )
    };

    let cancellation = CancellationToken::new();
    let cancel_later = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_later.cancel();
    });
    let cancellation_transport =
        ScriptedTransport::new([metadata(), ScriptStep::AcceptedWritePending]);
    let (cancellation_source, cancellation_transport) =
        source(cancellation_transport, markdown_root_config());
    let cancellation_error = cancellation_source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context_with_id(uuid::Uuid::new_v4(), 10, 10, 64 * 1024, cancellation),
        )
        .await
        .expect_err("a cancellation after dispatch cannot prove the write failed");
    let DriveDocumentError::WriteOutcomeUnknown { reconciliation } = cancellation_error else {
        panic!("a post-dispatch cancellation must preserve an unknown outcome");
    };
    assert_eq!(reconciliation.record_fingerprint().len(), 43);
    assert!(!format!("{reconciliation:?}").contains("record-1"));
    assert_eq!(cancellation_transport.accepted_writes(), 1);

    let deadline_transport = ScriptedTransport::new([metadata(), ScriptStep::AcceptedWritePending]);
    let (deadline_source, deadline_transport) = source(deadline_transport, markdown_root_config());
    let deadline_context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_millis(20),
        10,
        64 * 1024,
        10,
        CancellationToken::new(),
    )
    .unwrap();
    let deadline_error = deadline_source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &deadline_context,
        )
        .await
        .expect_err("a deadline after dispatch cannot prove the write failed");
    assert!(matches!(
        deadline_error,
        DriveDocumentError::WriteOutcomeUnknown { .. }
    ));
    assert_eq!(deadline_transport.accepted_writes(), 1);
}

#[tokio::test]
async fn drive_write_before_dispatch_keeps_cancellation_and_deadline_definite() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancellation_transport = ScriptedTransport::new([]);
    let (cancellation_source, cancellation_transport) =
        source(cancellation_transport, markdown_root_config());
    assert_eq!(
        cancellation_source
            .update_markdown(
                "record-1",
                &DriveRevision::new("rev-1").unwrap(),
                b"# edited",
                &context_with_id(uuid::Uuid::new_v4(), 10, 10, 64 * 1024, cancellation),
            )
            .await,
        Err(DriveDocumentError::Cancelled)
    );
    assert!(cancellation_transport.requests().is_empty());

    let deadline_transport = ScriptedTransport::new([]);
    let (deadline_source, deadline_transport) = source(deadline_transport, markdown_root_config());
    let deadline_context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() - Duration::from_millis(1),
        10,
        64 * 1024,
        10,
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        deadline_source
            .update_markdown(
                "record-1",
                &DriveRevision::new("rev-1").unwrap(),
                b"# edited",
                &deadline_context,
            )
            .await,
        Err(DriveDocumentError::Timeout)
    );
    assert!(deadline_transport.requests().is_empty());
}

#[tokio::test]
async fn drive_write_with_an_unusable_success_receipt_returns_unknown_without_a_retry() {
    let metadata = || {
        response_with_headers(
            200,
            [("ETag", "rev-1")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        )
    };

    for receipt in [
        response_with_headers(200, [("ETag", "rev-2")], Vec::new()),
        ScriptStep::TransportError(DriveTransportError::ResponseTooLarge),
    ] {
        let transport = ScriptedTransport::new([metadata(), receipt]);
        let (source, transport) = source(transport, markdown_root_config());
        let error = source
            .update_markdown(
                "record-1",
                &DriveRevision::new("rev-1").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect_err("an unusable receipt leaves the accepted write unknown");
        assert!(matches!(
            error,
            DriveDocumentError::WriteOutcomeUnknown { .. }
        ));
        assert_eq!(
            transport.requests().len(),
            2,
            "a guarded write receipt must never trigger a second mutation"
        );
    }
}

#[tokio::test]
async fn update_refuses_a_record_outside_the_selected_drive_file_before_patching() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [("ETag", "metadata-rev")],
        serde_json::to_vec(&serde_json::json!({
            "id": "outside-record",
            "name": "outside.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:34:56Z",
            "parents": [],
            "trashed": false
        }))
        .unwrap(),
    )]);
    let selected_file_config = DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![DriveSelection::file("selected-record")],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
    .with_content_scope(DriveContentScope::Markdown);
    let (source, transport) = source(transport, selected_file_config);

    let result = source
        .update_markdown(
            "outside-record",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await;

    assert!(result.is_err(), "an out-of-selection write must be refused");
    assert!(
        transport
            .requests()
            .iter()
            .all(|request| request.method() != DriveHttpMethod::Patch),
        "source authorization must complete before the provider mutation"
    );
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn guarded_drive_update_refuses_an_empty_response_even_when_an_etag_is_present() {
    let metadata = response_with_headers(
        200,
        [("ETag", "metadata-rev")],
        serde_json::to_vec(&serde_json::json!({
            "id": "record-1",
            "name": "note.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:34:56Z",
            "parents": ["provider-root"],
            "trashed": false
        }))
        .unwrap(),
    );
    let empty_update = response_with_headers(200, [("ETag", "rev-2")], Vec::new());
    let (source, _) = source(
        ScriptedTransport::new([metadata, empty_update]),
        markdown_root_config(),
    );

    let result = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await;

    assert!(
        result.is_err(),
        "a successful Drive update must return a validating File resource"
    );
}

#[tokio::test]
async fn guarded_drive_update_refuses_a_file_response_for_a_different_record() {
    let metadata = response_with_headers(
        200,
        [("ETag", "metadata-rev")],
        serde_json::to_vec(&serde_json::json!({
            "id": "record-1",
            "name": "note.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:34:56Z",
            "parents": ["provider-root"],
            "trashed": false
        }))
        .unwrap(),
    );
    let mismatched_update = response_with_headers(
        200,
        [("ETag", "rev-2")],
        serde_json::to_vec(&serde_json::json!({
            "id": "record-2",
            "name": "note.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:35:56Z",
            "parents": ["provider-root"],
            "trashed": false
        }))
        .unwrap(),
    );
    let (source, transport) = source(
        ScriptedTransport::new([metadata, mismatched_update]),
        markdown_root_config(),
    );

    let error = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect_err("a Drive update receipt must identify the requested record");

    assert!(matches!(
        error,
        DriveDocumentError::WriteOutcomeUnknown { .. }
    ));
    assert_eq!(transport.requests().len(), 2);
    assert!(transport.requests()[1]
        .path()
        .contains("fields=id%2Cname%2CmimeType%2CmodifiedTime%2Csize%2Cmd5Checksum%2CresourceKey%2Cparents%2Ctrashed"));
}

#[tokio::test]
async fn guarded_drive_update_refuses_a_non_markdown_file_response() {
    let metadata = response_with_headers(
        200,
        [("ETag", "metadata-rev")],
        serde_json::to_vec(&serde_json::json!({
            "id": "record-1",
            "name": "note.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:34:56Z",
            "parents": ["provider-root"],
            "trashed": false
        }))
        .unwrap(),
    );
    let non_markdown_update = response_with_headers(
        200,
        [("ETag", "rev-2")],
        serde_json::to_vec(&serde_json::json!({
            "id": "record-1",
            "name": "note.txt",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:35:56Z",
            "parents": ["provider-root"],
            "trashed": false
        }))
        .unwrap(),
    );
    let (source, _) = source(
        ScriptedTransport::new([metadata, non_markdown_update]),
        markdown_root_config(),
    );

    let error = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect_err("a Drive update receipt must remain a Markdown file");

    assert!(matches!(
        error,
        DriveDocumentError::WriteOutcomeUnknown { .. }
    ));
}

#[tokio::test]
async fn guarded_drive_update_marks_an_out_of_selection_success_receipt_unknown_without_a_recheck()
{
    let metadata = response_with_headers(
        200,
        [("ETag", "rev-1")],
        serde_json::to_vec(&serde_json::json!({
            "id": "record-1",
            "name": "note.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:34:56Z",
            "parents": ["selected-folder"],
            "trashed": false
        }))
        .unwrap(),
    );
    let moved_receipt = response_with_headers(
        200,
        [("ETag", "rev-2")],
        serde_json::to_vec(&serde_json::json!({
            "id": "record-1",
            "name": "note.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:35:56Z",
            "parents": ["outside-folder"],
            "trashed": false
        }))
        .unwrap(),
    );
    let (source, transport) = source(
        ScriptedTransport::new([metadata, moved_receipt]),
        config().with_content_scope(DriveContentScope::Markdown),
    );

    let error = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect_err("a moved success receipt cannot be accepted cleanly");

    assert!(matches!(
        error,
        DriveDocumentError::WriteOutcomeUnknown { .. }
    ));
    assert_eq!(
        transport.requests().len(),
        2,
        "the response itself must be classified without a post-write metadata check"
    );
}

#[tokio::test]
async fn guarded_drive_update_accepts_a_nested_selected_folder_receipt_from_the_preflight_snapshot()
{
    let file = |id: &str, name: &str, mime_type: &str, parents: serde_json::Value| {
        serde_json::to_vec(&serde_json::json!({
            "id": id,
            "name": name,
            "mimeType": mime_type,
            "modifiedTime": "2026-07-20T12:34:56Z",
            "parents": parents,
            "trashed": false
        }))
        .unwrap()
    };
    let transport = ScriptedTransport::without_selected_folder_metadata([
        response_with_headers(
            200,
            [("ETag", "metadata-rev")],
            file(
                "record-1",
                "note.md",
                "text/markdown",
                serde_json::json!(["nested-folder"]),
            ),
        ),
        response(
            200,
            file(
                "nested-folder",
                "Nested",
                "application/vnd.google-apps.folder",
                serde_json::json!(["selected-folder"]),
            ),
        ),
        response_with_headers(
            200,
            [("ETag", "rev-2")],
            file(
                "record-1",
                "note.md",
                "text/markdown",
                serde_json::json!(["nested-folder"]),
            ),
        ),
    ]);
    let (source, transport) = source(
        transport,
        config().with_content_scope(DriveContentScope::Markdown),
    );

    let receipt = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect("a receipt with unchanged preflight parents remains authorized");

    assert_eq!(receipt.record_key(), "record-1");
    assert_eq!(receipt.revision().as_str(), "rev-2");
    let requests = transport.requests();
    assert_eq!(
        requests.len(),
        3,
        "receipt validation must not re-read Drive"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method() == DriveHttpMethod::Patch)
            .count(),
        1
    );
}

#[tokio::test]
async fn guarded_drive_update_returns_the_provider_revision_authority() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "metadata-rev")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response_with_headers(
            200,
            [("ETag", "rev-2")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:35:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
    ]);
    let (source, _) = source(transport, markdown_root_config());

    let receipt = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect("guarded update");

    assert_eq!(receipt.record_key(), "record-1");
    assert_eq!(receipt.revision().as_str(), "rev-2");
}

#[tokio::test]
async fn guarded_drive_update_checks_content_revision_but_sends_etag_validator() {
    let transport = ScriptedTransport::new([
        response_with_headers(
            200,
            [("ETag", "metadata-rev")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:34:56Z",
                "md5Checksum": "content-hash-before",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
        response_with_headers(
            200,
            [("ETag", "rev-2")],
            serde_json::to_vec(&serde_json::json!({
                "id": "record-1",
                "name": "note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-20T12:35:56Z",
                "parents": ["provider-root"],
                "trashed": false
            }))
            .unwrap(),
        ),
    ]);
    let (source, transport) = source(transport, markdown_root_config());

    let receipt = source
        .update_markdown(
            "record-1",
            &DriveRevision::new("content-hash-before").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect("guarded update");

    assert_eq!(receipt.record_key(), "record-1");
    assert_eq!(receipt.revision().as_str(), "rev-2");
    assert_eq!(transport.requests()[1].header("If-Match"), Some("metadata-rev"));
}


#[test]
fn downloaded_document_and_write_receipt_debug_redact_provider_record_keys() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let download_transport = ScriptedTransport::new([
            response_with_headers(
                200,
                [("ETag", "download-revision")],
                serde_json::to_vec(&serde_json::json!({
                    "id": "sensitive-record-key",
                    "name": "note.md",
                    "mimeType": "text/markdown",
                    "modifiedTime": "2026-07-20T12:34:56Z",
                    "parents": ["provider-root"],
                    "trashed": false
                }))
                .unwrap(),
            ),
            response(200, b"# sensitive body".to_vec()),
        ]);
        let (download_source, _) = source(download_transport, markdown_root_config());
        let document = download_source
            .download_markdown("sensitive-record-key", &context(10, 10, 64 * 1024))
            .await
            .expect("download");
        let rendered_document = format!("{document:?}");
        assert!(!rendered_document.contains("sensitive-record-key"));
        assert!(!rendered_document.contains("download-revision"));
        assert!(!rendered_document.contains("# sensitive body"));

        let update_transport = ScriptedTransport::new([
            response_with_headers(
                200,
                [("ETag", "metadata-revision")],
                serde_json::to_vec(&serde_json::json!({
                    "id": "sensitive-record-key",
                    "name": "note.md",
                    "mimeType": "text/markdown",
                    "modifiedTime": "2026-07-20T12:34:56Z",
                    "parents": ["provider-root"],
                    "trashed": false
                }))
                .unwrap(),
            ),
            response_with_headers(
                200,
                [("ETag", "updated-revision")],
                serde_json::to_vec(&serde_json::json!({
                    "id": "sensitive-record-key",
                    "name": "note.md",
                    "mimeType": "text/markdown",
                    "modifiedTime": "2026-07-20T12:35:56Z",
                    "parents": ["provider-root"],
                    "trashed": false
                }))
                .unwrap(),
            ),
        ]);
        let (update_source, _) = source(update_transport, markdown_root_config());
        let receipt = update_source
            .update_markdown(
                "sensitive-record-key",
                &DriveRevision::new("expected-revision").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect("update");
        let rendered_receipt = format!("{receipt:?}");
        assert!(!rendered_receipt.contains("sensitive-record-key"));
        assert!(!rendered_receipt.contains("metadata-revision"));
    });
}

#[test]
fn provider_response_debug_redacts_direct_urls_and_bodies() {
    let response = DriveHttpResponse::new(
        200,
        BTreeMap::from([(
            "location".to_string(),
            "https://provider.example/private-document".to_string(),
        )]),
        b"# confidential Markdown".to_vec(),
    )
    .unwrap();

    let rendered = format!("{response:?}");
    assert!(!rendered.contains("https://provider.example/private-document"));
    assert!(!rendered.contains("# confidential Markdown"));
}

#[test]
fn native_pkce_requests_the_exact_read_only_scope_without_a_client_secret() {
    let pkce = DriveOAuthPkce::new(
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        "state-123",
        "http://127.0.0.1:41003/oauth/callback",
    )
    .unwrap();
    assert_eq!(
        pkce.code_challenge(),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    let authorization = pkce
        .authorization_url("desktop-client.apps.googleusercontent.com")
        .unwrap();
    assert!(authorization.contains("code_challenge_method=S256"));
    assert!(authorization.contains("state=state-123"));
    assert!(
        authorization.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.readonly")
    );
    assert!(!authorization.contains("drive.file"));
    assert!(authorization.contains("prompt=consent"));
    assert!(!authorization.contains("client_secret"));
    assert_eq!(config().oauth_scope(), GOOGLE_DRIVE_READONLY_SCOPE);
    assert_eq!(
        GOOGLE_DRIVE_READONLY_SCOPE,
        "https://www.googleapis.com/auth/drive.readonly"
    );
}

#[test]
fn native_pkce_requests_the_exact_markdown_write_scope() {
    let pkce = DriveOAuthPkce::new(
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        "state-123",
        "http://127.0.0.1:41003/oauth/callback",
    )
    .unwrap();

    let authorization = pkce
        .authorization_url_for_profile(
            "desktop-client.apps.googleusercontent.com",
            DriveAuthorizationProfile::MarkdownReadWrite,
        )
        .unwrap();

    assert!(authorization.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive"));
    assert!(!authorization.contains("drive.readonly"));
    assert!(!authorization.contains("client_secret"));
}

#[derive(Clone)]
struct RefreshingResolver {
    resolve_count: Arc<AtomicUsize>,
    refresh_count: Arc<AtomicUsize>,
    refresh_mode: RefreshMode,
    initial_expired: bool,
}

#[derive(Clone, Copy)]
enum RefreshMode {
    Succeeds,
    Revoked,
    Transport,
    RateLimited,
    Pending,
}

impl RefreshingResolver {
    fn new(refresh_mode: RefreshMode) -> Self {
        Self {
            resolve_count: Arc::new(AtomicUsize::new(0)),
            refresh_count: Arc::new(AtomicUsize::new(0)),
            refresh_mode,
            initial_expired: false,
        }
    }

    fn with_expired_access(mut self) -> Self {
        self.initial_expired = true;
        self
    }
}

#[async_trait]
impl DriveSecretResolver for RefreshingResolver {
    async fn resolve(
        &self,
        _reference: &DriveSecretReference,
    ) -> Result<DriveRuntimeSecrets, DriveConnectorError> {
        self.resolve_count.fetch_add(1, Ordering::SeqCst);
        let expires_at = if self.initial_expired {
            SystemTime::now() - Duration::from_secs(1)
        } else {
            SystemTime::now() + Duration::from_secs(3_600)
        };
        Ok(DriveRuntimeSecrets::refreshable(
            DriveAccessGrant::new(
                DriveAccessToken::new("initial-access-token")?,
                Some(expires_at),
            )?,
            DriveRefreshCredential::new("runtime-refresh-token")?,
            DriveCursorKey::new([0x41; 32]),
        ))
    }

    async fn refresh(
        &self,
        _reference: &DriveSecretReference,
        _credential: &DriveRefreshCredential,
    ) -> Result<DriveAccessGrant, DriveConnectorError> {
        self.refresh_count.fetch_add(1, Ordering::SeqCst);
        match self.refresh_mode {
            RefreshMode::Succeeds => DriveAccessGrant::new(
                DriveAccessToken::new("refreshed-access-token")?,
                Some(SystemTime::now() + Duration::from_secs(3_600)),
            ),
            RefreshMode::Revoked => Err(DriveConnectorError::InvalidSecret),
            RefreshMode::Transport => Err(DriveConnectorError::Transport),
            RefreshMode::RateLimited => Err(DriveConnectorError::RateLimited {
                retry_after_ms: Some(17_000),
            }),
            RefreshMode::Pending => std::future::pending().await,
        }
    }
}

#[tokio::test]
async fn concurrent_expired_access_uses_one_bounded_refresh_exchange() {
    let transport = ScriptedTransport::new([
        response(200, listing(serde_json::json!([]), None, false)),
        response(200, listing(serde_json::json!([]), None, false)),
    ]);
    let resolver = RefreshingResolver::new(RefreshMode::Succeeds).with_expired_access();
    let (source, transport, resolver) = refreshable_source(transport, resolver).await;
    let first = context(2, 2, 4_096);
    let second = context(2, 2, 4_096);
    let first_plan = plan(&source, &["id"]);
    let second_plan = plan(&source, &["id"]);

    let (left, right) = tokio::join!(
        source.execute_page(&first_plan, None, &first),
        source.execute_page(&second_plan, None, &second),
    );

    left.unwrap();
    right.unwrap();
    assert_eq!(resolver.refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport
            .requests()
            .iter()
            .map(DriveHttpRequest::credential_generation)
            .collect::<Vec<_>>(),
        vec![1, 1, 1, 1]
    );
}

#[tokio::test]
async fn concurrent_unauthorized_responses_share_one_forced_refresh() {
    let transport = ConcurrentUnauthorizedTransport::new();
    let resolver = RefreshingResolver::new(RefreshMode::Succeeds);
    let source = DriveSource::open_with_resolver_and_transport(
        PinnedDriveSourceSpec::new(config(), "drive-schema-v1").unwrap(),
        Arc::new(resolver.clone()),
        Arc::new(transport.clone()),
    )
    .await
    .unwrap();
    let first = context(2, 2, 4_096);
    let second = context(2, 2, 4_096);
    let first_plan = plan(&source, &["id"]);
    let second_plan = plan(&source, &["id"]);

    let (left, right) = tokio::join!(
        source.execute_page(&first_plan, None, &first),
        source.execute_page(&second_plan, None, &second),
    );

    left.unwrap();
    right.unwrap();
    assert_eq!(resolver.refresh_count.load(Ordering::SeqCst), 1);
    let requests = transport.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.credential_generation() == 0)
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.credential_generation() == 1)
            .count(),
        4
    );
}

async fn refreshable_source(
    transport: ScriptedTransport,
    resolver: RefreshingResolver,
) -> (DriveSource, ScriptedTransport, RefreshingResolver) {
    let source = DriveSource::open_with_resolver_and_transport(
        PinnedDriveSourceSpec::new(config(), "drive-schema-v1").unwrap(),
        Arc::new(resolver.clone()),
        Arc::new(transport.clone()),
    )
    .await
    .unwrap();
    (source, transport, resolver)
}

#[tokio::test]
async fn one_unauthorized_response_refreshes_once_and_the_same_source_serves_later_reads() {
    let transport = ScriptedTransport::new([
        response(401, br#"{"error":{"code":401}}"#),
        response(200, listing(serde_json::json!([]), None, false)),
        response(200, listing(serde_json::json!([]), None, false)),
    ]);
    let (source, transport, resolver) =
        refreshable_source(transport, RefreshingResolver::new(RefreshMode::Succeeds)).await;

    for _ in 0..2 {
        source
            .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 4_096))
            .await
            .unwrap();
    }

    assert_eq!(resolver.resolve_count.load(Ordering::SeqCst), 1);
    assert_eq!(resolver.refresh_count.load(Ordering::SeqCst), 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests
            .iter()
            .filter(|request| !is_selected_folder_metadata_request(request))
            .map(DriveHttpRequest::credential_generation)
            .collect::<Vec<_>>(),
        vec![0, 1, 1]
    );
}

#[tokio::test]
async fn revoked_refresh_credential_and_cancelled_refresh_remain_typed() {
    let (revoked, _, revoked_resolver) = refreshable_source(
        ScriptedTransport::new([response(401, br#"{"error":{"code":401}}"#)]),
        RefreshingResolver::new(RefreshMode::Revoked),
    )
    .await;
    assert!(matches!(
        revoked
            .execute_page(&plan(&revoked, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::Authentication { .. })
    ));
    assert_eq!(revoked_resolver.refresh_count.load(Ordering::SeqCst), 1);

    let (pending, _, pending_resolver) = refreshable_source(
        ScriptedTransport::new([response(401, br#"{"error":{"code":401}}"#)]),
        RefreshingResolver::new(RefreshMode::Pending),
    )
    .await;
    let cancellation = CancellationToken::new();
    let cancel_later = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_later.cancel();
    });
    assert_eq!(
        pending
            .execute_page(
                &plan(&pending, &["id"]),
                None,
                &context_with_id(uuid::Uuid::new_v4(), 2, 2, 4_096, cancellation),
            )
            .await,
        Err(SourceError::Cancelled)
    );
    assert_eq!(pending_resolver.refresh_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refresh_pipeline_preserves_authentication_transport_and_rate_limit_outcomes() {
    let (authentication, _, _) = refreshable_source(
        ScriptedTransport::new([response(401, br#"{"error":{"code":401}}"#)]),
        RefreshingResolver::new(RefreshMode::Revoked),
    )
    .await;
    assert!(matches!(
        authentication
            .execute_page(&plan(&authentication, &["id"]), None, &context(2, 2, 4_096),)
            .await,
        Err(SourceError::Authentication { .. })
    ));

    let (transport, _, _) = refreshable_source(
        ScriptedTransport::new([response(401, br#"{"error":{"code":401}}"#)]),
        RefreshingResolver::new(RefreshMode::Transport),
    )
    .await;
    assert!(matches!(
        transport
            .execute_page(&plan(&transport, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::Transport { .. })
    ));

    let (rate_limited, _, _) = refreshable_source(
        ScriptedTransport::new([
            response(401, br#"{"error":{"code":401}}"#),
            response_with_headers(429, [("retry-after", "11")], br#"{"error":{"code":429}}"#),
        ]),
        RefreshingResolver::new(RefreshMode::Succeeds),
    )
    .await;
    assert!(matches!(
        rate_limited
            .execute_page(&plan(&rate_limited, &["id"]), None, &context(2, 2, 4_096),)
            .await,
        Err(SourceError::RateLimited {
            retry_after_ms: Some(11_000),
            ..
        })
    ));

    let (refresh_rate_limited, _, _) = refreshable_source(
        ScriptedTransport::new([response(401, br#"{"error":{"code":401}}"#)]),
        RefreshingResolver::new(RefreshMode::RateLimited),
    )
    .await;
    assert!(matches!(
        refresh_rate_limited
            .execute_page(
                &plan(&refresh_rate_limited, &["id"]),
                None,
                &context(2, 2, 4_096),
            )
            .await,
        Err(SourceError::RateLimited {
            retry_after_ms: Some(17_000),
            ..
        })
    ));
}

#[test]
fn source_specs_and_debug_output_never_contain_runtime_credentials() {
    let transport = ScriptedTransport::new([]);
    let source_config = config();
    assert!(!format!("{source_config:?}").contains("selected-folder"));
    let (source, _) = source(transport, source_config);
    let spec = serde_json::to_string(&source.source_spec()).unwrap();
    assert!(spec.contains("keychain/mdreader/drive"));
    assert!(!spec.contains("fixture-access-token"));
    assert!(!format!("{source:?}").contains("fixture-access-token"));
    assert!(!format!("{:?}", source.runtime_secret_evidence()).contains("fixture-access-token"));
    assert_eq!(
        source.availability_evidence(),
        DriveAvailability::FixtureValidated
    );
}

#[test]
fn deserialized_pinned_spec_revalidates_schema_identity() {
    let mut value = serde_json::to_value(
        PinnedDriveSourceSpec::new(config(), "drive-schema-v1").expect("valid pin"),
    )
    .expect("serialize pin");
    value["schema_version"] = serde_json::Value::String(String::new());

    assert!(serde_json::from_value::<PinnedDriveSourceSpec>(value).is_err());
}

#[test]
fn resource_keys_accept_bounded_header_safe_values_without_guessing_provider_grammar() {
    assert!(
        DriveSelection::file_with_resource_key("selected-file", "rk.dot~value=").is_ok(),
        "visible non-delimiter bytes remain provider-defined"
    );
    assert!(DriveSelection::file_with_resource_key("selected-file", "rk,bad").is_err());
    assert!(DriveSelection::file_with_resource_key("selected-file", "rk/bad").is_err());
    assert!(DriveSelection::file_with_resource_key("selected-file", "rk\nbad").is_err());
}

#[test]
fn configured_selections_are_unique_by_drive_id_and_conflicting_keys_fail_closed() {
    let secret = DriveSecretReference::new("keychain/mdreader/drive").unwrap();
    assert!(DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![
            DriveSelection::file("same-id"),
            DriveSelection::folder("same-id"),
        ],
        secret.clone(),
    )
    .is_err());
    assert!(DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![
            DriveSelection::file_with_resource_key("same-id", "key-one").unwrap(),
            DriveSelection::file_with_resource_key("same-id", "key-two").unwrap(),
        ],
        secret,
    )
    .is_err());
}

#[tokio::test]
async fn readable_root_lists_the_drive_once_and_excludes_non_reader_content() {
    let root_metadata = serde_json::to_vec(&serde_json::json!({
        "id":"provider-root",
        "name":"My Drive",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let global_listing = listing(
        serde_json::json!([
            {
                "id":"nested-note",
                "name":"nested.md",
                "mimeType":"text/markdown",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "size":"4",
                "parents":["nested-folder"],
                "trashed":false
            },
            {
                "id":"unreadable-pdf",
                "name":"archive.pdf",
                "mimeType":"application/pdf",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "size":"400",
                "parents":["nested-folder"],
                "trashed":false
            }
        ]),
        None,
        false,
    );
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([
            response(200, root_metadata),
            response(200, global_listing),
        ]),
        readable_root_config(),
    );

    let answer = read(
        &source,
        &query(),
        &["id".to_string(), "name".to_string()],
        &context(16, 16, 64 * 1024),
    )
    .await
    .expect("readable root query");

    assert_eq!(answer.rows.len(), 1);
    assert_eq!(answer.rows[0].key, "nested-note");
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    let list = url::Url::parse(&format!("https://www.googleapis.com{}", requests[1].path()))
        .expect("list URL");
    let query = list
        .query_pairs()
        .find_map(|(key, value)| (key == "q").then_some(value.into_owned()))
        .expect("Drive query");
    assert!(!query.contains("in parents"));
    assert!(query.contains("text/markdown"));
    assert!(query.contains("text/plain"));
    assert!(query.contains("application/vnd.google-apps.document"));
}

#[tokio::test]
async fn markdown_root_excludes_plain_text_and_google_documents() {
    let root_metadata = serde_json::to_vec(&serde_json::json!({
        "id":"provider-root",
        "name":"My Drive",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let global_listing = listing(
        serde_json::json!([
            {
                "id":"markdown-short",
                "name":"one.md",
                "mimeType":"text/plain",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "size":"4",
                "parents":["nested-folder"],
                "trashed":false
            },
            {
                "id":"markdown-long",
                "name":"guide.markdown",
                "mimeType":"text/markdown",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "size":"4",
                "parents":["nested-folder"],
                "trashed":false
            },
            {
                "id":"plain-text",
                "name":"notes.txt",
                "mimeType":"text/plain",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "size":"4",
                "parents":["nested-folder"],
                "trashed":false
            },
            {
                "id":"google-document",
                "name":"Provider document",
                "mimeType":"application/vnd.google-apps.document",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "parents":["nested-folder"],
                "trashed":false
            }
        ]),
        None,
        false,
    );
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([
            response(200, root_metadata),
            response(200, global_listing),
        ]),
        markdown_root_config(),
    );

    let answer = read(
        &source,
        &query(),
        &["id".to_string(), "name".to_string()],
        &context(16, 16, 64 * 1024),
    )
    .await
    .expect("Markdown-only Drive query");

    assert_eq!(
        answer
            .rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>(),
        ["markdown-short", "markdown-long"]
    );
    let requests = transport.requests();
    let list = url::Url::parse(&format!("https://www.googleapis.com{}", requests[1].path()))
        .expect("list URL");
    let provider_query = list
        .query_pairs()
        .find_map(|(key, value)| (key == "q").then_some(value.into_owned()))
        .expect("Drive query");
    assert!(provider_query.contains("text/markdown"));
    assert!(provider_query.contains("text/plain"));
    assert!(!provider_query.contains("application/vnd.google-apps.document"));
}

#[tokio::test]
async fn readable_root_exact_id_reads_only_that_document_and_its_body() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"nested-note",
        "name":"nested.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:56Z",
        "size":"14",
        "md5Checksum":"note-checksum",
        "parents":["nested-folder"],
        "trashed":false
    }))
    .unwrap();
    let parent_metadata = serde_json::to_vec(&serde_json::json!({
        "id":"nested-folder",
        "name":"Notes",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":["provider-root"],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([
            response(200, metadata),
            response(200, parent_metadata),
            response(200, b"# Nested note\n".to_vec()),
        ]),
        readable_root_config(),
    );
    let plan = source
        .compile(
            &exact_id_query("nested-note"),
            &["id".to_string(), "name".to_string(), "body".to_string()],
        )
        .expect("exact id plan");
    let page = source
        .execute_page(&plan, None, &context(10, 10, 64 * 1024))
        .await
        .expect("exact document");

    assert_eq!(page.scanned, 1);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].value["body"], "# Nested note\n");
    let requests = transport.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0]
        .path()
        .starts_with("/drive/v3/files/nested-note?"));
    assert!(requests[1]
        .path()
        .starts_with("/drive/v3/files/nested-folder?"));
    assert!(requests[2]
        .path()
        .contains("/drive/v3/files/nested-note?alt=media"));
    assert!(requests
        .iter()
        .all(|request| !request.path().contains("q=")));
}

#[tokio::test]
async fn root_and_file_exact_id_accepts_the_selected_file_outside_the_root() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"selected-file",
        "name":"selected.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:56Z",
        "size":"16",
        "md5Checksum":"selected-checksum",
        "parents":["foreign-folder"],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([
            response(200, metadata),
            response(200, b"# Selected note\n".to_vec()),
        ]),
        readable_root_and_file_config(),
    );
    let plan = source
        .compile(
            &exact_id_query("selected-file"),
            &["id".to_string(), "name".to_string(), "body".to_string()],
        )
        .expect("exact id plan");

    let page = source
        .execute_page(&plan, None, &context(10, 10, 64 * 1024))
        .await
        .expect("the selected File remains exact-read authority");

    assert_eq!(page.rows[0].value["body"], "# Selected note\n");
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]
        .path()
        .contains("/drive/v3/files/selected-file?alt=media"));
    assert!(requests
        .iter()
        .all(|request| !request.path().contains("foreign-folder")));
}

#[tokio::test]
async fn root_and_folder_exact_id_accepts_a_nested_file_beneath_the_folder() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"folder-note",
        "name":"folder-note.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:56Z",
        "size":"14",
        "md5Checksum":"folder-checksum",
        "parents":["nested-selected-folder"],
        "trashed":false
    }))
    .unwrap();
    let parent_metadata = serde_json::to_vec(&serde_json::json!({
        "id":"nested-selected-folder",
        "name":"Nested",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":["selected-folder"],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([
            response(200, metadata),
            response(200, parent_metadata),
            response(200, b"# Folder note\n".to_vec()),
        ]),
        readable_root_and_folder_config(),
    );
    let plan = source
        .compile(
            &exact_id_query("folder-note"),
            &["id".to_string(), "name".to_string(), "body".to_string()],
        )
        .expect("exact id plan");

    let page = source
        .execute_page(&plan, None, &context(10, 10, 64 * 1024))
        .await
        .expect("the selected Folder remains exact-read authority");

    assert_eq!(page.rows[0].value["body"], "# Folder note\n");
    let requests = transport.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1]
        .path()
        .starts_with("/drive/v3/files/nested-selected-folder?"));
    assert!(requests[2]
        .path()
        .contains("/drive/v3/files/folder-note?alt=media"));
}

#[tokio::test]
async fn root_exact_id_rejects_an_accessible_file_outside_the_selected_root_before_body_read() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"outside-note",
        "name":"outside.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:56Z",
        "size":"17",
        "md5Checksum":"outside-checksum",
        "parents":["foreign-folder"],
        "trashed":false
    }))
    .unwrap();
    let foreign_parent = serde_json::to_vec(&serde_json::json!({
        "id":"foreign-folder",
        "name":"Shared elsewhere",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([
            response(200, metadata),
            response(200, foreign_parent),
        ]),
        readable_root_config(),
    );
    let plan = source
        .compile(
            &exact_id_query("outside-note"),
            &["id".to_string(), "name".to_string(), "body".to_string()],
        )
        .expect("exact id plan");

    let error = source
        .execute_page(&plan, None, &context(10, 10, 64 * 1024))
        .await
        .expect_err("a token-readable ID outside My Drive is not source authority");

    assert!(matches!(error, SourceError::Read { .. }));
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]
        .path()
        .starts_with("/drive/v3/files/foreign-folder?"));
    assert!(requests
        .iter()
        .all(|request| !request.path().contains("alt=media")));
}

#[tokio::test]
async fn readable_root_change_tracking_keeps_nested_text_and_tombstones_other_content() {
    let changed = serde_json::to_vec(&serde_json::json!({
        "changes": [
            {
                "changeType":"file",
                "removed":false,
                "fileId":"nested-note",
                "file":{
                    "id":"nested-note",
                    "name":"nested.md",
                    "mimeType":"text/markdown",
                    "modifiedTime":"2026-07-20T12:34:56Z",
                    "md5Checksum":"note-checksum",
                    "parents":["nested-folder"],
                    "trashed":false
                }
            },
            {
                "changeType":"file",
                "removed":false,
                "fileId":"nested-pdf",
                "file":{
                    "id":"nested-pdf",
                    "name":"archive.pdf",
                    "mimeType":"application/pdf",
                    "modifiedTime":"2026-07-20T12:34:57Z",
                    "md5Checksum":"pdf-checksum",
                    "parents":["nested-folder"],
                    "trashed":false
                }
            }
        ],
        "newStartPageToken":"changes-2"
    }))
    .unwrap();
    let (source, _) = source(
        ScriptedTransport::new([
            response(200, br#"{"startPageToken":"changes-1"}"#),
            response(200, changed),
        ]),
        readable_root_config(),
    );
    let cursor = source
        .start_change_tracking(&context(20, 20, 64 * 1024))
        .await
        .expect("start changes");
    let outcome = source
        .read_changes(&cursor, &context(20, 20, 64 * 1024))
        .await
        .expect("read changes");
    let DriveChangeOutcome::Page { events, .. } = outcome else {
        panic!("expected change page");
    };
    assert_eq!(
        events,
        vec![
            DriveChangeEvent::Upserted {
                file_id: "nested-note".to_string(),
            },
            DriveChangeEvent::Deleted {
                file_id: "nested-pdf".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn markdown_selected_file_change_leaving_content_scope_emits_tombstone() {
    let materialized = serde_json::to_vec(&serde_json::json!({
        "id":"selected-file",
        "name":"selected.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let changed = serde_json::to_vec(&serde_json::json!({
        "changes": [{
            "changeType":"file",
            "removed":false,
            "fileId":"selected-file",
            "file":{
                "id":"selected-file",
                "name":"selected.txt",
                "mimeType":"text/plain",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "parents":[],
                "trashed":false
            }
        }],
        "newStartPageToken":"changes-2"
    }))
    .unwrap();
    let (source, _) = source(
        ScriptedTransport::new([
            response(200, materialized),
            response(200, br#"{"startPageToken":"changes-1"}"#),
            response(200, changed),
        ]),
        selected_file_config().with_content_scope(DriveContentScope::Markdown),
    );

    source
        .execute_page(&plan(&source, &["id"]), None, &context(20, 20, 64 * 1024))
        .await
        .expect("materialize the selected Markdown record");
    let cursor = source
        .start_change_tracking(&context(20, 20, 64 * 1024))
        .await
        .expect("start changes");
    let DriveChangeOutcome::Page { events, .. } = source
        .read_changes(&cursor, &context(20, 20, 64 * 1024))
        .await
        .expect("read changes")
    else {
        panic!("expected change page");
    };

    assert_eq!(
        events,
        vec![DriveChangeEvent::Deleted {
            file_id: "selected-file".to_string(),
        }]
    );
}

#[tokio::test]
async fn markdown_tracked_folder_child_leaving_content_scope_emits_tombstone() {
    let materialized = listing(
        serde_json::json!([{
            "id":"renamed-note",
            "name":"renamed-note.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:55Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let changed = serde_json::to_vec(&serde_json::json!({
        "changes": [{
            "changeType":"file",
            "removed":false,
            "fileId":"renamed-note",
            "file":{
                "id":"renamed-note",
                "name":"renamed-note.txt",
                "mimeType":"text/plain",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "parents":["selected-folder"],
                "trashed":false
            }
        }],
        "newStartPageToken":"changes-2"
    }))
    .unwrap();
    let (source, _) = source(
        ScriptedTransport::new([
            response(200, materialized),
            response(200, br#"{"startPageToken":"changes-1"}"#),
            response(200, changed),
        ]),
        config().with_content_scope(DriveContentScope::Markdown),
    );

    read(
        &source,
        &query(),
        &["id".to_string()],
        &context(20, 20, 64 * 1024),
    )
    .await
    .expect("materialize the tracked Markdown record");
    let cursor = source
        .start_change_tracking(&context(20, 20, 64 * 1024))
        .await
        .expect("start changes");
    let DriveChangeOutcome::Page { events, .. } = source
        .read_changes(&cursor, &context(20, 20, 64 * 1024))
        .await
        .expect("read changes")
    else {
        panic!("expected change page");
    };

    assert_eq!(
        events,
        vec![DriveChangeEvent::Deleted {
            file_id: "renamed-note".to_string(),
        }]
    );
}

#[tokio::test]
async fn read_follows_every_page_and_preserves_examined_zero_as_success() {
    let first = listing(
        serde_json::json!([{
            "id": "a",
            "name": "a.md",
            "mimeType": "text/markdown",
            "modifiedTime": "2026-07-20T12:34:56.789Z",
            "size": "3",
            "md5Checksum": "aaa",
            "parents": ["selected-folder"],
            "trashed": false
        }]),
        Some("page-token-2"),
        false,
    );
    let second = listing(serde_json::json!([]), None, false);
    let (source, transport) = source(
        ScriptedTransport::new([response(200, first), response(200, second)]),
        config(),
    );
    let answer = read(
        &source,
        &query(),
        &["id".to_string(), "name".to_string()],
        &context(1, 4, 64 * 1024),
    )
    .await
    .unwrap();

    assert_eq!(answer.scanned, 1);
    assert_eq!(answer.rows.len(), 1);
    assert_eq!(answer.rows[0].key, "a");
    assert_eq!(
        answer.rows[0].value,
        serde_json::json!({"id":"a","name":"a.md"})
    );
    let requests = transport.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].path().contains("pageToken=page-token-2"));
    assert!(requests
        .iter()
        .all(|request| request.method() == DriveHttpMethod::Get));
}

#[tokio::test]
async fn incomplete_search_is_a_typed_failure_not_a_complete_empty_page() {
    let body = listing(serde_json::json!([]), None, true);
    let (source, _) = source(ScriptedTransport::new([response(200, body)]), config());
    assert!(matches!(
        source
            .execute_page(
                &plan(&source, &["id"]),
                None,
                &context(10, 10, 64 * 1024),
            )
            .await,
        Err(SourceError::IncompleteSearch { source_id }) if source_id == *source.id()
    ));
}

#[tokio::test]
async fn rejected_remote_page_token_is_invalid_cursor_not_transport_or_empty() {
    let first = listing(
        serde_json::json!([{
            "id":"a",
            "name":"a.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        Some("expired-token"),
        false,
    );
    let (source, _) = source(
        ScriptedTransport::new([
            response(200, first),
            response(
                400,
                br#"{"error":{"code":400,"message":"Invalid page token."}}"#,
            ),
        ]),
        config(),
    );
    let query_id = uuid::Uuid::new_v4();
    let page = source
        .execute_page(
            &plan(&source, &["id"]),
            None,
            &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
        )
        .await
        .unwrap();
    assert!(matches!(
        source
            .execute_page(
                &plan(&source, &["id"]),
                page.next_cursor.as_ref(),
                &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
            )
            .await,
        Err(SourceError::InvalidCursor)
    ));
}

#[tokio::test]
async fn selected_file_ids_use_files_get_instead_of_an_unsupported_list_query_term() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"selected-file",
        "name":"selected.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:56Z",
        "size":"8",
        "md5Checksum":"selected-checksum",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::new([response(200, metadata)]),
        selected_file_config(),
    );
    let page = source
        .execute_page(
            &plan(&source, &["id", "name"]),
            None,
            &context(10, 10, 64 * 1024),
        )
        .await
        .unwrap();

    assert_eq!(page.scanned, 1);
    assert_eq!(page.rows[0].key, "selected-file");
    assert_eq!(
        transport.requests()[0].path(),
        "/drive/v3/files/selected-file?fields=id%2Cname%2CmimeType%2CmodifiedTime%2Csize%2Cmd5Checksum%2CresourceKey%2Cparents%2Ctrashed&supportsAllDrives=true"
    );
    assert!(!transport.requests()[0].path().contains("q=id"));
    assert!(!format!("{:?}", transport.requests()[0]).contains("selected-file"));
}

#[tokio::test]
async fn selected_folder_is_validated_with_files_get_before_list_traversal() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"selected-folder",
        "name":"Selected",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([
            response(200, metadata),
            response(200, listing(serde_json::json!([]), None, false)),
        ]),
        config(),
    );

    let page = source
        .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 4_096))
        .await
        .unwrap();

    assert_eq!(page.scanned, 0);
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(is_selected_folder_metadata_request(&requests[0]));
    assert!(requests[1]
        .path()
        .contains("%27selected-folder%27+in+parents"));
}

#[tokio::test]
async fn deleted_or_trashed_selected_folder_is_a_read_failure_not_empty_success() {
    let (deleted, deleted_transport) = source(
        ScriptedTransport::without_selected_folder_metadata([response(
            404,
            br#"{"error":{"code":404}}"#,
        )]),
        config(),
    );
    assert!(matches!(
        deleted
            .execute_page(&plan(&deleted, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::Read { path, .. }) if path == "selected Drive folder"
    ));
    assert!(is_selected_folder_metadata_request(
        &deleted_transport.requests()[0]
    ));

    let trashed_metadata = serde_json::to_vec(&serde_json::json!({
        "id":"selected-folder",
        "name":"Selected",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":[],
        "trashed":true
    }))
    .unwrap();
    let (trashed, _) = source(
        ScriptedTransport::without_selected_folder_metadata([response(200, trashed_metadata)]),
        config(),
    );
    assert!(matches!(
        trashed
            .execute_page(&plan(&trashed, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::Read { path, .. }) if path == "selected Drive folder"
    ));
}

#[tokio::test]
async fn non_folder_selection_cannot_be_traversed_as_a_folder() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"selected-folder",
        "name":"not-a-folder.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::without_selected_folder_metadata([response(200, metadata)]),
        config(),
    );

    assert!(matches!(
        source
            .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::Malformed { attr, .. }) if attr == "selected folder kind"
    ));
    assert!(is_selected_folder_metadata_request(
        &transport.requests()[0]
    ));
}

#[tokio::test]
async fn selected_folder_get_preserves_auth_rate_and_transport_failures() {
    let cases = [
        (
            response(403, br#"{"error":{"code":403}}"#),
            "authentication",
        ),
        (
            response_with_headers(429, [("retry-after", "13")], br#"{"error":{"code":429}}"#),
            "rate",
        ),
        (response(500, br#"{"error":{"code":500}}"#), "transport"),
    ];
    for (step, expected) in cases {
        let (source, transport) = source(
            ScriptedTransport::without_selected_folder_metadata([step]),
            config(),
        );
        let result = source
            .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 4_096))
            .await;
        match expected {
            "authentication" => {
                assert!(matches!(result, Err(SourceError::Authentication { .. })))
            }
            "rate" => assert!(matches!(
                result,
                Err(SourceError::RateLimited {
                    retry_after_ms: Some(13_000),
                    ..
                })
            )),
            "transport" => assert!(matches!(result, Err(SourceError::Transport { .. }))),
            _ => unreachable!(),
        }
        assert!(is_selected_folder_metadata_request(
            &transport.requests()[0]
        ));
    }
}

#[tokio::test]
async fn selected_folder_metadata_with_a_conflicting_resource_key_fails_closed() {
    let keyed_config = DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![DriveSelection::folder_with_resource_key("selected-folder", "picker-key").unwrap()],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap();
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"selected-folder",
        "name":"Selected",
        "mimeType":"application/vnd.google-apps.folder",
        "modifiedTime":"2026-07-20T12:34:55Z",
        "resourceKey":"different-key",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let (source, _) = source(
        ScriptedTransport::without_selected_folder_metadata([response(200, metadata)]),
        keyed_config,
    );

    assert!(matches!(
        source
            .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::Malformed { attr, .. }) if attr == "resourceKey"
    ));
}

#[tokio::test]
async fn selected_folders_are_walked_recursively_and_membership_is_rechecked() {
    let root = listing(
        serde_json::json!([
            {
                "id":"nested-folder",
                "name":"Nested",
                "mimeType":"application/vnd.google-apps.folder",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "parents":["selected-folder"],
                "trashed":false
            },
            {
                "id":"root-note",
                "name":"root.md",
                "mimeType":"text/markdown",
                "modifiedTime":"2026-07-20T12:34:57Z",
                "parents":["selected-folder"],
                "trashed":false
            }
        ]),
        None,
        false,
    );
    let nested = listing(
        serde_json::json!([{
            "id":"nested-note",
            "name":"nested.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:58Z",
            "parents":["nested-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (recursive_source, transport) = source(
        ScriptedTransport::new([response(200, root), response(200, nested)]),
        config(),
    );

    let answer = read(
        &recursive_source,
        &query(),
        &["id".to_string(), "name".to_string()],
        &context(10, 10, 64 * 1024),
    )
    .await
    .unwrap();

    assert_eq!(answer.scanned, 3);
    assert_eq!(
        answer
            .rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>(),
        vec!["root-note", "nested-note"]
    );
    let requests = transport.requests();
    assert!(requests[1]
        .path()
        .contains("%27selected-folder%27+in+parents"));
    assert!(requests[2]
        .path()
        .contains("%27nested-folder%27+in+parents"));

    let unrelated = listing(
        serde_json::json!([{
            "id":"leak",
            "name":"leak.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:59Z",
            "parents":["other-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (guarded, _) = source(ScriptedTransport::new([response(200, unrelated)]), config());
    assert!(matches!(
        guarded
            .execute_page(
                &plan(&guarded, &["id"]),
                None,
                &context(10, 10, 64 * 1024),
            )
            .await,
        Err(SourceError::Malformed { attr, .. }) if attr == "parents"
    ));
}

#[tokio::test]
async fn recursive_walk_deduplicates_cycles_and_refuses_excessive_depth() {
    let cycle = listing(
        serde_json::json!([{
            "id":"selected-folder",
            "name":"Cycle",
            "mimeType":"application/vnd.google-apps.folder",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (cyclic, cyclic_transport) =
        source(ScriptedTransport::new([response(200, cycle)]), config());
    let answer = read(
        &cyclic,
        &query(),
        &["id".to_string()],
        &context(4, 4, 64 * 1024),
    )
    .await
    .unwrap();
    assert_eq!(answer.scanned, 1);
    assert!(answer.rows.is_empty());
    assert_eq!(cyclic_transport.requests().len(), 2);

    let mut steps = Vec::new();
    for depth in 0..=32 {
        let parent = if depth == 0 {
            "selected-folder".to_string()
        } else {
            format!("folder-{}", depth - 1)
        };
        steps.push(response(
            200,
            listing(
                serde_json::json!([{
                    "id":format!("folder-{depth}"),
                    "name":format!("Folder {depth}"),
                    "mimeType":"application/vnd.google-apps.folder",
                    "modifiedTime":"2026-07-20T12:34:56Z",
                    "parents":[parent],
                    "trashed":false
                }]),
                None,
                false,
            ),
        ));
    }
    let (too_deep, _) = source(ScriptedTransport::new(steps), config());
    assert!(matches!(
        read(
            &too_deep,
            &query(),
            &["id".to_string()],
            &context(100, 100, 256 * 1024),
        )
        .await,
        Err(SourceError::ResourceLimit { resource, .. })
            if resource == "Drive recursion depth"
    ));
}

#[tokio::test]
async fn recursive_membership_classifies_moved_out_and_moved_in_children() {
    let root = listing(
        serde_json::json!([{
            "id":"nested-folder",
            "name":"Nested",
            "mimeType":"application/vnd.google-apps.folder",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let nested = listing(serde_json::json!([]), None, false);
    let start = br#"{"startPageToken":"changes-100"}"#;
    let changes = serde_json::to_vec(&serde_json::json!({
        "changes": [
            {
                "changeType":"file",
                "removed":false,
                "fileId":"moved-out",
                "file":{
                    "id":"moved-out",
                    "name":"out.md",
                    "mimeType":"text/markdown",
                    "modifiedTime":"2026-07-21T00:00:00Z",
                    "parents":["outside"],
                    "trashed":false
                }
            },
            {
                "changeType":"file",
                "removed":false,
                "fileId":"moved-in",
                "file":{
                    "id":"moved-in",
                    "name":"in.md",
                    "mimeType":"text/markdown",
                    "modifiedTime":"2026-07-21T00:00:01Z",
                    "parents":["nested-folder"],
                    "trashed":false
                }
            }
        ],
        "newStartPageToken":"changes-200"
    }))
    .unwrap();
    let (source, _) = source(
        ScriptedTransport::new([
            response(200, root),
            response(200, nested),
            response(200, start),
            response(200, changes),
        ]),
        config(),
    );
    read(
        &source,
        &query(),
        &["id".to_string()],
        &context(10, 10, 64 * 1024),
    )
    .await
    .unwrap();
    let cursor = source
        .start_change_tracking(&context(10, 10, 64 * 1024))
        .await
        .unwrap();
    let DriveChangeOutcome::Page { events, .. } = source
        .read_changes(&cursor, &context(10, 10, 64 * 1024))
        .await
        .unwrap()
    else {
        panic!("expected change page");
    };
    assert_eq!(
        events,
        vec![
            DriveChangeEvent::Deleted {
                file_id: "moved-out".to_string(),
            },
            DriveChangeEvent::Upserted {
                file_id: "moved-in".to_string(),
            }
        ]
    );
}

#[tokio::test]
async fn picker_resource_keys_follow_selected_and_discovered_link_shared_items() {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "id":"selected-file",
        "name":"selected.md",
        "mimeType":"text/markdown",
        "modifiedTime":"2026-07-20T12:34:56Z",
        "parents":[],
        "trashed":false
    }))
    .unwrap();
    let (selected, selected_transport) = source(
        ScriptedTransport::new([response(200, metadata)]),
        selected_file_with_resource_key_config(),
    );
    selected
        .execute_page(&plan(&selected, &["id"]), None, &context(2, 2, 64 * 1024))
        .await
        .unwrap();
    assert_eq!(
        selected_transport.requests()[0].header("x-goog-drive-resource-keys"),
        Some("selected-file/link-key-7")
    );

    let root_config = DriveSourceConfig::new(
        "drive-library",
        "drive",
        "Document",
        vec![
            DriveSelection::folder_with_resource_key("selected-folder", "root-key")
                .expect("bounded Picker resource key"),
        ],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap();
    let root = listing(
        serde_json::json!([{
            "id":"nested-folder",
            "name":"Nested",
            "mimeType":"application/vnd.google-apps.folder",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "resourceKey":"nested-key",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (nested, nested_transport) = source(
        ScriptedTransport::new([
            response(200, root),
            response(200, listing(serde_json::json!([]), None, false)),
        ]),
        root_config,
    );
    read(
        &nested,
        &query(),
        &["id".to_string()],
        &context(4, 4, 64 * 1024),
    )
    .await
    .unwrap();
    assert_eq!(
        nested_transport.requests()[0].header("x-goog-drive-resource-keys"),
        Some("selected-folder/root-key")
    );
    let nested_requests = nested_transport.requests();
    let nested_header = nested_requests[2]
        .header("x-goog-drive-resource-keys")
        .unwrap();
    assert!(nested_header.contains("selected-folder/root-key"));
    assert!(nested_header.contains("nested-folder/nested-key"));
}

#[tokio::test]
async fn discovered_file_resource_key_is_forwarded_before_body_read_and_then_persisted() {
    let root = listing(
        serde_json::json!([{
            "id":"linked-note",
            "name":"linked.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "resourceKey":"linked-key",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (source, transport) = source(
        ScriptedTransport::new([
            response(200, root),
            response(200, b"# Linked".to_vec()),
            response(200, br#"{"startPageToken":"changes-100"}"#),
        ]),
        config(),
    );

    let page = source
        .execute_page(
            &plan(&source, &["id", "body"]),
            None,
            &context(4, 4, 64 * 1024),
        )
        .await
        .unwrap();
    assert_eq!(page.rows[0].value["body"], "# Linked");
    source
        .start_change_tracking(&context(4, 4, 64 * 1024))
        .await
        .unwrap();

    let requests = transport.requests();
    let body_request = requests
        .iter()
        .find(|request| request.path().contains("linked-note?alt=media"))
        .unwrap();
    assert_eq!(
        body_request.header("x-goog-drive-resource-keys"),
        Some("linked-note/linked-key")
    );
    let change_request = requests
        .iter()
        .find(|request| request.path().contains("changes/startPageToken"))
        .unwrap();
    assert_eq!(
        change_request.header("x-goog-drive-resource-keys"),
        Some("linked-note/linked-key")
    );
}

#[tokio::test]
async fn completed_rebuild_replaces_recursive_membership_after_drive_churn() {
    let first_root = listing(
        serde_json::json!([{
            "id":"moved-folder",
            "name":"Moved",
            "mimeType":"application/vnd.google-apps.folder",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "resourceKey":"moved-folder-key",
            "parents":["selected-folder"],
            "trashed":false
        }, {
            "id":"moved-file",
            "name":"moved.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:57Z",
            "resourceKey":"moved-file-key",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (source, transport) = source(
        ScriptedTransport::new([
            response(200, first_root),
            response(200, listing(serde_json::json!([]), None, false)),
            response(200, listing(serde_json::json!([]), None, false)),
            response(200, br#"{"startPageToken":"changes-100"}"#),
        ]),
        config(),
    );

    for _ in 0..2 {
        read(
            &source,
            &query(),
            &["id".to_string()],
            &context(10, 10, 64 * 1024),
        )
        .await
        .unwrap();
    }
    source
        .start_change_tracking(&context(4, 4, 64 * 1024))
        .await
        .unwrap();

    let requests = transport.requests();
    let change_request = requests
        .iter()
        .find(|request| request.path().contains("changes/startPageToken"))
        .unwrap();
    assert!(change_request
        .header("x-goog-drive-resource-keys")
        .is_none());
}

#[tokio::test]
async fn failed_header_bound_does_not_partially_publish_tracked_membership() {
    let files = (0..12)
        .map(|index| {
            serde_json::json!({
                "id":format!("{}{:02}", "f".repeat(248), index),
                "name":format!("Folder {index}"),
                "mimeType":"application/vnd.google-apps.folder",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "resourceKey":"k".repeat(500),
                "parents":["selected-folder"],
                "trashed":false
            })
        })
        .collect::<Vec<_>>();
    let (source, transport) = source(
        ScriptedTransport::new([
            response(200, listing(files.into(), None, false)),
            response(200, br#"{"startPageToken":"changes-100"}"#),
        ]),
        config(),
    );

    assert!(matches!(
        source
            .execute_page(&plan(&source, &["id"]), None, &context(20, 20, 64 * 1024))
            .await,
        Err(SourceError::ResourceLimit { resource, .. })
            if resource == "Drive resource-key header"
    ));
    source
        .start_change_tracking(&context(2, 2, 64 * 1024))
        .await
        .unwrap();
    let requests = transport.requests();
    let change_request = requests
        .iter()
        .find(|request| request.path().contains("changes/startPageToken"))
        .unwrap();
    assert!(change_request
        .header("x-goog-drive-resource-keys")
        .is_none());
}

#[tokio::test]
async fn selected_file_404_is_an_ambiguous_read_failure_not_successful_emptiness() {
    let (source, _) = source(
        ScriptedTransport::new([response(404, br#"{"error":{"code":404}}"#)]),
        selected_file_config(),
    );
    assert!(matches!(
        source
            .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::Read { path, .. }) if path == "selected Drive item"
    ));
}

#[tokio::test]
async fn provider_empty_pages_with_continuations_are_followed_inside_one_bounded_page() {
    let empty = listing(serde_json::json!([]), Some("sparse-next"), false);
    let populated = listing(
        serde_json::json!([{
            "id":"later",
            "name":"later.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (source, transport) = source(
        ScriptedTransport::new([response(200, empty), response(200, populated)]),
        DriveSourceConfig::new(
            "drive-library",
            "drive",
            "Document",
            vec![DriveSelection::folder("selected-folder")],
            DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
        )
        .unwrap(),
    );
    let page = source
        .execute_page(&plan(&source, &["id"]), None, &context(1, 2, 64 * 1024))
        .await
        .unwrap();

    assert_eq!(page.scanned, 1);
    assert_eq!(page.rows[0].key, "later");
    assert!(page.next_cursor.is_none());
    assert_eq!(transport.requests().len(), 3);
    assert!(transport.requests()[2]
        .path()
        .contains("pageToken=sparse-next"));
}

#[tokio::test]
async fn malformed_provider_page_tokens_never_cross_into_a_followup_request() {
    let malformed = listing(serde_json::json!([]), Some("bad\npage"), false);
    let (source, transport) = source(ScriptedTransport::new([response(200, malformed)]), config());
    assert_eq!(
        source
            .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 4_096))
            .await,
        Err(SourceError::InvalidCursor)
    );
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn query_cursor_is_authenticated_and_bound_to_query_plan_spec_and_query_id() {
    let body = listing(
        serde_json::json!([{
            "id":"a",
            "name":"a.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        Some("opaque-next"),
        false,
    );
    let transport = ScriptedTransport::new([
        response(200, body),
        response(200, listing(serde_json::json!([]), None, false)),
    ]);
    let (first_source, _) = source(transport, config());
    let query_id = uuid::Uuid::new_v4();
    let page = first_source
        .execute_page(
            &plan(&first_source, &["id"]),
            None,
            &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
        )
        .await
        .unwrap();
    let cursor = page.next_cursor.unwrap();

    let same_source = source(
        ScriptedTransport::new([response(200, listing(serde_json::json!([]), None, false))]),
        config(),
    )
    .0;
    assert!(first_source
        .execute_page(
            &plan(&first_source, &["id"]),
            Some(&cursor),
            &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
        )
        .await
        .is_ok());
    assert!(matches!(
        same_source
            .execute_page(
                &plan(&same_source, &["id"]),
                Some(&cursor),
                &context_with_id(
                    uuid::Uuid::new_v4(),
                    1,
                    4,
                    64 * 1024,
                    CancellationToken::new()
                ),
            )
            .await,
        Err(SourceError::InvalidCursor)
    ));
    assert!(matches!(
        same_source
            .execute_page(
                &plan(&same_source, &["name"]),
                Some(&cursor),
                &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
            )
            .await,
        Err(SourceError::InvalidCursor)
    ));

    for (different_config, schema_version) in [
        (
            DriveSourceConfig::new(
                "other-drive-source",
                "drive",
                "Document",
                vec![DriveSelection::folder("selected-folder")],
                DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
            )
            .unwrap(),
            "drive-schema-v1",
        ),
        (
            DriveSourceConfig::new(
                "drive-library",
                "drive",
                "Document",
                vec![DriveSelection::folder("different-selection")],
                DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
            )
            .unwrap(),
            "drive-schema-v1",
        ),
        (config(), "drive-schema-v2"),
    ] {
        let different = DriveSource::open_with_transport(
            PinnedDriveSourceSpec::new(different_config, schema_version).unwrap(),
            DriveRuntimeSecrets::new(
                DriveAccessToken::new("fixture-access-token").unwrap(),
                DriveCursorKey::new([0x41; 32]),
            ),
            Arc::new(ScriptedTransport::new([])),
        )
        .unwrap();
        assert!(matches!(
            different
                .execute_page(
                    &plan(&different, &["id"]),
                    Some(&cursor),
                    &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
                )
                .await,
            Err(SourceError::InvalidCursor)
        ));
    }

    let mut tampered = cursor.as_bytes().to_vec();
    let index = tampered.len() / 2;
    tampered[index] ^= 0x01;
    let tampered = SourceCursor::new(tampered).unwrap();
    assert!(matches!(
        same_source
            .execute_page(
                &plan(&same_source, &["id"]),
                Some(&tampered),
                &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
            )
            .await,
        Err(SourceError::InvalidCursor)
    ));

    let cross_kind = DriveChangeCursor::from_bytes(cursor.as_bytes().to_vec()).unwrap();
    assert_eq!(
        same_source
            .read_changes(
                &cross_kind,
                &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
            )
            .await,
        Err(SourceError::InvalidCursor)
    );
}

#[tokio::test]
async fn stale_cursor_replay_cannot_consume_the_live_continuation() {
    let first = listing(
        serde_json::json!([{
            "id":"first",
            "name":"first.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:56Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        Some("page-two"),
        false,
    );
    let second = listing(
        serde_json::json!([{
            "id":"second",
            "name":"second.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:57Z",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        Some("page-three"),
        false,
    );
    let (source, _) = source(
        ScriptedTransport::new([
            response(200, first),
            response(200, second),
            response(200, listing(serde_json::json!([]), None, false)),
        ]),
        config(),
    );
    let query_id = uuid::Uuid::new_v4();
    let first_page = source
        .execute_page(
            &plan(&source, &["id"]),
            None,
            &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
        )
        .await
        .unwrap();
    let first_cursor = first_page.next_cursor.unwrap();
    let second_page = source
        .execute_page(
            &plan(&source, &["id"]),
            Some(&first_cursor),
            &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
        )
        .await
        .unwrap();
    let live_cursor = second_page.next_cursor.unwrap();

    assert_eq!(
        source
            .execute_page(
                &plan(&source, &["id"]),
                Some(&first_cursor),
                &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
            )
            .await,
        Err(SourceError::InvalidCursor)
    );
    let final_page = source
        .execute_page(
            &plan(&source, &["id"]),
            Some(&live_cursor),
            &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(final_page.scanned, 0);
    assert!(final_page.next_cursor.is_none());
}

#[tokio::test]
async fn body_projection_exports_google_docs_downloads_text_and_leaves_binaries_metadata_only() {
    let body = listing(
        serde_json::json!([
            {
                "id":"gdoc",
                "name":"Research",
                "mimeType":"application/vnd.google-apps.document",
                "modifiedTime":"2026-07-20T12:34:56Z",
                "parents":["selected-folder"],
                "trashed":false
            },
            {
                "id":"markdown",
                "name":"notes.md",
                "mimeType":"text/markdown",
                "modifiedTime":"2026-07-20T12:34:57Z",
                "size":"8",
                "parents":["selected-folder"],
                "trashed":false
            },
            {
                "id":"pdf",
                "name":"paper.pdf",
                "mimeType":"application/pdf",
                "modifiedTime":"2026-07-20T12:34:58Z",
                "size":"99",
                "parents":["selected-folder"],
                "trashed":false
            }
        ]),
        None,
        false,
    );
    let (source, transport) = source(
        ScriptedTransport::new([
            response(200, body),
            response(200, b"Google body".to_vec()),
            response(200, b"# Notes".to_vec()),
        ]),
        config(),
    );
    let page = source
        .execute_page(
            &plan(&source, &["id", "name", "mimeType", "body"]),
            None,
            &context(10, 10, 64 * 1024),
        )
        .await
        .unwrap();

    assert_eq!(page.rows[0].value["body"], "Google body");
    assert_eq!(page.rows[1].value["body"], "# Notes");
    assert!(page.rows[2].value.get("body").is_none());
    let requests = transport.requests();
    assert!(requests
        .iter()
        .any(|request| { request.path() == "/drive/v3/files/gdoc/export?mimeType=text%2Fplain" }));
    assert!(requests.iter().any(|request| {
        request.path() == "/drive/v3/files/markdown?alt=media&supportsAllDrives=true"
    }));
    assert!(!requests
        .iter()
        .any(|request| request.path().contains("/files/pdf?alt=media")));
}

#[tokio::test]
async fn projected_body_cannot_cross_the_context_byte_bound() {
    let body = listing(
        serde_json::json!([{
            "id":"markdown",
            "name":"notes.md",
            "mimeType":"text/markdown",
            "modifiedTime":"2026-07-20T12:34:57Z",
            "size":"4096",
            "parents":["selected-folder"],
            "trashed":false
        }]),
        None,
        false,
    );
    let (source, _) = source(
        ScriptedTransport::new([response(200, body), response(200, vec![b'x'; 256])]),
        config(),
    );
    assert_eq!(
        source
            .execute_page(&plan(&source, &["id", "body"]), None, &context(2, 2, 300),)
            .await,
        Err(SourceError::ByteBudgetExceeded)
    );
}

#[tokio::test]
async fn revoked_credentials_rate_limits_and_cancellation_remain_distinct() {
    let (unauthorized, _) = source(
        ScriptedTransport::new([response(401, br#"{"error":{"code":401}}"#)]),
        config(),
    );
    assert!(matches!(
        unauthorized
            .execute_page(&plan(&unauthorized, &["id"]), None, &context(2, 2, 1024),)
            .await,
        Err(SourceError::Authentication { .. })
    ));

    let (rate_limited, _) = source(
        ScriptedTransport::new([response_with_headers(
            429,
            [("retry-after", "7")],
            br#"{"error":{"code":429}}"#,
        )]),
        config(),
    );
    assert!(matches!(
        rate_limited
            .execute_page(&plan(&rate_limited, &["id"]), None, &context(2, 2, 1024),)
            .await,
        Err(SourceError::RateLimited {
            retry_after_ms: Some(7_000),
            ..
        })
    ));

    let (pending, _) = source(ScriptedTransport::new([ScriptStep::Pending]), config());
    let cancellation = CancellationToken::new();
    let cancel_later = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_later.cancel();
    });
    let context = context_with_id(uuid::Uuid::new_v4(), 2, 2, 1024, cancellation);
    assert_eq!(
        pending
            .execute_page(&plan(&pending, &["id"]), None, &context)
            .await,
        Err(SourceError::Cancelled)
    );
}

#[tokio::test]
async fn shared_drive_listing_declares_all_required_drive_flags() {
    let (source, transport) = source(
        ScriptedTransport::new([response(200, listing(serde_json::json!([]), None, false))]),
        shared_drive_config(),
    );
    source
        .execute_page(&plan(&source, &["id"]), None, &context(2, 2, 1024))
        .await
        .unwrap();
    let path = transport.requests()[1].path().to_string();
    assert!(path.contains("corpora=drive"));
    assert!(path.contains("driveId=shared-drive-7"));
    assert!(path.contains("includeItemsFromAllDrives=true"));
    assert!(path.contains("supportsAllDrives=true"));
}

#[tokio::test]
async fn changes_page_preserves_deletion_unchanged_and_new_checkpoint_semantics() {
    let start = br#"{"startPageToken":"changes-100"}"#;
    let changed = serde_json::to_vec(&serde_json::json!({
        "changes": [
            {
                "changeType":"file",
                "removed":true,
                "fileId":"gone"
            },
            {
                "changeType":"file",
                "removed":false,
                "fileId":"same",
                "file":{
                    "id":"same",
                    "name":"same.md",
                    "mimeType":"text/markdown",
                    "modifiedTime":"2026-07-20T12:34:56Z",
                    "md5Checksum":"same-checksum",
                    "parents":["selected-folder"],
                    "trashed":false
                }
            }
        ],
        "nextPageToken":"changes-101"
    }))
    .unwrap();
    let final_page = serde_json::to_vec(&serde_json::json!({
        "changes": [],
        "newStartPageToken":"changes-200"
    }))
    .unwrap();
    let (source, transport) = source(
        ScriptedTransport::new([
            response(200, start),
            response(200, changed),
            response(200, final_page),
        ]),
        config(),
    );
    let initial = source
        .start_change_tracking(&context(20, 20, 64 * 1024))
        .await
        .unwrap();
    let first = source
        .read_changes(&initial, &context(20, 20, 64 * 1024))
        .await
        .unwrap();
    let DriveChangeOutcome::Page {
        events,
        next_cursor,
        checkpoint,
    } = first
    else {
        panic!("expected a changes page");
    };
    assert_eq!(
        events,
        vec![
            DriveChangeEvent::Deleted {
                file_id: "gone".to_string()
            },
            DriveChangeEvent::Upserted {
                file_id: "same".to_string()
            }
        ]
    );
    assert!(checkpoint.is_none());
    let second_cursor = next_cursor.expect("provider returned another page");
    let second = source
        .read_changes(&second_cursor, &context(20, 20, 64 * 1024))
        .await
        .unwrap();
    let DriveChangeOutcome::Page {
        events,
        next_cursor,
        checkpoint,
    } = second
    else {
        panic!("expected final changes page");
    };
    assert!(events.is_empty());
    assert!(next_cursor.is_none());
    assert!(checkpoint.is_some());
    assert!(transport.requests()[1]
        .path()
        .contains("pageToken=changes-100"));
    assert!(transport.requests()[2]
        .path()
        .contains("pageToken=changes-101"));
}

#[tokio::test]
async fn change_cursor_is_bound_to_source_selection_schema_domain_and_authentication() {
    let (tracker, _) = source(
        ScriptedTransport::new([response(200, br#"{"startPageToken":"changes-100"}"#)]),
        config(),
    );
    let cursor = tracker
        .start_change_tracking(&context(2, 2, 4_096))
        .await
        .unwrap();

    for (different_config, schema_version) in [
        (
            DriveSourceConfig::new(
                "other-drive-source",
                "drive",
                "Document",
                vec![DriveSelection::folder("selected-folder")],
                DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
            )
            .unwrap(),
            "drive-schema-v1",
        ),
        (
            DriveSourceConfig::new(
                "drive-library",
                "drive",
                "Document",
                vec![DriveSelection::folder("different-selection")],
                DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
            )
            .unwrap(),
            "drive-schema-v1",
        ),
        (config(), "drive-schema-v2"),
    ] {
        let different = DriveSource::open_with_transport(
            PinnedDriveSourceSpec::new(different_config, schema_version).unwrap(),
            DriveRuntimeSecrets::new(
                DriveAccessToken::new("fixture-access-token").unwrap(),
                DriveCursorKey::new([0x41; 32]),
            ),
            Arc::new(ScriptedTransport::new([])),
        )
        .unwrap();
        assert_eq!(
            different.read_changes(&cursor, &context(2, 2, 4_096)).await,
            Err(SourceError::InvalidCursor)
        );
    }

    let mut tampered = cursor.as_bytes().to_vec();
    let index = tampered.len() / 2;
    tampered[index] ^= 0x01;
    let tampered = DriveChangeCursor::from_bytes(tampered).unwrap();
    assert_eq!(
        tracker.read_changes(&tampered, &context(2, 2, 4_096)).await,
        Err(SourceError::InvalidCursor)
    );

    let query_page = source(
        ScriptedTransport::new([response(
            200,
            listing(
                serde_json::json!([{
                    "id":"a",
                    "name":"a.md",
                    "mimeType":"text/markdown",
                    "modifiedTime":"2026-07-20T12:34:56Z",
                    "parents":["selected-folder"],
                    "trashed":false
                }]),
                Some("query-next"),
                false,
            ),
        )]),
        config(),
    )
    .0;
    let query_id = uuid::Uuid::new_v4();
    let query_cursor = query_page
        .execute_page(
            &plan(&query_page, &["id"]),
            None,
            &context_with_id(query_id, 1, 4, 4_096, CancellationToken::new()),
        )
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    let cross_kind = DriveChangeCursor::from_bytes(query_cursor.as_bytes().to_vec()).unwrap();
    assert_eq!(
        tracker
            .read_changes(&cross_kind, &context(2, 2, 4_096))
            .await,
        Err(SourceError::InvalidCursor)
    );

    let change_as_query = SourceCursor::new(cursor.as_bytes().to_vec()).unwrap();
    assert_eq!(
        query_page
            .execute_page(
                &plan(&query_page, &["id"]),
                Some(&change_as_query),
                &context_with_id(query_id, 1, 4, 4_096, CancellationToken::new()),
            )
            .await,
        Err(SourceError::InvalidCursor)
    );
}

#[tokio::test]
async fn malformed_change_continuation_never_crosses_into_a_followup_request() {
    let (source, transport) = source(
        ScriptedTransport::new([
            response(200, br#"{"startPageToken":"changes-100"}"#),
            response(200, br#"{"changes":[],"nextPageToken":"bad\u000Apage"}"#),
        ]),
        config(),
    );
    let cursor = source
        .start_change_tracking(&context(2, 2, 4_096))
        .await
        .unwrap();
    assert_eq!(
        source.read_changes(&cursor, &context(2, 2, 4_096)).await,
        Err(SourceError::InvalidCursor)
    );
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn expired_change_token_requires_rebuild_instead_of_empty_success() {
    let start = br#"{"startPageToken":"expired"}"#;
    let (source, _) = source(
        ScriptedTransport::new([
            response(200, start),
            response(410, br#"{"error":{"code":410,"message":"Gone"}}"#),
        ]),
        config(),
    );
    let cursor = source
        .start_change_tracking(&context(2, 2, 4096))
        .await
        .unwrap();
    assert_eq!(
        source
            .read_changes(&cursor, &context(2, 2, 4096))
            .await
            .unwrap(),
        DriveChangeOutcome::RebuildRequired
    );
}
