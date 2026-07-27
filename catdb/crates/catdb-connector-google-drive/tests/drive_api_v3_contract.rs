use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use catdb_connector_google_drive::{
    DriveAccessToken, DriveAvailability, DriveChangeEvent, DriveChangeOutcome, DriveCursorKey,
    DriveHttpMethod, DriveHttpRequest, DriveHttpResponse, DriveKnownRevision, DriveOAuthPkce,
    DriveRuntimeSecrets, DriveSecretReference, DriveSelection, DriveSource, DriveSourceConfig,
    DriveTransport, DriveTransportError, PinnedDriveSourceSpec, GOOGLE_DRIVE_FILE_SCOPE,
};
use catdb_connectors::{
    read, CancellationToken, DataSource, ReadContext, SourceCursor, SourceError,
};
use catdb_query::{EntityQuery, EntityTarget};

#[derive(Clone)]
struct ScriptedTransport {
    state: Arc<Mutex<ScriptState>>,
}

struct ScriptState {
    steps: VecDeque<ScriptStep>,
    requests: Vec<DriveHttpRequest>,
}

enum ScriptStep {
    Response(DriveHttpResponse),
    Pending,
}

impl ScriptedTransport {
    fn new(steps: impl IntoIterator<Item = ScriptStep>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptState {
                steps: steps.into_iter().collect(),
                requests: Vec::new(),
            })),
        }
    }

    fn requests(&self) -> Vec<DriveHttpRequest> {
        self.state.lock().unwrap().requests.clone()
    }
}

#[async_trait]
impl DriveTransport for ScriptedTransport {
    async fn send(
        &self,
        request: DriveHttpRequest,
    ) -> Result<DriveHttpResponse, DriveTransportError> {
        let step = {
            let mut state = self.state.lock().unwrap();
            state.requests.push(request);
            state.steps.pop_front().expect("fixture script exhausted")
        };
        match step {
            ScriptStep::Response(response) => Ok(response),
            ScriptStep::Pending => std::future::pending().await,
        }
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
        vec![
            DriveSelection::file("selected-file"),
            DriveSelection::folder("selected-folder"),
        ],
        DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
    )
    .unwrap()
}

fn shared_drive_config() -> DriveSourceConfig {
    config().with_shared_drive("shared-drive-7").unwrap()
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

fn plan(source: &DriveSource, projected: &[&str]) -> catdb_connectors::SourcePlan {
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

#[test]
fn pkce_uses_s256_state_and_the_exact_selected_file_scope_without_a_client_secret() {
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
    assert!(authorization.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file"));
    assert!(!authorization.contains("client_secret"));
    assert_eq!(config().oauth_scope(), GOOGLE_DRIVE_FILE_SCOPE);
    assert_eq!(
        GOOGLE_DRIVE_FILE_SCOPE,
        "https://www.googleapis.com/auth/drive.file"
    );
}

#[test]
fn source_specs_and_debug_output_never_contain_runtime_credentials() {
    let transport = ScriptedTransport::new([]);
    let (source, _) = source(transport, config());
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
    assert_eq!(requests.len(), 2);
    assert!(requests[1].path().contains("pageToken=page-token-2"));
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
    let page = source
        .execute_page(&plan(&source, &["id"]), None, &context(1, 4, 64 * 1024))
        .await
        .unwrap();
    assert!(matches!(
        source
            .execute_page(
                &plan(&source, &["id"]),
                page.next_cursor.as_ref(),
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
    let transport = ScriptedTransport::new([response(200, body)]);
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
    assert!(same_source
        .execute_page(
            &plan(&same_source, &["id"]),
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
    assert!(requests.iter().any(|request| {
        request.path() == "/drive/v3/files/gdoc/export?mimeType=text%2Fplain&supportsAllDrives=true"
    }));
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
    let path = transport.requests()[0].path().to_string();
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
    let known = BTreeMap::from([
        (
            "gone".to_string(),
            DriveKnownRevision::new("2026-07-19T00:00:00Z", Some("gone-checksum")).unwrap(),
        ),
        (
            "same".to_string(),
            DriveKnownRevision::new("2026-07-20T12:34:56Z", Some("same-checksum")).unwrap(),
        ),
    ]);
    let first = source
        .read_changes(&initial, &known, &context(20, 20, 64 * 1024))
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
            DriveChangeEvent::Unchanged {
                file_id: "same".to_string()
            }
        ]
    );
    assert!(checkpoint.is_none());
    let second_cursor = next_cursor.expect("provider returned another page");
    let second = source
        .read_changes(&second_cursor, &known, &context(20, 20, 64 * 1024))
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
            .read_changes(&cursor, &BTreeMap::new(), &context(2, 2, 4096))
            .await
            .unwrap(),
        DriveChangeOutcome::RebuildRequired
    );
}
