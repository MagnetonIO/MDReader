use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use mdreader_connector_dropbox::{
    DropboxAccessGrant, DropboxAccessToken, DropboxAuthorizationMode, DropboxAuthorizationProfile,
    DropboxAvailability, DropboxChangeEvent, DropboxChangeOutcome, DropboxConnectorError,
    DropboxContentScope, DropboxCursorKey, DropboxDocumentError, DropboxEndpoint,
    DropboxHttpMethod, DropboxHttpRequest, DropboxHttpResponse, DropboxOAuthPkce,
    DropboxRefreshCredential, DropboxRevision, DropboxRuntimeSecrets, DropboxSecretReference,
    DropboxSecretResolver, DropboxSource, DropboxSourceConfig, DropboxTransport,
    DropboxTransportError, PinnedDropboxSourceSpec, DROPBOX_CONTENT_READ_SCOPE,
    DROPBOX_METADATA_READ_SCOPE,
};
use catdb_sdk::connectors::{
    read, CancellationToken, DataSource, ReadContext, SourceCursor, SourceError,
};
use catdb_sdk::core::BaseType;
use catdb_sdk::query::{AttrOp, AttrValue, EntityFilter, EntityQuery, EntityTarget};

#[derive(Clone)]
struct ScriptedTransport {
    state: Arc<Mutex<ScriptState>>,
}

struct ScriptState {
    steps: VecDeque<ScriptStep>,
    requests: Vec<DropboxHttpRequest>,
    accepted_writes: usize,
}

enum ScriptStep {
    Response(DropboxHttpResponse),
    Pending,
    AcceptedWritePending,
    TransportError(DropboxTransportError),
}

impl ScriptedTransport {
    fn new(steps: impl IntoIterator<Item = ScriptStep>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptState {
                steps: steps.into_iter().collect(),
                requests: Vec::new(),
                accepted_writes: 0,
            })),
        }
    }

    fn requests(&self) -> Vec<DropboxHttpRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    fn accepted_writes(&self) -> usize {
        self.state.lock().unwrap().accepted_writes
    }
}

#[async_trait]
impl DropboxTransport for ScriptedTransport {
    async fn send(
        &self,
        request: DropboxHttpRequest,
    ) -> Result<DropboxHttpResponse, DropboxTransportError> {
        let step = {
            let mut state = self.state.lock().unwrap();
            state.requests.push(request);
            let step = state.steps.pop_front().expect("fixture script exhausted");
            if matches!(step, ScriptStep::AcceptedWritePending) {
                state.accepted_writes += 1;
            }
            step
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
struct DropboxSelectionRaceTransport {
    state: Arc<Mutex<DropboxSelectionRaceState>>,
}

struct DropboxSelectionRaceState {
    requests: Vec<DropboxHttpRequest>,
    rebound_after_preflight: bool,
    accepted_writes: usize,
}

impl DropboxSelectionRaceTransport {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DropboxSelectionRaceState {
                requests: Vec::new(),
                rebound_after_preflight: false,
                accepted_writes: 0,
            })),
        }
    }

    fn requests(&self) -> Vec<DropboxHttpRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    fn rebound_after_preflight(&self) -> bool {
        self.state.lock().unwrap().rebound_after_preflight
    }

    fn accepted_writes(&self) -> usize {
        self.state.lock().unwrap().accepted_writes
    }
}

#[async_trait]
impl DropboxTransport for DropboxSelectionRaceTransport {
    async fn send(
        &self,
        request: DropboxHttpRequest,
    ) -> Result<DropboxHttpResponse, DropboxTransportError> {
        let mut state = self.state.lock().unwrap();
        state.requests.push(request.clone());
        if request.path() == "/2/files/get_metadata" {
            state.rebound_after_preflight = true;
            return DropboxHttpResponse::new(
                200,
                BTreeMap::new(),
                file("/knowledge/note.md", "note.md", "rev-1", "hash-1")
                    .to_string()
                    .into_bytes(),
            );
        }
        if request.path() == "/2/files/upload" {
            let argument = serde_json::from_str::<serde_json::Value>(
                request.header("Dropbox-API-Arg").expect("upload argument"),
            )
            .expect("valid upload argument");
            if state.rebound_after_preflight
                && argument["path"] == "/knowledge/note.md"
                && argument["mode"] == serde_json::json!({".tag": "update", "update": "rev-1"})
                && argument["strict_conflict"] == true
                && argument["autorename"] == false
            {
                return DropboxHttpResponse::new(
                    409,
                    BTreeMap::new(),
                    b"rebound-and-revised".to_vec(),
                );
            }
            state.accepted_writes += 1;
            return DropboxHttpResponse::new(200, BTreeMap::new(), Vec::new());
        }
        panic!("unexpected Dropbox selection-race request: {:?}", request);
    }
}

fn response(status: u16, body: impl Into<Vec<u8>>) -> ScriptStep {
    ScriptStep::Response(
        DropboxHttpResponse::new(status, BTreeMap::new(), body.into())
            .expect("bounded fixture response"),
    )
}

