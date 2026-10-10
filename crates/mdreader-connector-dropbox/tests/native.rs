use async_trait::async_trait;
use mdreader_connector_dropbox::native::*;
use mdreader_provider::{
    transport::{Method, Request, Response, Transport},
    *,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
struct Resolver;
#[async_trait]
impl CredentialResolver for Resolver {
    async fn access(&self, _: &SourceBinding, _: &IoContext) -> Result<AccessGrant> {
        AccessGrant::new(
            "synthetic-token".into(),
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
        AccessGrant::new(
            "synthetic-refreshed".into(),
            SystemTime::now() + Duration::from_secs(60),
            previous + 1,
        )
    }
}
struct Authority(AtomicBool, Mutex<rusqlite::Connection>);
#[async_trait]
impl ProviderAuthority for Authority {
    async fn revalidate(&self, _: &SourceBinding) -> Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ProviderError::Unauthorized)
        }
    }
    async fn store_page(
        &self,
        binding: &SourceBinding,
        kind: CursorKind,
        slot: uuid::Uuid,
        digest: [u8; 32],
        bytes: &[u8],
    ) -> Result<()> {
        self.revalidate(binding).await?;
        self.1.lock().unwrap().execute("INSERT INTO pages VALUES(?1,?2,?3,?4,?5) ON CONFLICT(binding,kind) DO UPDATE SET slot=excluded.slot,digest=excluded.digest,bytes=excluded.bytes", rusqlite::params![binding.digest().as_slice(),format!("{kind:?}"),slot.to_string(),digest.as_slice(),bytes]).unwrap();
        Ok(())
    }
    async fn load_page(
        &self,
        binding: &SourceBinding,
        kind: CursorKind,
        slot: uuid::Uuid,
        digest: [u8; 32],
    ) -> Result<Vec<u8>> {
        use rusqlite::OptionalExtension;
        self.revalidate(binding).await?;
        self.1
            .lock()
            .unwrap()
            .query_row(
                "SELECT bytes FROM pages WHERE binding=?1 AND kind=?2 AND slot=?3 AND digest=?4",
                rusqlite::params![
                    binding.digest().as_slice(),
                    format!("{kind:?}"),
                    slot.to_string(),
                    digest.as_slice()
                ],
                |r| r.get(0),
            )
            .optional()
            .unwrap()
            .ok_or(ProviderError::ResetCursor)
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
        if request.url().ends_with("/upload") {
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
fn file(revision: &str, body: &[u8]) -> serde_json::Value {
    serde_json::json!({".tag":"file","id":"id:selected","name":"plan.md","path_lower":"/plans/plan.md","rev":revision,"content_hash":hex(&body_hash(&body_hash(body))),"size":body.len(),"server_modified":"2026-10-07T12:00:00Z"})
}
fn json(value: serde_json::Value) -> Response {
    Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap()).unwrap()
}
fn fixture(root: bool, steps: Vec<Step>) -> (Client, Arc<Script>, Arc<Authority>) {
    let key = RecordKey::new("id:selected").unwrap();
    let scope = if root {
        SelectedScope::ScopedRoot {
            root: RecordKey::new("/plans").unwrap(),
            recursive: true,
        }
    } else {
        SelectedScope::exact(vec![key.clone()]).unwrap()
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
        ProviderKind::Dropbox,
        DisplayText::new("Selected plans").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: root,
            assets: false,
            guarded_write: true,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    let selection = if root {
        Selection::AppFolderRoot {
            path: RecordKey::new("/plans").unwrap(),
            recursive: true,
        }
    } else {
        Selection::ExactFiles(vec![
            SelectedFile::new(key, RecordKey::new("/plans/plan.md").unwrap()).unwrap(),
        ])
    };
    let script = Arc::new(Script {
        steps: Mutex::new(steps.into()),
        requests: Mutex::new(vec![]),
        mutations: AtomicUsize::new(0),
    });
    let spool = rusqlite::Connection::open_in_memory().unwrap();
    spool.execute_batch("CREATE TABLE pages(binding BLOB,kind TEXT,slot TEXT,digest BLOB,bytes BLOB,PRIMARY KEY(binding,kind))").unwrap();
    let authority = Arc::new(Authority(AtomicBool::new(true), Mutex::new(spool)));
    let client = Client::new(
        descriptor,
        selection,
        Arc::new(Resolver),
        authority.clone(),
        script.clone(),
        [9; 32],
    )
    .unwrap();
    (client, script, authority)
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
async fn exact_file_download_checks_id_rev_hash_and_never_adopts_other_ids() {
    let mut headers = BTreeMap::new();
    headers.insert(
        "Dropbox-API-Result".into(),
        file("abcdef001", b"old").to_string(),
    );
    let (client, script, authority) = fixture(
        false,
        vec![Step::Reply(
            Response::new(200, headers, b"old".to_vec()).unwrap(),
        )],
    );
    let document = client
        .read(&RecordKey::new("id:selected").unwrap(), &context())
        .await
        .unwrap();
    assert_eq!(document.body.as_bytes(), b"old");
    assert_eq!(document.version.provider_revision.as_str(), "abcdef001");
    assert_eq!(
        document.version.content_hash.unwrap().algorithm(),
        HashAlgorithm::DropboxContentHash
    );
    assert_eq!(
        document.version.conditional_write_token.unwrap().as_str(),
        "abcdef001"
    );
    assert!(
        client
            .read(&RecordKey::new("id:unselected").unwrap(), &context())
            .await
            .is_err()
    );
    assert_eq!(script.requests.lock().unwrap().len(), 1);
    authority.0.store(false, Ordering::SeqCst);
    assert!(
        client
            .read(&RecordKey::new("id:selected").unwrap(), &context())
            .await
            .is_err()
    );
    assert_eq!(script.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn accepted_then_lost_upload_is_send_once_with_revision_mode_not_hash() {
    let (client, script, _) = fixture(
        false,
        vec![
            Step::Reply(json(file("abcdef001", b"old"))),
            Step::AcceptedThenLost,
        ],
    );
    let body = MarkdownBytes::new(b"new".to_vec()).unwrap();
    let key = RecordKey::new("id:selected").unwrap();
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("abcdef001").unwrap(),
        content_hash: Some(
            ContentHash::new(
                HashAlgorithm::DropboxContentHash,
                body_hash(&body_hash(b"old")).to_vec(),
            )
            .unwrap(),
        ),
        conditional_write_token: Some(OpaqueValue::new("abcdef001").unwrap()),
    };
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let id = journal::prepare(
        &mut db,
        client.descriptor().binding(),
        &key,
        &version,
        &body,
    )
    .unwrap();
    let permit = journal::begin_dispatch(&mut db, client.descriptor().binding(), id).unwrap();
    assert!(
        matches!(client.guarded_write(permit,&body,&context()).await,Err(ProviderError::UnknownWrite{operation_id})if operation_id==id)
    );
    assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
    assert!(journal::begin_dispatch(&mut db, client.descriptor().binding(), id).is_err());
    let requests = script.requests.lock().unwrap();
    let args: serde_json::Value =
        serde_json::from_str(&requests[1].headers()["dropbox-api-arg"]).unwrap();
    assert_eq!(args["path"], "id:selected");
    assert_eq!(args["mode"][".tag"], "update");
    assert_eq!(args["mode"]["update"], "abcdef001");
    assert_eq!(args["autorename"], false);
    assert_eq!(args["strict_conflict"], true);
    assert_eq!(requests[1].method(), Method::Post);
}
#[tokio::test]
async fn explicit_root_listing_change_delete_and_reset_keep_stable_ids_and_scoped_cursors() {
    let initial = serde_json::json!({"entries":[file("abcdef001",b"old")],"cursor":"list-checkpoint","has_more":false});
    let start = serde_json::json!({"cursor":"change-start"});
    let deleted = serde_json::json!({"entries":[{".tag":"deleted","path_lower":"/plans/plan.md","name":"plan.md"}],"cursor":"change-checkpoint","has_more":false});
    let reset = Response::new(
        409,
        BTreeMap::new(),
        br#"{"error":{".tag":"reset"}}"#.to_vec(),
    )
    .unwrap();
    let (client, script, _) = fixture(
        true,
        vec![
            Step::Reply(json(initial)),
            Step::Reply(json(start)),
            Step::Reply(json(deleted.clone())),
            Step::Reply(json(deleted)),
            Step::Reply(reset),
        ],
    );
    assert!(
        client
            .read(&RecordKey::new("id:selected").unwrap(), &context())
            .await
            .is_err()
    );
    assert_eq!(script.requests.lock().unwrap().len(), 0);
    let page = client.list(None, &context()).await.unwrap();
    assert_eq!(page.rows()[0].key.as_str(), "id:selected");
    assert!(matches!(
        page.coverage(),
        Coverage::MetadataCompleteSelected { .. }
    ));
    let cursor = client.start_changes(&context()).await.unwrap();
    for _ in 0..2 {
        let outcome = client.changes(&cursor, &context()).await.unwrap();
        assert!(
            matches!(outcome,ChangeOutcome::Page{events,..}if matches!(&events[0],Change::Deleted{key,..}if key.as_str()=="id:selected"))
        );
    }
    assert!(matches!(
        client.changes(&cursor, &context()).await.unwrap(),
        ChangeOutcome::RebuildRequired
    ));
    let requests = script.requests.lock().unwrap();
    let args: serde_json::Value = serde_json::from_slice(requests[0].body()).unwrap();
    assert_eq!(args["path"], "/plans");
    assert_eq!(args["recursive"], true);
}
#[tokio::test]
async fn outside_selected_root_and_corrupt_download_hash_fail_closed() {
    let mut outside = file("abcdef001", b"old");
    outside["path_lower"] = serde_json::json!("/private/plan.md");
    let (client, _, _) = fixture(
        true,
        vec![Step::Reply(json(
            serde_json::json!({"entries":[outside],"cursor":"c","has_more":false}),
        ))],
    );
    assert_eq!(
        client.list(None, &context()).await.unwrap_err(),
        ProviderError::Unauthorized
    );
    let mut headers = BTreeMap::new();
    headers.insert(
        "dropbox-api-result".into(),
        file("abcdef001", b"old").to_string(),
    );
    let (client, _, _) = fixture(
        false,
        vec![Step::Reply(
            Response::new(200, headers, b"bad".to_vec()).unwrap(),
        )],
    );
    assert_eq!(
        client
            .read(&RecordKey::new("id:selected").unwrap(), &context())
            .await
            .unwrap_err(),
        ProviderError::Protocol
    );
}

#[path = "native/create.rs"]
mod create;
#[path = "native/paging.rs"]
mod paging;
