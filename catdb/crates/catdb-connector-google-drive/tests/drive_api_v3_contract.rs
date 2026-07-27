use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use catdb_connector_google_drive::{
    DriveAccessGrant, DriveAccessToken, DriveAvailability, DriveChangeCursor, DriveChangeEvent,
    DriveChangeOutcome, DriveConnectorError, DriveCursorKey, DriveHttpMethod, DriveHttpRequest,
    DriveHttpResponse, DriveKnownRevision, DriveOAuthPkce, DriveRefreshCredential,
    DriveRuntimeSecrets, DriveSecretReference, DriveSecretResolver, DriveSelection, DriveSource,
    DriveSourceConfig, DriveTransport, DriveTransportError, PinnedDriveSourceSpec,
    GOOGLE_DRIVE_FILE_SCOPE,
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
fn picker_pkce_requests_one_file_consent_and_the_exact_scope_without_a_client_secret() {
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
    assert!(authorization.contains("prompt=consent"));
    assert!(authorization.contains("trigger_onepick=true"));
    assert!(!authorization.contains("client_secret"));
    assert_eq!(config().oauth_scope(), GOOGLE_DRIVE_FILE_SCOPE);
    assert_eq!(
        GOOGLE_DRIVE_FILE_SCOPE,
        "https://www.googleapis.com/auth/drive.file"
    );
}

#[derive(Clone)]
struct RefreshingResolver {
    resolve_count: Arc<AtomicUsize>,
    refresh_count: Arc<AtomicUsize>,
    refresh_mode: RefreshMode,
}

#[derive(Clone, Copy)]
enum RefreshMode {
    Succeeds,
    Revoked,
    Pending,
}

impl RefreshingResolver {
    fn new(refresh_mode: RefreshMode) -> Self {
        Self {
            resolve_count: Arc::new(AtomicUsize::new(0)),
            refresh_count: Arc::new(AtomicUsize::new(0)),
            refresh_mode,
        }
    }
}

#[async_trait]
impl DriveSecretResolver for RefreshingResolver {
    async fn resolve(
        &self,
        _reference: &DriveSecretReference,
    ) -> Result<DriveRuntimeSecrets, DriveConnectorError> {
        self.resolve_count.fetch_add(1, Ordering::SeqCst);
        Ok(DriveRuntimeSecrets::refreshable(
            DriveAccessGrant::new(
                DriveAccessToken::new("initial-access-token")?,
                Some(SystemTime::now() + Duration::from_secs(3_600)),
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
            RefreshMode::Pending => std::future::pending().await,
        }
    }
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
    assert_eq!(transport.requests().len(), 3);
    assert_eq!(transport.requests()[0].credential_generation(), 0);
    assert_eq!(transport.requests()[1].credential_generation(), 1);
    assert_eq!(transport.requests()[2].credential_generation(), 1);
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
        "/drive/v3/files/selected-file?fields=id%2Cname%2CmimeType%2CmodifiedTime%2Csize%2Cmd5Checksum%2Cparents%2Ctrashed&supportsAllDrives=true"
    );
    assert!(!transport.requests()[0].path().contains("q=id"));
    assert!(!format!("{:?}", transport.requests()[0]).contains("selected-file"));
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
    assert!(requests[0]
        .path()
        .contains("%27selected-folder%27+in+parents"));
    assert!(requests[1]
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
    let known = BTreeMap::from([(
        "moved-out".to_string(),
        DriveKnownRevision::new("2026-07-20T00:00:00Z", Some("old")).unwrap(),
    )]);
    let DriveChangeOutcome::Page { events, .. } = source
        .read_changes(&cursor, &known, &context(10, 10, 64 * 1024))
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
                modified_time: "2026-07-21T00:00:01Z".to_string(),
                checksum: None,
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
    let nested_header = nested_transport.requests()[1]
        .header("x-goog-drive-resource-keys")
        .unwrap();
    assert!(nested_header.contains("selected-folder/root-key"));
    assert!(nested_header.contains("nested-folder/nested-key"));
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
    assert_eq!(transport.requests().len(), 2);
    assert!(transport.requests()[1]
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
    assert_eq!(transport.requests().len(), 1);
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
                &BTreeMap::new(),
                &context_with_id(query_id, 1, 4, 64 * 1024, CancellationToken::new()),
            )
            .await,
        Err(SourceError::InvalidCursor)
    );
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
            different
                .read_changes(&cursor, &BTreeMap::new(), &context(2, 2, 4_096))
                .await,
            Err(SourceError::InvalidCursor)
        );
    }

    let mut tampered = cursor.as_bytes().to_vec();
    let index = tampered.len() / 2;
    tampered[index] ^= 0x01;
    let tampered = DriveChangeCursor::from_bytes(tampered).unwrap();
    assert_eq!(
        tracker
            .read_changes(&tampered, &BTreeMap::new(), &context(2, 2, 4_096))
            .await,
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
            .read_changes(&cross_kind, &BTreeMap::new(), &context(2, 2, 4_096))
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
        source
            .read_changes(&cursor, &BTreeMap::new(), &context(2, 2, 4_096))
            .await,
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
            .read_changes(&cursor, &BTreeMap::new(), &context(2, 2, 4096))
            .await
            .unwrap(),
        DriveChangeOutcome::RebuildRequired
    );
}