fn response_with_headers(
    status: u16,
    headers: impl IntoIterator<Item = (&'static str, String)>,
    body: impl Into<Vec<u8>>,
) -> ScriptStep {
    ScriptStep::Response(
        DropboxHttpResponse::new(
            status,
            headers
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect(),
            body.into(),
        )
        .expect("bounded fixture response"),
    )
}

fn config() -> DropboxSourceConfig {
    DropboxSourceConfig::new(
        "dropbox-library",
        "dropbox",
        "Document",
        "/knowledge",
        DropboxAuthorizationMode::FullDropbox,
        DropboxSecretReference::new("keychain/mdreader/dropbox").unwrap(),
    )
    .unwrap()
}

fn markdown_config() -> DropboxSourceConfig {
    config().with_content_scope(DropboxContentScope::Markdown)
}

fn open_source(transport: ScriptedTransport) -> (DropboxSource, ScriptedTransport) {
    open_source_with_config(transport, config())
}

fn open_source_with_config(
    transport: ScriptedTransport,
    config: DropboxSourceConfig,
) -> (DropboxSource, ScriptedTransport) {
    let source = DropboxSource::open_with_transport(
        PinnedDropboxSourceSpec::new(config, "dropbox-schema-v1").unwrap(),
        DropboxRuntimeSecrets::new(
            DropboxAccessToken::new("fixture-access-token").unwrap(),
            DropboxCursorKey::new([0x51; 32]),
        ),
        Arc::new(transport.clone()),
    )
    .unwrap();
    (source, transport)
}

fn query() -> EntityQuery {
    EntityQuery {
        target: EntityTarget::Entity("dropbox.Document".to_string()),
        filters: Vec::new(),
    }
}

fn exact_path_query(path: &str) -> EntityQuery {
    EntityQuery {
        target: EntityTarget::Entity("dropbox.Document".to_string()),
        filters: vec![EntityFilter {
            attr: "path".to_string(),
            op: AttrOp::Eq,
            value: AttrValue::Text(path.to_string()),
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

fn plan(source: &DropboxSource, projected: &[&str]) -> catdb_sdk::connectors::SourcePlan {
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

fn list_page(entries: serde_json::Value, cursor: &str, has_more: bool) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "entries": entries,
        "cursor": cursor,
        "has_more": has_more
    }))
    .unwrap()
}

fn file(path: &str, name: &str, rev: &str, content_hash: &str) -> serde_json::Value {
    serde_json::json!({
        ".tag": "file",
        "name": name,
        "path_lower": path,
        "path_display": path,
        "id": format!("id:{name}"),
        "client_modified": "2026-07-20T12:34:56Z",
        "server_modified": "2026-07-20T12:34:57Z",
        "rev": rev,
        "size": 22,
        "content_hash": content_hash
    })
}

fn download_header(path: &str, name: &str, rev: &str, content_hash: &str) -> String {
    file(path, name, rev, content_hash).to_string()
}

fn provider_download_header(path: &str, name: &str, rev: &str, content_hash: &str) -> String {
    let mut metadata = file(path, name, rev, content_hash);
    metadata
        .as_object_mut()
        .expect("file metadata object")
        .remove(".tag");
    metadata.to_string()
}

#[tokio::test]
async fn sync_metadata_uses_dropbox_metadata_without_a_content_download() {
    let (source, transport) = open_source_with_config(
        ScriptedTransport::new([response(
            200,
            file("/knowledge/sync.md", "sync.md", "r1", "hash-1").to_string(),
        )]),
        markdown_config(),
    );

    let row = source
        .metadata_for_sync("/knowledge/sync.md", &context(10, 10, 64 * 1024))
        .await
        .expect("exact metadata");

    assert_eq!(row.key, "/knowledge/sync.md");
    assert_eq!(row.value["name"], "sync.md");
    assert_eq!(row.value["rev"], "r1");
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].endpoint(), DropboxEndpoint::Api);
    assert_eq!(requests[0].path(), "/2/files/get_metadata");
    assert_eq!(requests[0].json_body()["path"], "/knowledge/sync.md");
}

#[tokio::test]
async fn sync_metadata_preserves_typed_dropbox_provider_failures() {
    let cases = [
        (404, None, "not-found"),
        (401, None, "authentication"),
        (429, Some("7".to_string()), "rate-limited"),
        (500, None, "transport"),
    ];
    for (status, retry_after, expected) in cases {
        let step = match retry_after {
            Some(value) => response_with_headers(status, [("retry-after", value)], Vec::new()),
            None => response(status, Vec::new()),
        };
        let (source, transport) =
            open_source_with_config(ScriptedTransport::new([step]), markdown_config());
        let error = source
            .metadata_for_sync("/knowledge/sync.md", &context(10, 10, 64 * 1024))
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
        assert_eq!(
            transport.requests().len(),
            1,
            "one metadata endpoint request"
        );
    }
}

#[tokio::test]
async fn downloads_the_exact_markdown_record_and_revision() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            download_header("/knowledge/note.md", "note.md", "rev-1", "hash-1"),
        )],
        b"# one".to_vec(),
    )]);
    let (source, transport) = open_source_with_config(transport, markdown_config());

    let document = source
        .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
        .await
        .expect("download");

    assert_eq!(document.record_key(), "id:note.md");
    assert_eq!(document.revision().as_str(), "rev-1");
    assert_eq!(document.content_hash(), Some("hash-1"));
    assert_eq!(document.body(), b"# one");
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            requests[0]
                .header("Dropbox-API-Arg")
                .expect("download argument")
        )
        .unwrap()["path"],
        "id:note.md"
    );
}

#[tokio::test]
async fn download_accepts_provider_file_metadata_without_a_union_tag() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            provider_download_header("/knowledge/note.md", "note.md", "rev-1", "hash-1"),
        )],
        b"# one".to_vec(),
    )]);
    let (source, _) = open_source_with_config(transport, markdown_config());

    let document = source
        .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
        .await
        .expect("Dropbox download metadata does not carry the list-entry union tag");

    assert_eq!(document.record_key(), "id:note.md");
    assert_eq!(document.revision().as_str(), "rev-1");
    assert_eq!(document.body(), b"# one");
}

#[tokio::test]
async fn download_accepts_the_materialized_path_as_record_identity() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            provider_download_header("/knowledge/note.md", "note.md", "rev-1", "hash-1"),
        )],
        b"# one".to_vec(),
    )]);
    let (source, _) = open_source_with_config(transport, markdown_config());

    let document = source
        .download_markdown("/knowledge/note.md", &context(10, 10, 64 * 1024))
        .await
        .expect("materialized Dropbox rows use the provider path as their record key");

    assert_eq!(document.record_key(), "/knowledge/note.md");
    assert_eq!(document.body(), b"# one");
}

#[tokio::test]
async fn download_refuses_a_response_identity_other_than_the_requested_record() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            download_header("/knowledge/other.md", "other.md", "rev-9", "hash-9"),
        )],
        b"# other".to_vec(),
    )]);
    let (source, transport) = open_source_with_config(transport, markdown_config());

    let error = source
        .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
        .await
        .expect_err("response identity must match the requested record");

    assert!(matches!(
        error,
        DropboxDocumentError::IdentityMismatch { .. }
    ));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn download_refuses_a_non_markdown_name() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            download_header("/knowledge/note.txt", "note.txt", "rev-1", "hash-1"),
        )],
        b"plain text".to_vec(),
    )]);
    let (source, transport) = open_source_with_config(transport, markdown_config());

    let error = source
        .download_markdown("id:note.txt", &context(10, 10, 64 * 1024))
        .await
        .expect_err("only Markdown names are admitted");

    assert!(matches!(error, DropboxDocumentError::Protocol { .. }));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn download_refuses_a_markdown_body_larger_than_two_mebibytes() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            download_header("/knowledge/note.md", "note.md", "rev-1", "hash-1"),
        )],
        vec![b'a'; 2 * 1024 * 1024 + 1],
    )]);
    let (source, _) = open_source_with_config(transport, markdown_config());

    let error = source
        .download_markdown("id:note.md", &context(10, 10, 4 * 1024 * 1024))
        .await
        .expect_err("Markdown bodies are capped at two mebibytes");

    assert!(matches!(error, DropboxDocumentError::TooLarge { .. }));
}

#[tokio::test]
async fn download_refuses_a_non_utf8_markdown_body() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            download_header(
                "/knowledge/note.markdown",
                "note.markdown",
                "rev-1",
                "hash-1",
            ),
        )],
        vec![0xff, 0xfe],
    )]);
    let (source, _) = open_source_with_config(transport, markdown_config());

    let error = source
        .download_markdown("id:note.markdown", &context(10, 10, 64 * 1024))
        .await
        .expect_err("Markdown bodies must be UTF-8");

    assert!(matches!(error, DropboxDocumentError::InvalidUtf8 { .. }));
}

