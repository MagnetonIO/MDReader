use mdreader_provider::*;
use mdreader_service::provider::*;
use std::sync::{Arc, Mutex};
struct EmptyFactory;
#[async_trait::async_trait]
impl ProviderFactory for EmptyFactory {
    async fn describe(
        &self,
        _: LibraryId,
        _: SourceId,
        _: DisplayText,
        _: mdreader_credentials::CredentialReference,
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
        Err(ProviderError::Unsupported)
    }
}
#[test]
fn bounded_coordinator_lists_only_owner_registry() {
    let directory = tempfile::tempdir().unwrap();
    let connection = rusqlite::Connection::open(directory.path().join("library.sqlite3")).unwrap();
    let owner = Arc::new(Mutex::new(connection));
    let coordinator = Coordinator::new(
        owner,
        Arc::new(UnsupportedProjection),
        Arc::new(EmptyFactory),
    )
    .unwrap();
    assert_eq!(
        coordinator.execute(ProviderRequest::List).unwrap(),
        serde_json::json!([])
    );
}

use mdreader_credentials::*;
use mdreader_provider::transport::{Method, Request as HttpRequest, Response, Transport};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
#[derive(Default)]
struct MemoryVault(Mutex<BTreeMap<CredentialReference, ProviderGrant>>);
impl Vault for MemoryVault {
    fn get(&self, r: &CredentialReference) -> Result<Option<ProviderGrant>> {
        Ok(self.0.lock().unwrap().get(r).cloned())
    }
    fn create(&self, r: &CredentialReference, g: &ProviderGrant) -> Result<()> {
        let mut map = self.0.lock().unwrap();
        if map.contains_key(r) {
            return Err(ProviderError::Conflict);
        }
        map.insert(r.clone(), g.clone());
        Ok(())
    }
    fn compare_exchange(&self, r: &CredentialReference, n: u64, g: &ProviderGrant) -> Result<()> {
        let mut map = self.0.lock().unwrap();
        if map
            .get(r)
            .is_none_or(|prior| prior.secret().generation() != n)
        {
            return Err(ProviderError::Conflict);
        }
        map.insert(r.clone(), g.clone());
        Ok(())
    }
    fn delete(&self, r: &CredentialReference) -> Result<()> {
        self.0.lock().unwrap().remove(r);
        Ok(())
    }
}
const REFERENCE: &str = "native-provider-v3-514c36b5-7e58-43a4-9fc1-64f15471ea8e";
const SECOND_REFERENCE: &str = "native-provider-v3-66320de8-2124-49fa-9e17-23e8ed9fb3bd";
fn synthetic_grant(expires: u64) -> ProviderGrant {
    ProviderGrant::new(
        &Registration::google_drive("synthetic.apps.googleusercontent.com".into()).unwrap(),
        AccountBinding::new("verified-permission").unwrap(),
        NativeSelection::DriveFiles {
            files: vec![DriveFile {
                id: RecordKey::new("selected").unwrap(),
                resource_key: None,
            }],
        },
        SecretBundle::new(
            "synthetic-access".into(),
            "synthetic-refresh".into(),
            expires,
            [9; 32],
        )
        .unwrap(),
    )
    .unwrap()
}
struct SyntheticHttp {
    directory: PathBuf,
}
impl SyntheticHttp {
    fn state(&self) -> String {
        std::fs::read_to_string(self.directory.join("synthetic-remote-body"))
            .unwrap_or_else(|_| "old".into())
    }
    fn metadata(&self) -> Response {
        let body = self.state();
        let (new, hash) = if body == "old" {
            (false, "149603e6c03516362a8da23f624db945")
        } else {
            (true, "22af645d1859cb5ca6da0c484f1f37ea")
        };
        Response::new(200,BTreeMap::from([("etag".into(),if new{"\"v2\""}else{"\"v1\""}.into())]),serde_json::to_vec(&serde_json::json!({"id":"selected","name":"plan.md","mimeType":"text/markdown","version":if new{"2"}else{"1"},"md5Checksum":hash,"size":"3","trashed":false})).unwrap()).unwrap()
    }
}
#[async_trait::async_trait]
impl Transport for SyntheticHttp {
    async fn send(&self, r: HttpRequest) -> Result<Response> {
        let json = |v: serde_json::Value| {
            Response::new(200, BTreeMap::new(), serde_json::to_vec(&v).unwrap())
        };
        if r.url() == "https://oauth2.googleapis.com/token" {
            assert!(std::str::from_utf8(r.body())
                .unwrap()
                .contains("grant_type=refresh_token"));
            let form: BTreeMap<_, _> = url::form_urlencoded::parse(r.body()).into_owned().collect();
            assert_eq!(
                form.get("client_secret").map(String::as_str),
                Some("synthetic-desktop-parameter")
            );
            return json(
                serde_json::json!({"access_token":"rotated-access","expires_in":3600,"token_type":"Bearer","scope":"https://www.googleapis.com/auth/drive.file"}),
            );
        }
        if r.url().contains("/about?") {
            return json(serde_json::json!({"user":{"permissionId":"verified-permission"}}));
        }
        if r.method() == Method::Patch {
            assert_eq!(r.headers().get("if-match").unwrap(), "\"v1\"");
            let count = std::fs::read_to_string(self.directory.join("synthetic-patches"))
                .unwrap_or_else(|_| "0".into())
                .parse::<usize>()
                .unwrap();
            std::fs::write(
                self.directory.join("synthetic-patches"),
                (count + 1).to_string(),
            )
            .unwrap();
            std::fs::write(self.directory.join("synthetic-remote-body"), r.body()).unwrap();
            return Err(ProviderError::Unavailable); // provider accepted; response was lost
        }
        if r.url().contains("/changes/startPageToken") {
            return json(serde_json::json!({"startPageToken":"checkpoint"}));
        }
        if r.url().contains("/changes?") {
            return json(serde_json::json!({"changes":[],"newStartPageToken":"checkpoint"}));
        }
        assert!(r.url().contains("/files/selected?"));
        if r.url().contains("alt=media") {
            return Response::new(200, BTreeMap::new(), self.state().into_bytes());
        }
        Ok(self.metadata())
    }
}
#[test]
#[ignore]
fn provider_service_child() {
    let Some(directory) = std::env::var_os("MDREADER_SYNTHETIC_PROVIDER_DIRECTORY") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let memory = Arc::new(MemoryVault::default());
    memory
        .create(
            &CredentialReference::new(REFERENCE.into()).unwrap(),
            &synthetic_grant(1),
        )
        .unwrap();
    memory
        .create(
            &CredentialReference::new(SECOND_REFERENCE.into()).unwrap(),
            &synthetic_grant(1),
        )
        .unwrap();
    let factory = NativeFactory::new(
        VaultClient::new(memory).unwrap(),
        vec![
            Registration::google_drive("synthetic.apps.googleusercontent.com".into())
                .unwrap()
                .with_google_desktop_client_secret("synthetic-desktop-parameter".into())
                .unwrap(),
        ],
        Arc::new(SyntheticHttp {
            directory: directory.clone(),
        }),
    )
    .unwrap();
    mdreader_service::serve_with_provider_factory(
        &directory,
        mdreader_service::ServiceOptions {
            disable_model: true,
        },
        Arc::new(factory),
    )
    .unwrap();
}
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(directory: &std::path::Path) -> Child {
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider_service_child",
                "--ignored",
                "--nocapture",
            ])
            .env("MDREADER_SYNTHETIC_PROVIDER_DIRECTORY", directory)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    for _ in 0..150 {
        assert!(child.0.try_wait().unwrap().is_none(), "child exited");
        if mdreader_service::call(directory, &mdreader_service::Request::Ping).is_ok() {
            return child;
        }
        std::thread::sleep(Duration::from_millis(20))
    }
    panic!("synthetic service not ready")
}
fn rpc(directory: &std::path::Path, request: ProviderRequest) -> serde_json::Value {
    mdreader_service::call(directory, &mdreader_service::Request::Provider { request }).unwrap()
}
fn success(value: serde_json::Value) -> serde_json::Value {
    assert!(value["error"].is_null(), "{value}");
    value["result"].clone()
}
#[test]
fn native_factory_ipc_restart_unknown_write_and_revocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let child = start(path);
    let admitted = success(rpc(
        path,
        ProviderRequest::Admit {
            credential_reference: REFERENCE.into(),
            name: DisplayText::new("Selected plans").unwrap(),
        },
    ));
    let source = SourceId::new(admitted["source_id"].as_str().unwrap()).unwrap();
    let replay = success(rpc(
        path,
        ProviderRequest::Admit {
            credential_reference: REFERENCE.into(),
            name: DisplayText::new("Repeated reply").unwrap(),
        },
    ));
    assert_eq!(admitted["source_id"], replay["source_id"]);
    assert_eq!(replay["replayed"], true);
    success(rpc(
        path,
        ProviderRequest::Refresh {
            source_id: source.clone(),
        },
    ));
    let documents = success(rpc(
        path,
        ProviderRequest::Documents {
            source_id: source.clone(),
            offset: 0,
            limit: 10,
        },
    ));
    assert_eq!(documents["documents"].as_array().unwrap().len(), 1);
    assert!(!documents.to_string().contains("verified-permission"));
    assert!(!documents.to_string().contains("conditional_write_token"));
    assert!(!documents.to_string().contains("selected"));
    let id =
        uuid::Uuid::parse_str(documents["documents"][0]["document_id"].as_str().unwrap()).unwrap();
    let receipt = success(rpc(
        path,
        ProviderRequest::Open {
            source_id: source.clone(),
            document_id: id,
        },
    ));
    assert_eq!(
        success(rpc(
            path,
            ProviderRequest::CachedOpen {
                source_id: source.clone(),
                document_id: id
            }
        )),
        receipt
    );
    let search = mdreader_service::Request::Search {
        query: "old".into(),
        sources: vec![receipt["reader_source_id"].as_str().unwrap().into()],
        limit: 10,
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let hits = mdreader_service::call(path, &search).unwrap();
        if !hits.as_array().unwrap().is_empty() {
            assert_eq!(hits[0]["revision"], receipt["revision"]);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "automatic provider index did not publish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let read = mdreader_service::Request::Read {
        source_id: receipt["reader_source_id"].as_str().unwrap().into(),
        path: receipt["path"].as_str().unwrap().into(),
        revision: Some(receipt["revision"].as_str().unwrap().into()),
    };
    assert!(mdreader_service::call(path, &read)
        .unwrap()
        .to_string()
        .contains("old"));
    let edit = success(rpc(
        path,
        ProviderRequest::BeginEdit {
            source_id: source.clone(),
            document_id: id,
        },
    ));
    let edit_id = uuid::Uuid::parse_str(edit["edit_id"].as_str().unwrap()).unwrap();
    let save = ProviderRequest::PrepareSave {
        source_id: source.clone(),
        edit_id,
        markdown: "new".into(),
    };
    let prepared = success(rpc(path, save.clone()));
    assert_eq!(prepared, success(rpc(path, save)));
    let operation = uuid::Uuid::parse_str(prepared["operation_id"].as_str().unwrap()).unwrap();
    let dispatch = ProviderRequest::DispatchSave {
        source_id: source.clone(),
        operation_id: operation,
    };
    assert_eq!(
        rpc(path, dispatch.clone())["error"]["kind"],
        "unknown_write"
    );
    assert_eq!(rpc(path, dispatch)["error"]["kind"], "unknown_write");
    assert_eq!(
        std::fs::read_to_string(path.join("synthetic-patches")).unwrap(),
        "1"
    );
    drop(child);
    let _restarted = start(path);
    let listed = success(rpc(
        path,
        ProviderRequest::Documents {
            source_id: source.clone(),
            offset: 0,
            limit: 10,
        },
    ));
    assert_eq!(listed["documents"][0]["document_id"], id.to_string());
    assert_eq!(listed["documents"][0]["projection"], receipt);
    assert!(listed["documents"][0]["cached_observed_at_ms"].is_i64());
    let repaired = success(rpc(
        path,
        ProviderRequest::Repair {
            source_id: source.clone(),
        },
    ));
    assert_eq!(repaired["published"], 1);
    assert_eq!(
        std::fs::read_to_string(path.join("synthetic-patches")).unwrap(),
        "1"
    );
    let new_receipt = success(rpc(
        path,
        ProviderRequest::Open {
            source_id: source.clone(),
            document_id: id,
        },
    ));
    assert_ne!(new_receipt["revision"], receipt["revision"]);
    assert!(mdreader_service::call(path, &read).is_err());
    success(rpc(
        path,
        ProviderRequest::Disable {
            source_id: source.clone(),
        },
    ));
    assert_eq!(
        rpc(
            path,
            ProviderRequest::Open {
                source_id: source.clone(),
                document_id: id
            }
        )["error"]["kind"],
        "unauthorized"
    );
    assert_eq!(
        rpc(
            path,
            ProviderRequest::CachedOpen {
                source_id: source.clone(),
                document_id: id
            }
        )["error"]["kind"],
        "unauthorized"
    );
    let paused = success(rpc(
        path,
        ProviderRequest::Admit {
            credential_reference: REFERENCE.into(),
            name: DisplayText::new("Retry after pause").unwrap(),
        },
    ));
    assert_eq!(paused["enabled"], false);
    assert_eq!(paused["source_id"], admitted["source_id"]);
    assert!(mdreader_service::call(
        path,
        &mdreader_service::Request::Read {
            source_id: new_receipt["reader_source_id"].as_str().unwrap().into(),
            path: new_receipt["path"].as_str().unwrap().into(),
            revision: Some(new_receipt["revision"].as_str().unwrap().into())
        }
    )
    .is_err());
}
#[tokio::test]
async fn refresh_wrong_account_and_unregistered_grant_fail_before_custody_change() {
    struct WrongAccount(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl Transport for WrongAccount {
        async fn send(&self, request: HttpRequest) -> Result<Response> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let value = if request.url().contains("/token") {
                serde_json::json!({"access_token":"different-account-access","expires_in":3600,"token_type":"Bearer"})
            } else {
                serde_json::json!({"user":{"permissionId":"different-permission"}})
            };
            Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap())
        }
    }
    let owner = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
    let memory = Arc::new(MemoryVault::default());
    let reference = CredentialReference::fresh();
    memory.create(&reference, &synthetic_grant(1)).unwrap();
    let client = VaultClient::new(memory.clone()).unwrap();
    let transport = Arc::new(WrongAccount(std::sync::atomic::AtomicUsize::new(0)));
    let factory = NativeFactory::new(
        client.clone(),
        vec![Registration::google_drive("synthetic.apps.googleusercontent.com".into()).unwrap()],
        transport.clone(),
    )
    .unwrap();
    let context = IoContext::new(
        uuid::Uuid::new_v4(),
        std::time::Instant::now() + Duration::from_secs(2),
        256,
        1024 * 1024,
        Cancellation::default(),
    )
    .unwrap();
    let library = owner.with_connection(library_id).unwrap();
    let configuration = factory
        .describe(
            library.clone(),
            SourceId::new("source").unwrap(),
            DisplayText::new("Selected").unwrap(),
            reference.clone(),
            &context,
        )
        .await
        .unwrap();
    owner
        .with_connection(|c| {
            admit(
                c,
                &configuration.descriptor,
                &configuration.credential_reference,
                configuration.configuration_digest,
            )
        })
        .unwrap();
    let provider = factory
        .open(
            configuration,
            Arc::new(RegistryAuthority(owner)),
            vec![],
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        provider.list(None, &context).await.unwrap_err(),
        ProviderError::IdentityMismatch
    );
    assert_eq!(
        memory
            .get(&reference)
            .unwrap()
            .unwrap()
            .secret()
            .generation(),
        1
    );
    assert_eq!(transport.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    let unregistered = NativeFactory::new(
        client,
        vec![Registration::google_drive("other.apps.googleusercontent.com".into()).unwrap()],
        transport,
    )
    .unwrap();
    assert!(matches!(
        unregistered
            .describe(
                library,
                SourceId::new("other").unwrap(),
                DisplayText::new("Denied").unwrap(),
                reference,
                &context
            )
            .await,
        Err(ProviderError::Unauthorized)
    ));
}
#[test]
fn unavailable_provider_cache_reports_error_without_blocking_local_index() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("provider-cache"),
        b"synthetic obstruction",
    )
    .unwrap();
    let _child = start(directory.path());
    let response = rpc(directory.path(), ProviderRequest::List);
    assert!(response["result"].is_null());
    let expected = match CoreProjection::open(
        &directory.path().join("provider-cache"),
        LibraryId::new("synthetic").unwrap(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("obstructed cache unexpectedly opened"),
    };
    assert_eq!(response["error"], serde_json::to_value(expected).unwrap());
    let notes = tempfile::tempdir().unwrap();
    std::fs::write(notes.path().join("local.md"), "localavailabilityproof").unwrap();
    let root = mdreader_core::SourceRoot::capture(notes.path().to_path_buf()).unwrap();
    let source = mdreader_service::call(
        directory.path(),
        &mdreader_service::Request::Admit {
            name: "Local".into(),
            path: root.path.clone(),
            identity: root.identity.clone(),
        },
    )
    .unwrap();
    let id = source["id"].as_str().unwrap().to_owned();
    mdreader_service::call(
        directory.path(),
        &mdreader_service::Request::StartIndex {
            source_id: id.clone(),
        },
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let hits = mdreader_service::call(
            directory.path(),
            &mdreader_service::Request::Search {
                query: "localavailabilityproof".into(),
                sources: vec![id.clone()],
                limit: 10,
            },
        )
        .unwrap();
        if !hits.as_array().unwrap().is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "local index was blocked by provider initialization failure"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn lost_admission_replay_cannot_revive_replaced_grant_after_reconnect_or_forget() {
    use std::io::Write;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let child = start(path);
    let endpoint: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("reader-endpoint.json")).unwrap()).unwrap();
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", endpoint["port"].as_u64().unwrap() as u16))
            .unwrap();
    let request = mdreader_service::Request::Provider {
        request: ProviderRequest::Admit {
            credential_reference: REFERENCE.into(),
            name: DisplayText::new("Lost admission reply").unwrap(),
        },
    };
    let mut bytes = serde_json::to_vec(
        &serde_json::json!({"protocol":1,"token":endpoint["token"],"request":request}),
    )
    .unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).unwrap();
    stream.shutdown(std::net::Shutdown::Both).unwrap();
    drop(stream);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let original = loop {
        let listed = success(rpc(path, ProviderRequest::List));
        if listed.as_array().unwrap().len() == 1 {
            break listed[0].clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lost-reply admission did not commit"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let source = SourceId::new(original["source_id"].as_str().unwrap()).unwrap();
    success(rpc(
        path,
        ProviderRequest::Refresh {
            source_id: source.clone(),
        },
    ));
    let docs = success(rpc(
        path,
        ProviderRequest::Documents {
            source_id: source.clone(),
            offset: 0,
            limit: 10,
        },
    ));
    let old_id =
        uuid::Uuid::parse_str(docs["documents"][0]["document_id"].as_str().unwrap()).unwrap();
    let old = success(rpc(
        path,
        ProviderRequest::Open {
            source_id: source.clone(),
            document_id: old_id,
        },
    ));
    success(rpc(
        path,
        ProviderRequest::Disable {
            source_id: source.clone(),
        },
    ));
    let reconnected = success(rpc(
        path,
        ProviderRequest::Reconnect {
            source_id: source.clone(),
            credential_reference: SECOND_REFERENCE.into(),
            name: DisplayText::new("Fresh grant").unwrap(),
        },
    ));
    assert_eq!(reconnected["source_id"], original["source_id"]);
    success(rpc(
        path,
        ProviderRequest::Refresh {
            source_id: source.clone(),
        },
    ));
    let docs = success(rpc(
        path,
        ProviderRequest::Documents {
            source_id: source.clone(),
            offset: 0,
            limit: 10,
        },
    ));
    let new_id =
        uuid::Uuid::parse_str(docs["documents"][0]["document_id"].as_str().unwrap()).unwrap();
    let current = success(rpc(
        path,
        ProviderRequest::Open {
            source_id: source.clone(),
            document_id: new_id,
        },
    ));
    assert_ne!(current["reader_source_id"], old["reader_source_id"]);
    drop(child);
    let restarted = start(path);
    let replay = ProviderRequest::Admit {
        credential_reference: REFERENCE.into(),
        name: DisplayText::new("Delayed old reply reconciliation").unwrap(),
    };
    assert_eq!(rpc(path, replay.clone())["error"]["kind"], "conflict");
    let sources = success(rpc(path, ProviderRequest::List));
    assert_eq!(sources.as_array().unwrap().len(), 1);
    assert_eq!(sources[0]["reader_source_id"], current["reader_source_id"]);
    assert_eq!(sources[0]["enabled"], true);
    assert_eq!(
        rpc(
            path,
            ProviderRequest::Reconnect {
                source_id: source.clone(),
                credential_reference: REFERENCE.into(),
                name: DisplayText::new("Old grant reuse").unwrap()
            }
        )["error"]["kind"],
        "conflict"
    );
    success(rpc(
        path,
        ProviderRequest::Forget {
            source_id: source.clone(),
        },
    ));
    drop(restarted);
    let _restarted = start(path);
    assert_eq!(rpc(path, replay)["error"]["kind"], "conflict");
    let retained = success(rpc(
        path,
        ProviderRequest::Admit {
            credential_reference: SECOND_REFERENCE.into(),
            name: DisplayText::new("Current receipt after forget").unwrap(),
        },
    ));
    assert_eq!(retained["source_id"], original["source_id"]);
    assert_eq!(retained["enabled"], false);
    assert_eq!(retained["replayed"], true);
    let sources = success(rpc(path, ProviderRequest::List));
    assert_eq!(sources.as_array().unwrap().len(), 1);
    assert_eq!(sources[0]["enabled"], false);
    assert!(mdreader_service::call(
        path,
        &mdreader_service::Request::Read {
            source_id: current["reader_source_id"].as_str().unwrap().into(),
            path: current["path"].as_str().unwrap().into(),
            revision: Some(current["revision"].as_str().unwrap().into())
        }
    )
    .is_err());
}
#[path = "provider/cold_native_save.rs"]
mod cold_native_save;
#[path = "provider/disconnect.rs"]
mod disconnect;
