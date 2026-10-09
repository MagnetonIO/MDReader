#[test]
fn native_kernel_never_adopts_legacy_broad_oauth_as_selected_authority() {
    assert_eq!(
        mdreader_connector_google_drive::native::SELECTED_FILE_SCOPE,
        "https://www.googleapis.com/auth/drive.file"
    );
}
use async_trait::async_trait;
use mdreader_connector_google_drive::native::*;
use mdreader_provider::{
    transport::{Method, Request, Response, Transport},
    *,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime},
};
struct Resolver(AtomicUsize);
#[async_trait]
impl CredentialResolver for Resolver {
    async fn access(&self, _: &SourceBinding, _: &IoContext) -> Result<AccessGrant> {
        self.0.fetch_add(1, Ordering::SeqCst);
        AccessGrant::new(
            "synthetic-first".into(),
            SystemTime::now() + Duration::from_secs(60),
            1,
        )
    }
    async fn refresh(
        &self,
        _: &SourceBinding,
        previous: u64,
        _: &IoContext,
    ) -> Result<AccessGrant> {
        self.0.fetch_add(1, Ordering::SeqCst);
        AccessGrant::new(
            "synthetic-refreshed".into(),
            SystemTime::now() + Duration::from_secs(60),
            previous + 1,
        )
    }
}
struct Authority(AtomicBool);
#[async_trait]
impl ProviderAuthority for Authority {
    async fn revalidate(&self, _: &SourceBinding) -> Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ProviderError::Unauthorized)
        }
    }
}
enum Step {
    Reply(Response),
    AcceptedThenLost,
}
struct Script {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<Request>>,
    mutations: AtomicUsize,
}
#[async_trait]
impl Transport for Script {
    async fn send(&self, request: Request) -> Result<Response> {
        if request.method() == Method::Patch {
            self.mutations.fetch_add(1, Ordering::SeqCst);
        }
        self.requests.lock().unwrap().push(request);
        match self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request")
        {
            Step::Reply(response) => Ok(response),
            Step::AcceptedThenLost => Err(ProviderError::Unavailable),
        }
    }
}
fn metadata(revision: &str, etag: Option<&str>, body: &[u8]) -> Response {
    let hash = if body == b"old" {
        "149603e6c03516362a8da23f624db945"
    } else if body == b"new" {
        "22af645d1859cb5ca6da0c484f1f37ea"
    } else {
        panic!("fixture body")
    };
    let json = serde_json::json!({"id":"selected","name":"plan.md","mimeType":"text/markdown","version":revision,"md5Checksum":hash,"size":body.len().to_string(),"trashed":false});
    let mut headers = BTreeMap::new();
    if let Some(etag) = etag {
        headers.insert("etag".into(), etag.into());
    }
    Response::new(200, headers, serde_json::to_vec(&json).unwrap()).unwrap()
}
fn fixture(steps: Vec<Step>) -> (Client, Arc<Script>, Arc<Resolver>, Arc<Authority>) {
    let key = RecordKey::new("selected").unwrap();
    let scope = SelectedScope::exact(vec![key.clone()]).unwrap();
    let binding = SourceBinding::new(
        LibraryId::new("library").unwrap(),
        SourceId::new("source").unwrap(),
        AccountBinding::new("account").unwrap(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap();
    let descriptor = SourceDescriptor::new(
        binding,
        ProviderKind::GoogleDrive,
        DisplayText::new("Selected plans").unwrap(),
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
    )
    .unwrap();
    let script = Arc::new(Script {
        steps: Mutex::new(steps.into()),
        requests: Mutex::new(vec![]),
        mutations: AtomicUsize::new(0),
    });
    let resolver = Arc::new(Resolver(AtomicUsize::new(0)));
    let authority = Arc::new(Authority(AtomicBool::new(true)));
    let client = Client::new(
        descriptor,
        vec![SelectedFile::new(key, Some(OpaqueValue::new("resource-key").unwrap())).unwrap()],
        resolver.clone(),
        authority.clone(),
        script.clone(),
        [7; 32],
    )
    .unwrap();
    (client, script, resolver, authority)
}
fn context() -> IoContext {
    IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        16,
        8 * 1024 * 1024,
        Cancellation::default(),
    )
    .unwrap()
}
#[tokio::test]
async fn exact_selected_read_returns_three_distinct_authorities_and_revalidates_body_version() {
    let (client, script, _, _) = fixture(vec![
        Step::Reply(metadata("1", Some("\"http-1\""), b"old")),
        Step::Reply(Response::new(200, BTreeMap::new(), b"old".to_vec()).unwrap()),
        Step::Reply(metadata("1", Some("\"http-1\""), b"old")),
    ]);
    let read = client
        .read(&RecordKey::new("selected").unwrap(), &context())
        .await
        .unwrap();
    assert_eq!(read.body.as_bytes(), b"old");
    assert_eq!(read.version.provider_revision.as_str(), "1");
    assert_eq!(
        read.version.conditional_write_token.unwrap().as_str(),
        "\"http-1\""
    );
    assert_eq!(
        read.version.content_hash.unwrap().algorithm(),
        HashAlgorithm::Md5
    );
    {
        let requests = script.requests.lock().unwrap();
        assert_eq!(requests[1].headers()["if-match"], "\"http-1\"");
        assert_eq!(
            requests[1].headers()["x-goog-drive-resource-keys"],
            "selected/resource-key"
        );
        assert!(requests[0].url().contains("version"));
        assert!(!format!("{:?}", requests[0]).contains("synthetic-first"));
    }
    assert!(client
        .read(&RecordKey::new("unselected").unwrap(), &context())
        .await
        .is_err());
    assert_eq!(script.requests.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn send_once_guarded_write_uses_only_http_validator_and_unknown_is_durable() {
    let (client, script, _, _) = fixture(vec![
        Step::Reply(metadata("1", Some("\"http-1\""), b"old")),
        Step::AcceptedThenLost,
    ]);
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(b"new".to_vec()).unwrap();
    let expected = DocumentVersion {
        provider_revision: OpaqueValue::new("1").unwrap(),
        content_hash: Some(
            ContentHash::from_hex(HashAlgorithm::Md5, "149603e6c03516362a8da23f624db945").unwrap(),
        ),
        conditional_write_token: Some(OpaqueValue::new("\"http-1\"").unwrap()),
    };
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let binding = client.descriptor().binding();
    let id = journal::prepare(&mut db, binding, &key, &expected, &body).unwrap();
    let permit = journal::begin_dispatch(&mut db, binding, id).unwrap();
    assert!(
        matches!(client.guarded_write(permit,&body,&context()).await,Err(ProviderError::UnknownWrite{operation_id})if operation_id==id)
    );
    journal::mark_unknown(&mut db, binding, id).unwrap();
    assert!(journal::begin_dispatch(&mut db, binding, id).is_err());
    assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
    let requests = script.requests.lock().unwrap();
    assert_eq!(requests[1].headers()["if-match"], "\"http-1\"");
    assert_eq!(requests[1].body(), b"new");
    assert!(!requests[1].headers()["if-match"].contains("149603"));
}
#[tokio::test]
async fn read_auth_refresh_is_bounded_and_revoked_authority_prevents_network() {
    let (client, script, resolver, authority) = fixture(vec![
        Step::Reply(Response::new(401, BTreeMap::new(), vec![]).unwrap()),
        Step::Reply(metadata("1", None, b"old")),
    ]);
    let page = client.list(None, &context()).await.unwrap();
    assert_eq!(page.rows().len(), 1);
    assert_eq!(resolver.0.load(Ordering::SeqCst), 2);
    assert_eq!(script.requests.lock().unwrap().len(), 2);
    authority.0.store(false, Ordering::SeqCst);
    assert_eq!(
        client.list(None, &context()).await.unwrap_err(),
        ProviderError::Unauthorized
    );
    assert_eq!(script.requests.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn missing_http_validator_refuses_write_and_changed_read_does_not_cache_wrong_revision() {
    let (client, script, _, _) = fixture(vec![Step::Reply(metadata("1", None, b"old"))]);
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(b"new".to_vec()).unwrap();
    let expected = DocumentVersion {
        provider_revision: OpaqueValue::new("1").unwrap(),
        content_hash: Some(
            ContentHash::from_hex(HashAlgorithm::Md5, "149603e6c03516362a8da23f624db945").unwrap(),
        ),
        conditional_write_token: None,
    };
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let id = journal::prepare(
        &mut db,
        client.descriptor().binding(),
        &key,
        &expected,
        &body,
    )
    .unwrap();
    let permit = journal::begin_dispatch(&mut db, client.descriptor().binding(), id).unwrap();
    assert_eq!(
        client
            .guarded_write(permit, &body, &context())
            .await
            .unwrap_err(),
        ProviderError::Unsupported
    );
    assert_eq!(script.mutations.load(Ordering::SeqCst), 0);
    let (client, _, _, _) = fixture(vec![
        Step::Reply(metadata("1", None, b"old")),
        Step::Reply(Response::new(200, BTreeMap::new(), b"old".to_vec()).unwrap()),
        Step::Reply(metadata("2", None, b"new")),
    ]);
    assert_eq!(
        client.read(&key, &context()).await.unwrap_err(),
        ProviderError::Conflict
    );
}
struct HungAuthority;
#[async_trait]
impl ProviderAuthority for HungAuthority {
    async fn revalidate(&self, _: &SourceBinding) -> Result<()> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn hung_admission_callback_obeys_operation_deadline_before_network() {
    let (client, script, resolver, _) = fixture(vec![]);
    let descriptor = client.descriptor().clone();
    let client = Client::new(
        descriptor,
        vec![SelectedFile::new(RecordKey::new("selected").unwrap(), None).unwrap()],
        resolver,
        Arc::new(HungAuthority),
        script.clone(),
        [8; 32],
    )
    .unwrap();
    let context = IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_millis(20),
        1,
        1024,
        Cancellation::default(),
    )
    .unwrap();
    let outcome =
        tokio::time::timeout(Duration::from_millis(100), client.list(None, &context)).await;
    assert!(matches!(outcome, Ok(Err(ProviderError::Timeout))));
    assert_eq!(script.requests.lock().unwrap().len(), 0);
}
#[tokio::test]
async fn selected_file_leaving_markdown_scope_is_a_tombstone_not_an_upsert() {
    let start = Response::new(
        200,
        BTreeMap::new(),
        br#"{"startPageToken":"start"}"#.to_vec(),
    )
    .unwrap();
    let page=Response::new(200,BTreeMap::new(),br#"{"changes":[{"fileId":"selected","removed":false,"file":{"id":"selected","name":"plan.txt","mimeType":"text/plain","trashed":false}},{"fileId":"unselected","removed":true}],"newStartPageToken":"checkpoint"}"#.to_vec()).unwrap();
    let (client, _, _, _) = fixture(vec![Step::Reply(start), Step::Reply(page)]);
    let cursor = client.start_changes(&context()).await.unwrap();
    let outcome = client.changes(&cursor, &context()).await.unwrap();
    assert!(
        matches!(outcome,ChangeOutcome::Page{events,next_cursor:None,checkpoint:Some(_)}if events.len()==1&&matches!(&events[0],Change::Deleted{key,expected_path:None}if key.as_str()=="selected"))
    );
}

#[tokio::test]
async fn readonly_folder_pages_filter_suffix_bind_cursor_and_recheck_membership_after_body() {
    fn reply(value: serde_json::Value) -> Step {
        Step::Reply(
            Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap()).unwrap(),
        )
    }
    fn file(name: &str, parent: &str) -> serde_json::Value {
        serde_json::json!({"id":"selected","name":name,"mimeType":"text/markdown","version":"1","size":"3","parents":[parent],"md5Checksum":"149603e6c03516362a8da23f624db945"})
    }
    let (_, script, resolver, authority) = fixture(vec![
        reply(serde_json::json!({"files":[file("ignore.txt","notes")],"nextPageToken":"next"})),
        reply(serde_json::json!({"files":[file("Plan.MD","notes")]})),
        reply(
            serde_json::json!({"files":[{"id":"child","name":"Nested","mimeType":"application/vnd.google-apps.folder","parents":["notes"]}],"nextPageToken":"folders-next"}),
        ),
        reply(file("Plan.MD", "notes")),
        Step::Reply(Response::new(200, BTreeMap::new(), b"old".to_vec()).unwrap()),
        reply(file("Plan.MD", "outside")),
    ]);
    let scope = SelectedScope::ScopedRoot {
        root: RecordKey::new("notes").unwrap(),
        recursive: false,
    };
    let binding = SourceBinding::new(
        LibraryId::new("library").unwrap(),
        SourceId::new("source").unwrap(),
        AccountBinding::new("account").unwrap(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap();
    let descriptor = SourceDescriptor::new(
        binding,
        ProviderKind::GoogleDrive,
        DisplayText::new("Read-only notes").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: false,
            guarded_write: false,
            assets: false,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    let client = Client::new_readonly_folder(
        descriptor,
        RecordKey::new("notes").unwrap(),
        resolver,
        authority,
        script.clone(),
        [7; 32],
    )
    .unwrap();
    let first = client.list(None, &context()).await.unwrap();
    assert!(first.rows().is_empty());
    let second = client.list(first.next_cursor(), &context()).await.unwrap();
    assert_eq!(second.rows()[0].name.as_str(), "Plan.MD");
    assert_eq!(second.rows()[0].display_path.as_str(), "/notes/selected");
    assert!(second.next_cursor().is_none());
    let folders = client
        .folders(&RecordKey::new("notes").unwrap(), None, &context())
        .await
        .unwrap();
    assert_eq!(folders.folders[0].id.as_str(), "child");
    let count = script.requests.lock().unwrap().len();
    assert!(client
        .list(folders.next.as_ref(), &context())
        .await
        .is_err());
    assert!(client
        .folders(&RecordKey::new("outside").unwrap(), None, &context())
        .await
        .is_err());
    assert_eq!(script.requests.lock().unwrap().len(), count);
    assert!(matches!(
        client
            .read(&RecordKey::new("selected").unwrap(), &context())
            .await,
        Err(ProviderError::Unauthorized)
    ));
    let requests = script.requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    let url = url::Url::parse(requests[0].url()).unwrap();
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert!(query["q"].contains("'notes' in parents"));
    assert!(!query["q"].contains("contains"));
    assert!(requests.iter().all(|r| r.method() == Method::Get));
}