#[tokio::test]
async fn download_refuses_a_success_without_revision_authority() {
    let mut metadata = file("/knowledge/note.md", "note.md", "rev-1", "hash-1");
    metadata.as_object_mut().unwrap().remove("rev");
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [("Dropbox-API-Result", metadata.to_string())],
        b"# one".to_vec(),
    )]);
    let (source, transport) = open_source_with_config(transport, markdown_config());

    let error = source
        .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
        .await
        .expect_err("a successful document response still needs revision authority");

    assert!(matches!(error, DropboxDocumentError::Protocol { .. }));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn document_download_preserves_deadline_and_cancellation_as_typed_source_errors() {
    let (deadline_source, _) = open_source_with_config(
        ScriptedTransport::new([ScriptStep::Pending]),
        markdown_config(),
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
            .download_markdown("id:note.md", &deadline_context)
            .await,
        Err(DropboxDocumentError::Timeout)
    );

    let (cancelled_source, _) =
        open_source_with_config(ScriptedTransport::new([]), markdown_config());
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled_context = context_with_id(uuid::Uuid::new_v4(), 10, 10, 64 * 1024, cancellation);
    assert_eq!(
        cancelled_source
            .download_markdown("id:note.md", &cancelled_context)
            .await,
        Err(DropboxDocumentError::Cancelled)
    );
}

#[tokio::test]
async fn document_http_failures_remain_structurally_mappable() {
    let (not_found_source, _) = open_source_with_config(
        ScriptedTransport::new([response(404, Vec::new())]),
        markdown_config(),
    );
    assert!(matches!(
        not_found_source
            .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
            .await,
        Err(DropboxDocumentError::NotFound { .. })
    ));

    let (authentication_source, _) = open_source_with_config(
        ScriptedTransport::new([response(401, Vec::new())]),
        markdown_config(),
    );
    assert!(matches!(
        authentication_source
            .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
            .await,
        Err(DropboxDocumentError::Authentication { .. })
    ));

    let (timeout_source, _) = open_source_with_config(
        ScriptedTransport::new([response(408, Vec::new())]),
        markdown_config(),
    );
    assert!(matches!(
        timeout_source
            .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
            .await,
        Err(DropboxDocumentError::Timeout)
    ));

    let (rate_limited_source, _) = open_source_with_config(
        ScriptedTransport::new([response_with_headers(
            429,
            [("retry-after", "13".to_string())],
            Vec::new(),
        )]),
        markdown_config(),
    );
    assert!(matches!(
        rate_limited_source
            .download_markdown("id:note.md", &context(10, 10, 64 * 1024))
            .await,
        Err(DropboxDocumentError::RateLimited {
            retry_after_ms: Some(13_000),
            ..
        })
    ));
}

#[tokio::test]
async fn stale_revision_is_a_conflict_and_never_creates_a_second_dropbox_record() {
    let transport = ScriptedTransport::new([
        response(
            200,
            file("/knowledge/note.md", "note.md", "rev-2", "hash-2").to_string(),
        ),
        response(409, b"provider-conflict".to_vec()),
    ]);
    let (source, transport) = open_source_with_config(transport, markdown_config());

    let error = source
        .update_markdown(
            "id:note.md",
            &DropboxRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect_err("a stale revision must conflict");

    assert!(matches!(error, DropboxDocumentError::Conflict { .. }));
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].endpoint(), DropboxEndpoint::Api);
    assert_eq!(requests[0].path(), "/2/files/get_metadata");
    assert_eq!(requests[1].endpoint(), DropboxEndpoint::Content);
    assert_eq!(requests[1].path(), "/2/files/upload");
    let argument = serde_json::from_str::<serde_json::Value>(
        requests[1]
            .header("Dropbox-API-Arg")
            .expect("upload argument"),
    )
    .unwrap();
    assert_eq!(argument["path"], "/knowledge/note.md");
    assert_eq!(
        argument["mode"],
        serde_json::json!({".tag": "update", "update": "rev-1"})
    );
    assert_eq!(argument["strict_conflict"], true);
    assert_eq!(argument["autorename"], false);
    assert_eq!(requests[1].body(), b"# edited");
}

#[tokio::test]
async fn guarded_dropbox_update_treats_ambiguous_http_receipts_as_unknown_without_a_retry() {
    let metadata = || {
        response(
            200,
            file("/knowledge/note.md", "note.md", "rev-1", "hash-1").to_string(),
        )
    };

    for status in [408, 504, 500, 503] {
        let transport = ScriptedTransport::new([metadata(), response(status, Vec::new())]);
        let (source, transport) = open_source_with_config(transport, markdown_config());

        let error = source
            .update_markdown(
                "id:note.md",
                &DropboxRevision::new("rev-1").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect_err("an ambiguous post-dispatch HTTP response cannot prove the write failed");

        let DropboxDocumentError::WriteOutcomeUnknown { reconciliation } = error else {
            panic!("HTTP {status} after a Dropbox mutation must preserve an unknown outcome");
        };
        assert_eq!(reconciliation.record_fingerprint().len(), 43);
        let requests = transport.requests();
        assert_eq!(requests.len(), 2, "HTTP {status} must not trigger a retry");
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path() == "/2/files/upload")
                .count(),
            1,
            "HTTP {status} must dispatch exactly one guarded mutation"
        );
    }
}

#[tokio::test]
async fn guarded_dropbox_update_keeps_definitive_post_dispatch_statuses_typed() {
    let metadata = || {
        response(
            200,
            file("/knowledge/note.md", "note.md", "rev-1", "hash-1").to_string(),
        )
    };

    for status in [404, 401, 403, 429] {
        let receipt = if status == 429 {
            response_with_headers(status, [("retry-after", "7".to_string())], Vec::new())
        } else {
            response(status, Vec::new())
        };
        let (source, transport) = open_source_with_config(
            ScriptedTransport::new([metadata(), receipt]),
            markdown_config(),
        );

        let error = source
            .update_markdown(
                "id:note.md",
                &DropboxRevision::new("rev-1").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect_err("a provider-declared rejected mutation remains definite");

        assert!(
            match status {
                404 => matches!(error, DropboxDocumentError::NotFound { .. }),
                401 | 403 => matches!(error, DropboxDocumentError::Authentication { .. }),
                429 => matches!(
                    error,
                    DropboxDocumentError::RateLimited {
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
                .filter(|request| request.path() == "/2/files/upload")
                .count(),
            1,
            "HTTP {status} must not trigger a second mutation"
        );
    }
}

#[tokio::test]
async fn guarded_dropbox_update_refuses_a_rebind_outside_selection_between_preflight_and_upload() {
    let transport = DropboxSelectionRaceTransport::new();
    let source = DropboxSource::open_with_transport(
        PinnedDropboxSourceSpec::new(markdown_config(), "dropbox-schema-v1").unwrap(),
        DropboxRuntimeSecrets::new(
            DropboxAccessToken::new("fixture-access-token").unwrap(),
            DropboxCursorKey::new([0x51; 32]),
        ),
        Arc::new(transport.clone()),
    )
    .unwrap();

    let error = source
        .update_markdown(
            "id:note.md",
            &DropboxRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect_err("the guarded upload must reject the rebound record");

    assert!(matches!(error, DropboxDocumentError::Conflict { .. }));
    assert!(transport.rebound_after_preflight());
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    let upload = &requests[1];
    let argument = serde_json::from_str::<serde_json::Value>(
        upload.header("Dropbox-API-Arg").expect("upload argument"),
    )
    .unwrap();
    assert_eq!(argument["path"], "/knowledge/note.md");
    assert_eq!(
        argument["mode"],
        serde_json::json!({".tag": "update", "update": "rev-1"})
    );
    assert_eq!(argument["strict_conflict"], true);
    assert_eq!(argument["autorename"], false);
    assert_eq!(transport.accepted_writes(), 0);
}

#[tokio::test]
async fn dropbox_write_after_provider_acceptance_returns_unknown_on_cancellation_or_deadline() {
    let metadata = || {
        response(
            200,
            file("/knowledge/note.md", "note.md", "rev-1", "hash-1").to_string(),
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
        open_source_with_config(cancellation_transport, markdown_config());
    let cancellation_error = cancellation_source
        .update_markdown(
            "id:note.md",
            &DropboxRevision::new("rev-1").unwrap(),
            b"# edited",
            &context_with_id(uuid::Uuid::new_v4(), 10, 10, 64 * 1024, cancellation),
        )
        .await
        .expect_err("a cancellation after dispatch cannot prove the write failed");
    let DropboxDocumentError::WriteOutcomeUnknown { reconciliation } = cancellation_error else {
        panic!("a post-dispatch cancellation must preserve an unknown outcome");
    };
    assert_eq!(reconciliation.record_fingerprint().len(), 43);
    assert!(!format!("{reconciliation:?}").contains("id:note.md"));
    assert_eq!(cancellation_transport.accepted_writes(), 1);

    let deadline_transport = ScriptedTransport::new([metadata(), ScriptStep::AcceptedWritePending]);
    let (deadline_source, deadline_transport) =
        open_source_with_config(deadline_transport, markdown_config());
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
            "id:note.md",
            &DropboxRevision::new("rev-1").unwrap(),
            b"# edited",
            &deadline_context,
        )
        .await
        .expect_err("a deadline after dispatch cannot prove the write failed");
    assert!(matches!(
        deadline_error,
        DropboxDocumentError::WriteOutcomeUnknown { .. }
    ));
    assert_eq!(deadline_transport.accepted_writes(), 1);
}

#[tokio::test]
async fn dropbox_write_before_dispatch_keeps_cancellation_and_deadline_definite() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancellation_transport = ScriptedTransport::new([]);
    let (cancellation_source, cancellation_transport) =
        open_source_with_config(cancellation_transport, markdown_config());
    assert_eq!(
        cancellation_source
            .update_markdown(
                "id:note.md",
                &DropboxRevision::new("rev-1").unwrap(),
                b"# edited",
                &context_with_id(uuid::Uuid::new_v4(), 10, 10, 64 * 1024, cancellation),
            )
            .await,
        Err(DropboxDocumentError::Cancelled)
    );
    assert!(cancellation_transport.requests().is_empty());

    let deadline_transport = ScriptedTransport::new([]);
    let (deadline_source, deadline_transport) =
        open_source_with_config(deadline_transport, markdown_config());
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
                "id:note.md",
                &DropboxRevision::new("rev-1").unwrap(),
                b"# edited",
                &deadline_context,
            )
            .await,
        Err(DropboxDocumentError::Timeout)
    );
    assert!(deadline_transport.requests().is_empty());
}

#[tokio::test]
async fn dropbox_write_with_an_unusable_success_receipt_returns_unknown_without_a_retry() {
    let metadata = || {
        response(
            200,
            file("/knowledge/note.md", "note.md", "rev-1", "hash-1").to_string(),
        )
    };

    for receipt in [
        response(200, Vec::new()),
        ScriptStep::TransportError(DropboxTransportError::ResponseTooLarge),
    ] {
        let transport = ScriptedTransport::new([metadata(), receipt]);
        let (source, transport) = open_source_with_config(transport, markdown_config());
        let error = source
            .update_markdown(
                "id:note.md",
                &DropboxRevision::new("rev-1").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect_err("an unusable receipt leaves the accepted write unknown");
        assert!(matches!(
            error,
            DropboxDocumentError::WriteOutcomeUnknown { .. }
        ));
        assert_eq!(
            transport.requests().len(),
            2,
            "a guarded write receipt must never trigger a second mutation"
        );
    }
}

#[tokio::test]
async fn update_refuses_a_record_outside_the_selected_dropbox_path_before_uploading() {
    let transport = ScriptedTransport::new([
        response(
            200,
            file("/outside/note.md", "note.md", "rev-2", "hash-2").to_string(),
        ),
        response(
            200,
            file("/outside/note.md", "note.md", "rev-3", "hash-3").to_string(),
        ),
    ]);
    let (source, transport) = open_source_with_config(transport, markdown_config());

    let result = source
        .update_markdown(
            "id:note.md",
            &DropboxRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await;

    assert!(result.is_err(), "an out-of-selection write must be refused");
    assert!(
        transport
            .requests()
            .iter()
            .all(|request| request.path() != "/2/files/upload"),
        "source authorization must complete before the provider mutation"
    );
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn guarded_dropbox_update_returns_the_provider_revision_authority() {
    let transport = ScriptedTransport::new([
        response(
            200,
            file("/knowledge/note.md", "note.md", "rev-1", "hash-1").to_string(),
        ),
        response(
            200,
            file("/knowledge/note.md", "note.md", "rev-2", "hash-2").to_string(),
        ),
    ]);
    let (source, _) = open_source_with_config(transport, markdown_config());

    let receipt = source
        .update_markdown(
            "id:note.md",
            &DropboxRevision::new("rev-1").unwrap(),
            b"# edited",
            &context(10, 10, 64 * 1024),
        )
        .await
        .expect("guarded update");

    assert_eq!(receipt.record_key(), "id:note.md");
    assert_eq!(receipt.revision().as_str(), "rev-2");
}

#[test]
fn downloaded_document_and_write_receipt_debug_redact_provider_record_keys() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let mut downloaded_metadata = file(
            "/knowledge/note.md",
            "note.md",
            "download-revision",
            "hash-1",
        );
        downloaded_metadata["id"] = serde_json::json!("id:sensitive-record-key");
        let download_transport = ScriptedTransport::new([response_with_headers(
            200,
            [("Dropbox-API-Result", downloaded_metadata.to_string())],
            b"# sensitive body".to_vec(),
        )]);
        let (download_source, _) = open_source_with_config(download_transport, markdown_config());
        let document = download_source
            .download_markdown("id:sensitive-record-key", &context(10, 10, 64 * 1024))
            .await
            .expect("download");
        let rendered_document = format!("{document:?}");
        assert!(!rendered_document.contains("id:sensitive-record-key"));
        assert!(!rendered_document.contains("download-revision"));
        assert!(!rendered_document.contains("# sensitive body"));

        let mut updated_metadata = file(
            "/knowledge/note.md",
            "note.md",
            "updated-revision",
            "hash-2",
        );
        updated_metadata["id"] = serde_json::json!("id:sensitive-record-key");
        let update_transport = ScriptedTransport::new([
            response(200, updated_metadata.clone().to_string()),
            response(200, updated_metadata.to_string()),
        ]);
        let (update_source, _) = open_source_with_config(update_transport, markdown_config());
        let receipt = update_source
            .update_markdown(
                "id:sensitive-record-key",
                &DropboxRevision::new("expected-revision").unwrap(),
                b"# edited",
                &context(10, 10, 64 * 1024),
            )
            .await
            .expect("update");
        let rendered_receipt = format!("{receipt:?}");
        assert!(!rendered_receipt.contains("id:sensitive-record-key"));
        assert!(!rendered_receipt.contains("updated-revision"));
    });
}

#[test]
fn provider_response_debug_redacts_direct_urls_and_bodies() {
    let response = DropboxHttpResponse::new(
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
fn pkce_uses_exact_read_scopes_without_a_client_secret_or_fictional_folder_scope() {
    let pkce = DropboxOAuthPkce::new(
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        "state-123",
        "http://127.0.0.1:41004/oauth/callback",
    )
    .unwrap();
    assert_eq!(
        pkce.code_challenge(),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    let authorization = pkce.authorization_url("desktop-client").unwrap();
    assert!(authorization.contains("code_challenge_method=S256"));
    assert!(authorization.contains("token_access_type=offline"));
    assert!(authorization.contains("state=state-123"));
    assert!(authorization.contains("scope=files.metadata.read+files.content.read"));
    assert!(!authorization.contains("client_secret"));
    assert!(!authorization.contains("folder_scope"));
    assert_eq!(
        config().oauth_scopes(),
        [DROPBOX_METADATA_READ_SCOPE, DROPBOX_CONTENT_READ_SCOPE]
    );

    let app_folder = DropboxSourceConfig::new(
        "dropbox-app-folder",
        "dropbox",
        "Document",
        "",
        DropboxAuthorizationMode::AppFolder,
        DropboxSecretReference::new("keychain/mdreader/dropbox-app").unwrap(),
    )
    .unwrap();
    assert_eq!(app_folder.oauth_scopes(), config().oauth_scopes());
}

#[test]
fn pkce_uses_exact_markdown_write_scopes_without_metadata_write() {
    let pkce = DropboxOAuthPkce::new(
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        "state-123",
        "http://127.0.0.1:41004/oauth/callback",
    )
    .unwrap();

    let authorization = pkce
        .authorization_url_for_profile(
            "desktop-client",
            DropboxAuthorizationProfile::MarkdownReadWrite,
        )
        .unwrap();

    assert!(
        authorization.contains("scope=files.metadata.read+files.content.read+files.content.write")
    );
    assert!(!authorization.contains("files.metadata.write"));
    assert!(!authorization.contains("client_secret"));
}

#[tokio::test]
async fn default_scope_keeps_all_metadata_and_downloads_only_readable_text_content() {
    let first_markdown = file("/knowledge/one.md", "one.md", "r1", "hash-1");
    let binary = file("/knowledge/report.pdf", "report.pdf", "r2", "hash-2");
    let second_text = file("/knowledge/two.txt", "two.txt", "r3", "hash-3");
    let transport = ScriptedTransport::new([
        response(
            200,
            list_page(
                serde_json::json!([first_markdown, binary]),
                "cursor-1",
                true,
            ),
        ),
        response_with_headers(
            200,
            [(
                "Dropbox-API-Result",
                download_header("/knowledge/one.md", "one.md", "r1", "hash-1"),
            )],
            b"# One\n\nMarkdown body\n".to_vec(),
        ),
        response(
            200,
            list_page(serde_json::json!([second_text]), "cursor-2", false),
        ),
        response_with_headers(
            200,
            [(
                "Dropbox-API-Result",
                download_header("/knowledge/two.txt", "two.txt", "r3", "hash-3"),
            )],
            b"plain text body\n".to_vec(),
        ),
    ]);
    let (source, transport) = open_source(transport);

    let answer = read(
        &source,
        &query(),
        &[
            "id".to_string(),
            "name".to_string(),
            "path".to_string(),
            "body".to_string(),
        ],
        &context(2, 10, 64 * 1024),
    )
    .await
    .unwrap();

    assert_eq!(answer.scanned, 3);
    assert_eq!(answer.rows.len(), 3);
    assert_eq!(answer.rows[0].key, "/knowledge/one.md");
    assert_eq!(answer.rows[0].value["body"], "# One\n\nMarkdown body\n");
    assert_eq!(answer.rows[1].key, "/knowledge/report.pdf");
    assert!(answer.rows[1].value.get("body").is_none());
    assert_eq!(answer.rows[2].value["body"], "plain text body\n");

    let requests = transport.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].endpoint(), DropboxEndpoint::Api);
    assert_eq!(requests[0].path(), "/2/files/list_folder");
    assert_eq!(
        requests[0].json_body()["path"],
        serde_json::Value::String("/knowledge".to_string())
    );
    assert_eq!(requests[0].json_body()["recursive"], true);
    assert_eq!(requests[1].endpoint(), DropboxEndpoint::Content);
    assert_eq!(requests[1].path(), "/2/files/download");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            requests[1]
                .header("Dropbox-API-Arg")
                .expect("download argument")
        )
        .unwrap()["path"],
        "id:one.md"
    );
    assert_eq!(requests[2].path(), "/2/files/list_folder/continue");
    assert_eq!(requests[2].json_body()["cursor"], "cursor-1");
    assert_eq!(requests[3].endpoint(), DropboxEndpoint::Content);
    assert!(requests
        .iter()
        .all(|request| request.method() == DropboxHttpMethod::Post));
    assert!(requests
        .iter()
        .all(|request| !request.path().contains("upload")));
}

#[tokio::test]
async fn optional_metadata_on_an_unrelated_file_cannot_poison_markdown_listing() {
    let markdown = file("/knowledge/one.md", "one.md", "r1", "hash-1");
    let mut cloud_native = file(
        "/knowledge/provider-document.paper",
        "provider-document.paper",
        "r2",
        "unused",
    );
    cloud_native
        .as_object_mut()
        .expect("file fixture")
        .remove("content_hash");
    let transport = ScriptedTransport::new([response(
        200,
        list_page(
            serde_json::json!([cloud_native, markdown]),
            "cursor-1",
            false,
        ),
    )]);
    let (source, _) = open_source_with_config(transport, markdown_config());

    let answer = read(
        &source,
        &query(),
        &["id".to_string(), "name".to_string(), "path".to_string()],
        &context(10, 10, 64 * 1024),
    )
    .await
    .expect("optional metadata on an unrelated file");

    assert_eq!(answer.scanned, 2);
    assert_eq!(answer.rows.len(), 1);
    assert_eq!(answer.rows[0].key, "/knowledge/one.md");
}

#[tokio::test]
async fn markdown_scope_excludes_plain_text_files_from_the_semantic_source() {
    let transport = ScriptedTransport::new([response(
        200,
        list_page(
            serde_json::json!([
                file("/knowledge/one.md", "one.md", "r1", "hash-1"),
                file(
                    "/knowledge/guide.markdown",
                    "guide.markdown",
                    "r2",
                    "hash-2"
                ),
                file("/knowledge/notes.txt", "notes.txt", "r3", "hash-3")
            ]),
            "cursor-1",
            false,
        ),
    )]);
    let (source, _) = open_source_with_config(transport, markdown_config());

    let answer = read(
        &source,
        &query(),
        &["id".to_string(), "name".to_string(), "path".to_string()],
        &context(10, 10, 64 * 1024),
    )
    .await
    .expect("Markdown-only Dropbox query");

    assert_eq!(answer.scanned, 3);
    assert_eq!(
        answer
            .rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>(),
        ["/knowledge/one.md", "/knowledge/guide.markdown"]
    );
}

#[tokio::test]
async fn markdown_scope_preserves_continuation_when_the_first_page_has_no_markdown() {
    let transport = ScriptedTransport::new([
        response(
            200,
            list_page(
                serde_json::json!([file("/knowledge/report.pdf", "report.pdf", "r1", "hash-1")]),
                "cursor-1",
                true,
            ),
        ),
        response(
            200,
            list_page(
                serde_json::json!([file(
                    "/knowledge/page-two.markdown",
                    "page-two.markdown",
                    "r2",
                    "hash-2"
                )]),
                "cursor-2",
                false,
            ),
        ),
    ]);
    let (source, transport) = open_source_with_config(transport, markdown_config());
    let query_id = uuid::Uuid::new_v4();
    let read_context = context_with_id(query_id, 1, 10, 64 * 1024, CancellationToken::new());
    let source_plan = plan(&source, &["id", "name", "path"]);

    let first = source
        .execute_page(&source_plan, None, &read_context)
        .await
        .expect("a filtered first provider page");
    assert_eq!(first.scanned, 1);
    assert!(first.rows.is_empty());
    let cursor = first
        .next_cursor
        .expect("the provider continuation survives local Markdown filtering");

    let second = source
        .execute_page(&source_plan, Some(&cursor), &read_context)
        .await
        .expect("the next provider page");
    assert_eq!(second.scanned, 1);
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].key, "/knowledge/page-two.markdown");
    assert!(second.next_cursor.is_none());

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path(), "/2/files/list_folder/continue");
    assert_eq!(requests[1].json_body()["cursor"], "cursor-1");
}

#[tokio::test]
async fn exact_path_reads_only_the_selected_document_body() {
    let transport = ScriptedTransport::new([response_with_headers(
        200,
        [(
            "Dropbox-API-Result",
            download_header("/knowledge/one.md", "one.md", "r1", "hash-1"),
        )],
        b"# One\n\nMarkdown body\n".to_vec(),
    )]);
    let (source, transport) = open_source(transport);
    let plan = source
        .compile(
            &exact_path_query("/knowledge/one.md"),
            &[
                "id".to_string(),
                "name".to_string(),
                "path".to_string(),
                "body".to_string(),
            ],
        )
        .expect("exact path plan");
    let page = source
        .execute_page(&plan, None, &context(10, 10, 64 * 1024))
        .await
        .expect("exact document");

    assert_eq!(page.scanned, 1);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].value["body"], "# One\n\nMarkdown body\n");
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].endpoint(), DropboxEndpoint::Content);
    assert_eq!(requests[0].path(), "/2/files/download");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            requests[0]
                .header("Dropbox-API-Arg")
                .expect("download argument")
        )
        .unwrap()["path"],
        "/knowledge/one.md"
    );
}

#[tokio::test]
async fn download_result_must_match_the_requested_path_id_and_revision() {
    let transport = ScriptedTransport::new([
        response(
            200,
            list_page(
                serde_json::json!([file("/knowledge/one.md", "one.md", "r1", "hash-1")]),
                "cursor-1",
                false,
            ),
        ),
        response_with_headers(
            200,
            [(
                "Dropbox-API-Result",
                download_header("/knowledge/other.md", "other.md", "r9", "hash-9"),
            )],
            b"wrong bytes\n".to_vec(),
        ),
    ]);
    let (source, transport) = open_source(transport);

    let error = source
        .execute_page(&plan(&source, &["body"]), None, &context(10, 10, 4096))
        .await
        .unwrap_err();
    assert!(matches!(error, SourceError::Malformed { .. }));
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn bound_query_cursor_rejects_replay_wrong_query_and_tampering_before_http() {
    let transport = ScriptedTransport::new([
        response(
            200,
            list_page(
                serde_json::json!([file("/knowledge/report.pdf", "report.pdf", "r2", "hash-2")]),
                "cursor-1",
                true,
            ),
        ),
        response(200, list_page(serde_json::json!([]), "cursor-2", false)),
    ]);
    let (source, transport) = open_source(transport);
    let query_id = uuid::Uuid::new_v4();
    let first_context = context_with_id(query_id, 1, 10, 4096, CancellationToken::new());
    let source_plan = plan(&source, &["name"]);
    let first = source
        .execute_page(&source_plan, None, &first_context)
        .await
        .unwrap();
    let cursor = first.next_cursor.expect("provider continuation");
    assert_eq!(transport.requests().len(), 1);

    let wrong_query = context_with_id(uuid::Uuid::new_v4(), 1, 10, 4096, CancellationToken::new());
    assert_eq!(
        source
            .execute_page(&source_plan, Some(&cursor), &wrong_query)
            .await,
        Err(SourceError::InvalidCursor)
    );
    assert_eq!(transport.requests().len(), 1);

    let mut tampered = cursor.as_bytes().to_vec();
    *tampered.last_mut().unwrap() ^= 0x01;
    let tampered = SourceCursor::new(tampered).unwrap();
    assert_eq!(
        source
            .execute_page(&source_plan, Some(&tampered), &first_context)
            .await,
        Err(SourceError::InvalidCursor)
    );
    assert_eq!(transport.requests().len(), 1);

    source
        .execute_page(&source_plan, Some(&cursor), &first_context)
        .await
        .expect("an authenticated continuation remains usable after invalid attempts");
    assert_eq!(transport.requests().len(), 2);

    assert_eq!(
        source
            .execute_page(&source_plan, Some(&cursor), &first_context)
            .await,
        Err(SourceError::InvalidCursor)
    );
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn provider_cursors_larger_than_one_kib_remain_opaque_and_paginatable() {
    let provider_cursor = "c".repeat(1_795);
    let transport = ScriptedTransport::new([
        response(
            200,
            list_page(
                serde_json::json!([file("/knowledge/report.pdf", "report.pdf", "r2", "hash-2",)]),
                &provider_cursor,
                true,
            ),
        ),
        response(200, list_page(serde_json::json!([]), "cursor-2", false)),
    ]);
    let (source, transport) = open_source(transport);
    let read_context = context_with_id(
        uuid::Uuid::new_v4(),
        1,
        10,
        64 * 1024,
        CancellationToken::new(),
    );
    let source_plan = plan(&source, &["name"]);

    let first = source
        .execute_page(&source_plan, None, &read_context)
        .await
        .expect("a provider cursor is opaque, not limited to one KiB");
    let cursor = first.next_cursor.expect("provider continuation");
    source
        .execute_page(&source_plan, Some(&cursor), &read_context)
        .await
        .expect("the authenticated continuation remains within CatDB's bound");

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].json_body()["cursor"],
        serde_json::Value::String(provider_cursor)
    );
}

#[tokio::test]
async fn query_cursor_is_bound_to_selection_schema_source_version_and_authentication() {
    let issuer_transport = ScriptedTransport::new([response(
        200,
        list_page(
            serde_json::json!([file("/knowledge/report.pdf", "report.pdf", "r2", "hash-2")]),
            "cursor-1",
            true,
        ),
    )]);
    let (issuer, _) = open_source(issuer_transport);
    let query_id = uuid::Uuid::new_v4();
    let issuer_context = context_with_id(query_id, 1, 10, 4096, CancellationToken::new());
    let cursor = issuer
        .execute_page(&plan(&issuer, &["name"]), None, &issuer_context)
        .await
        .unwrap()
        .next_cursor
        .unwrap();

    let selected_elsewhere = DropboxSourceConfig::new(
        "dropbox-library",
        "dropbox",
        "Document",
        "/other",
        DropboxAuthorizationMode::FullDropbox,
        DropboxSecretReference::new("keychain/mdreader/dropbox").unwrap(),
    )
    .unwrap();
    let different_source = DropboxSourceConfig::new(
        "dropbox-library-v2",
        "dropbox",
        "Document",
        "/knowledge",
        DropboxAuthorizationMode::FullDropbox,
        DropboxSecretReference::new("keychain/mdreader/dropbox").unwrap(),
    )
    .unwrap();
    let cases = [
        (
            PinnedDropboxSourceSpec::new(selected_elsewhere, "dropbox-schema-v1").unwrap(),
            [0x51; 32],
        ),
        (
            PinnedDropboxSourceSpec::new(config(), "dropbox-schema-v2").unwrap(),
            [0x51; 32],
        ),
        (
            PinnedDropboxSourceSpec::new(different_source, "dropbox-schema-v1").unwrap(),
            [0x51; 32],
        ),
        (
            PinnedDropboxSourceSpec::new(config(), "dropbox-schema-v1").unwrap(),
            [0x52; 32],
        ),
    ];

    for (spec, cursor_key) in cases {
        let transport = ScriptedTransport::new([]);
        let foreign = DropboxSource::open_with_transport(
            spec,
            DropboxRuntimeSecrets::new(
                DropboxAccessToken::new("fixture-access-token").unwrap(),
                DropboxCursorKey::new(cursor_key),
            ),
            Arc::new(transport.clone()),
        )
        .unwrap();
        assert_eq!(
            foreign
                .execute_page(&plan(&foreign, &["name"]), Some(&cursor), &issuer_context,)
                .await,
            Err(SourceError::InvalidCursor)
        );
        assert!(
            transport.requests().is_empty(),
            "binding mismatch crossed the provider boundary"
        );
    }
}

#[tokio::test]
async fn incremental_changes_distinguish_unchanged_upsert_delete_and_reset() {
    let transport = ScriptedTransport::new([
        response(
            200,
            serde_json::to_vec(&serde_json::json!({"cursor":"checkpoint-0"})).unwrap(),
        ),
        response(
            200,
            list_page(
                serde_json::json!([
                    file("/knowledge/same.md", "same.md", "r1", "hash-1"),
                    file("/knowledge/new.md", "new.md", "r2", "hash-2"),
                    {".tag":"deleted","name":"gone.md","path_lower":"/knowledge/gone.md","path_display":"/knowledge/gone.md"}
                ]),
                "checkpoint-1",
                false,
            ),
        ),
        response(
            409,
            serde_json::to_vec(&serde_json::json!({
                "error_summary":"reset/...",
                "error":{".tag":"reset"}
            }))
            .unwrap(),
        ),
    ]);
    let (source, transport) = open_source(transport);
    let read_context = context(10, 20, 64 * 1024);
    let start = source.start_change_tracking(&read_context).await.unwrap();
    let page = source.read_changes(&start, &read_context).await.unwrap();
    let checkpoint = match page {
        DropboxChangeOutcome::Page {
            events,
            next_cursor,
            checkpoint,
        } => {
            assert_eq!(
                events,
                vec![
                    DropboxChangeEvent::Upserted {
                        path_lower: "/knowledge/same.md".to_string()
                    },
                    DropboxChangeEvent::Upserted {
                        path_lower: "/knowledge/new.md".to_string(),
                    },
                    DropboxChangeEvent::Deleted {
                        path_lower: "/knowledge/gone.md".to_string()
                    },
                ]
            );
            assert!(next_cursor.is_none());
            checkpoint.expect("durable checkpoint only after exhaustion")
        }
        other => panic!("unexpected change outcome: {other:?}"),
    };

    assert_eq!(
        source
            .read_changes(&checkpoint, &read_context)
            .await
            .unwrap(),
        DropboxChangeOutcome::RebuildRequired
    );
    assert_eq!(transport.requests().len(), 3);
    assert_eq!(
        transport.requests()[0].path(),
        "/2/files/list_folder/get_latest_cursor"
    );
    assert_eq!(transport.requests()[0].json_body()["include_deleted"], true);
    assert_eq!(
        transport.requests()[1].path(),
        "/2/files/list_folder/continue"
    );
    assert_eq!(
        transport.requests()[1].json_body()["cursor"],
        "checkpoint-0"
    );
    assert_eq!(
        transport.requests()[2].path(),
        "/2/files/list_folder/continue"
    );
}

#[derive(Clone)]
struct RefreshingResolver {
    refresh_count: Arc<AtomicUsize>,
}

#[async_trait]
impl DropboxSecretResolver for RefreshingResolver {
    async fn resolve(
        &self,
        _reference: &DropboxSecretReference,
    ) -> Result<DropboxRuntimeSecrets, DropboxConnectorError> {
        unreachable!("fixture opens from explicit initial secrets")
    }

    async fn refresh(
        &self,
        _reference: &DropboxSecretReference,
        _refresh: &DropboxRefreshCredential,
    ) -> Result<DropboxAccessGrant, DropboxConnectorError> {
        self.refresh_count.fetch_add(1, Ordering::SeqCst);
        DropboxAccessGrant::new(
            DropboxAccessToken::new("refreshed-token").unwrap(),
            Some(SystemTime::now() + Duration::from_secs(600)),
        )
    }
}

#[derive(Clone)]
struct ConcurrentUnauthorizedTransport {
    barrier: Arc<tokio::sync::Barrier>,
    requests: Arc<Mutex<Vec<DropboxHttpRequest>>>,
}

#[async_trait]
impl DropboxTransport for ConcurrentUnauthorizedTransport {
    async fn send(
        &self,
        request: DropboxHttpRequest,
    ) -> Result<DropboxHttpResponse, DropboxTransportError> {
        let generation = request.credential_generation();
        self.requests.lock().unwrap().push(request);
        if generation == 0 {
            self.barrier.wait().await;
            return DropboxHttpResponse::new(401, BTreeMap::new(), b"{}".to_vec());
        }
        DropboxHttpResponse::new(
            200,
            BTreeMap::new(),
            list_page(serde_json::json!([]), "done", false),
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_401s_share_one_refresh_and_each_read_retries_once() {
    let resolver = Arc::new(RefreshingResolver {
        refresh_count: Arc::new(AtomicUsize::new(0)),
    });
    let requests = Arc::new(Mutex::new(Vec::new()));
    let transport = Arc::new(ConcurrentUnauthorizedTransport {
        barrier: Arc::new(tokio::sync::Barrier::new(2)),
        requests: requests.clone(),
    });
    let source = Arc::new(
        DropboxSource::open_with_resolver_and_transport(
            PinnedDropboxSourceSpec::new(config(), "dropbox-schema-v1").unwrap(),
            DropboxRuntimeSecrets::refreshable(
                DropboxAccessGrant::new(
                    DropboxAccessToken::new("expired-token").unwrap(),
                    Some(SystemTime::now() + Duration::from_secs(600)),
                )
                .unwrap(),
                DropboxRefreshCredential::new("refresh-secret").unwrap(),
                DropboxCursorKey::new([0x51; 32]),
            ),
            resolver.clone(),
            transport,
        )
        .unwrap(),
    );
    let source_plan = plan(&source, &["name"]);
    let left = {
        let source = source.clone();
        let source_plan = source_plan.clone();
        tokio::spawn(async move {
            source
                .execute_page(&source_plan, None, &context(10, 10, 4096))
                .await
        })
    };
    let right = {
        let source = source.clone();
        let source_plan = source_plan.clone();
        tokio::spawn(async move {
            source
                .execute_page(&source_plan, None, &context(10, 10, 4096))
                .await
        })
    };
    left.await.unwrap().unwrap();
    right.await.unwrap().unwrap();
    assert_eq!(resolver.refresh_count.load(Ordering::SeqCst), 1);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
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
        2
    );
}

#[tokio::test]
async fn a_retried_401_remains_authentication_failure_without_a_retry_loop() {
    let resolver = Arc::new(RefreshingResolver {
        refresh_count: Arc::new(AtomicUsize::new(0)),
    });
    let transport =
        ScriptedTransport::new([response(401, b"{}".to_vec()), response(401, b"{}".to_vec())]);
    let source = DropboxSource::open_with_resolver_and_transport(
        PinnedDropboxSourceSpec::new(config(), "dropbox-schema-v1").unwrap(),
        DropboxRuntimeSecrets::refreshable(
            DropboxAccessGrant::new(
                DropboxAccessToken::new("current-token").unwrap(),
                Some(SystemTime::now() + Duration::from_secs(600)),
            )
            .unwrap(),
            DropboxRefreshCredential::new("refresh-secret").unwrap(),
            DropboxCursorKey::new([0x51; 32]),
        ),
        resolver.clone(),
        Arc::new(transport.clone()),
    )
    .unwrap();

    assert_eq!(
        source
            .execute_page(&plan(&source, &["name"]), None, &context(10, 10, 4096))
            .await,
        Err(SourceError::Authentication {
            source_id: source.id().clone(),
        })
    );
    assert_eq!(resolver.refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn rate_limit_timeout_and_cancellation_remain_typed() {
    let rate_limited = ScriptedTransport::new([response_with_headers(
        429,
        [("retry-after", "3".to_string())],
        b"{}".to_vec(),
    )]);
    let (source, _) = open_source(rate_limited);
    assert_eq!(
        source
            .execute_page(&plan(&source, &["name"]), None, &context(10, 10, 4096))
            .await,
        Err(SourceError::RateLimited {
            source_id: source.id().clone(),
            retry_after_ms: Some(3_000),
        })
    );

    let pending = ScriptedTransport::new([ScriptStep::Pending]);
    let (source, _) = open_source(pending);
    let deadline_context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_millis(20),
        10,
        4096,
        10,
        CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        source
            .execute_page(&plan(&source, &["name"]), None, &deadline_context)
            .await,
        Err(SourceError::DeadlineExceeded)
    );

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let cancelled = context_with_id(uuid::Uuid::new_v4(), 10, 10, 4096, cancellation);
    assert_eq!(
        source
            .execute_page(&plan(&source, &["name"]), None, &cancelled)
            .await,
        Err(SourceError::Cancelled)
    );
}

#[test]
fn source_spec_and_debug_are_secret_free_and_document_mutation_is_constrained() {
    let transport = ScriptedTransport::new([]);
    let (source, _) = open_source(transport);
    let spec = source.source_spec().to_string();
    assert!(spec.contains("keychain/mdreader/dropbox"));
    assert!(!spec.contains("fixture-access-token"));
    assert!(!format!("{source:?}").contains("fixture-access-token"));
    assert_eq!(
        source.availability_evidence(),
        DropboxAvailability::FixtureValidated
    );

    let source_text = include_str!("../src/legacy.rs").to_ascii_lowercase();
    assert!(source_text.contains("/2/files/upload"));
    for forbidden in [
        "/2/files/delete",
        "/2/files/move",
        "/2/files/copy",
        "/2/sharing/",
        "arbitrary_rpc",
    ] {
        assert!(
            !source_text.contains(forbidden),
            "mutation surface: {forbidden}"
        );
    }
}

#[test]
fn deserialized_pinned_spec_revalidates_schema_identity() {
    let mut value = serde_json::to_value(
        PinnedDropboxSourceSpec::new(config(), "dropbox-schema-v1").expect("valid pin"),
    )
    .expect("serialize pin");
    value["schema_version"] = serde_json::Value::String(String::new());

    assert!(serde_json::from_value::<PinnedDropboxSourceSpec>(value).is_err());
}

#[test]
fn request_construction_refuses_non_dropbox_origins_redirects_and_header_injection() {
    let transport = mdreader_connector_dropbox::ReqwestDropboxTransport::new().unwrap();
    assert_eq!(
        transport.api_origin().as_str(),
        "https://api.dropboxapi.com/"
    );
    assert_eq!(
        transport.content_origin().as_str(),
        "https://content.dropboxapi.com/"
    );
    assert!(!transport.follows_redirects());

    assert!(DropboxSourceConfig::new(
        "dropbox",
        "dropbox",
        "Document",
        "/knowledge\nAuthorization: bearer stolen",
        DropboxAuthorizationMode::FullDropbox,
        DropboxSecretReference::new("keychain/dropbox").unwrap(),
    )
    .is_err());
}
