//! Real native Drive refusal through durable service publication, synthetic HTTP.
use async_trait::async_trait;
use mdreader_connector_google_drive::native::{AccessGrant, Client, CredentialResolver};
use mdreader_provider::{
    transport::{Method, Request, Response, Transport},
    *,
};
use mdreader_service::provider::{self, Owner};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

struct Credentials;
#[async_trait]
impl CredentialResolver for Credentials {
    async fn access(&self, _: &SourceBinding, _: &IoContext) -> Result<AccessGrant> {
        AccessGrant::new(
            "synthetic-access".into(),
            SystemTime::now() + Duration::from_secs(300),
            1,
        )
    }
    async fn refresh(&self, _: &SourceBinding, _: u64, _: &IoContext) -> Result<AccessGrant> {
        Err(ProviderError::ReconnectRequired)
    }
}
fn folder(id: &str, parents: &[&str]) -> Value {
    json!({"id":id,"name":id,"mimeType":"application/vnd.google-apps.folder","parents":parents})
}
fn note() -> Value {
    json!({"id":"note","name":"Public.md","mimeType":"text/plain","version":"1","parents":["sub"]})
}
struct Http {
    refused: Mutex<Option<String>>,
    changes: Mutex<Value>,
}
#[async_trait]
impl Transport for Http {
    async fn send(&self, request: Request) -> Result<Response> {
        assert_eq!(request.method(), Method::Get);
        let url = url::Url::parse(request.url()).unwrap();
        assert!(!url.query_pairs().any(|(k, _)| k == "alt"));
        let value = if url.path().ends_with("/startPageToken") {
            json!({"startPageToken":"initial"})
        } else if url.path().ends_with("/changes") {
            self.changes.lock().unwrap().clone()
        } else if url.path().ends_with("/files") {
            let q = url
                .query_pairs()
                .find(|(k, _)| k == "q")
                .unwrap()
                .1
                .into_owned();
            if q.contains("'notes'") {
                json!({"files":[folder("sub", &["notes"])]})
            } else {
                assert!(q.contains("'sub'"));
                json!({"files":[note()]})
            }
        } else {
            let key = url.path_segments().unwrap().next_back().unwrap();
            if self.refused.lock().unwrap().as_deref() == Some(key) {
                return Response::new(403, BTreeMap::new(), vec![]);
            }
            match key {
                "notes" => folder("notes", &[]),
                "sub" => folder("sub", &["notes"]),
                "note" => note(),
                _ => panic!("unexpected fixture request"),
            }
        };
        Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap())
    }
}
fn context() -> IoContext {
    IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        16,
        1024 * 1024,
        Cancellation::default(),
    )
    .unwrap()
}
fn client(
    d: SourceDescriptor,
    owner: Arc<Mutex<rusqlite::Connection>>,
    http: Arc<Http>,
    known: Vec<Metadata>,
) -> Client {
    Client::new_folder(
        d,
        RecordKey::new("notes").unwrap(),
        true,
        Arc::new(Credentials),
        Arc::new(provider::RegistryAuthority(owner)),
        http,
        [7; 32],
    )
    .unwrap()
    .with_known_metadata(known)
    .unwrap()
}
fn descriptor(c: &mut rusqlite::Connection) -> SourceDescriptor {
    let scope = SelectedScope::DriveFolder {
        root: RecordKey::new("notes").unwrap(),
        recursive: true,
    };
    SourceDescriptor::new(
        SourceBinding::new(
            provider::library_id(c).unwrap(),
            SourceId::new("synthetic-drive").unwrap(),
            AccountBinding::new("synthetic-account").unwrap(),
            uuid::Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Public notes").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: true,
            guarded_write: false,
            assets: false,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap()
}
#[tokio::test]
async fn remote_file_and_structural_403_preserve_metadata_checkpoint_and_reopen_retry() {
    for (key, changed) in [("note", note()), ("sub", folder("sub", &["notes"]))] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("public-registry.sqlite");
        let mut c = rusqlite::Connection::open(&path).unwrap();
        let d = descriptor(&mut c);
        let binding = d.binding().clone();
        provider::admit(
            &mut c,
            &d,
            &OpaqueValue::new("synthetic-vault-reference").unwrap(),
            [8; 32],
        )
        .unwrap();
        let owner = Arc::new(Mutex::new(c));
        let http = Arc::new(Http {
            refused: Mutex::new(None),
            changes: Mutex::new(json!({"changes":[],"newStartPageToken":"published"})),
        });
        let native = client(d.clone(), owner.clone(), http.clone(), vec![]);
        let mut completed = false;
        for _ in 0..5 {
            if provider::synchronize(
                owner.as_ref(),
                &native,
                &provider::UnsupportedProjection,
                &context(),
            )
            .await
            .unwrap()
            .metadata_complete
            {
                completed = true;
                break;
            }
        }
        assert!(completed);
        let before = owner
            .with_connection(|c| provider::checkpoint(c, &binding))
            .unwrap()
            .unwrap();
        let original = owner
            .with_connection(|c| provider::metadata(c, &binding, &RecordKey::new("note").unwrap()))
            .unwrap();
        *http.refused.lock().unwrap() = Some(key.into());
        *http.changes.lock().unwrap() =
            json!({"changes":[{"fileId":key,"file":changed}],"newStartPageToken":"after-refusal"});
        assert!(matches!(
            provider::synchronize(
                owner.as_ref(),
                &native,
                &provider::UnsupportedProjection,
                &context()
            )
            .await,
            Err(ProviderError::Unavailable)
        ));
        assert_eq!(
            owner
                .with_connection(|c| provider::checkpoint(c, &binding))
                .unwrap(),
            Some(before.clone())
        );
        let pending = owner
            .with_connection(|c| provider::resume(c, &binding))
            .unwrap()
            .unwrap();
        assert_eq!(pending.change_cursor, Some(before.clone()));
        assert_eq!(
            owner
                .with_connection(|c| provider::metadata(c, &binding, &original.key))
                .unwrap()
                .version,
            original.version
        );
        drop(native);
        drop(owner);
        let owner = Arc::new(Mutex::new(rusqlite::Connection::open(&path).unwrap()));
        assert_eq!(
            owner
                .with_connection(|c| provider::checkpoint(c, &binding))
                .unwrap(),
            Some(before)
        );
        let summary = owner
            .with_connection(|c| provider::list(c, binding.library_id()))
            .unwrap();
        let listing = summary[0].metadata_listing.as_ref().unwrap();
        assert_eq!(listing.known_documents, 1);
        assert!(matches!(
            listing.completeness,
            provider::ListingCompleteness::CompleteSelected
        ));
        assert!(listing.refresh_pending);
        let resumed = owner
            .with_connection(|c| provider::metadata(c, &binding, &original.key))
            .unwrap();
        assert_eq!(resumed.version, original.version);
        *http.refused.lock().unwrap() = None;
        let native = client(d, owner.clone(), http, vec![resumed]);
        let mut completed = false;
        for _ in 0..8 {
            if provider::synchronize(
                owner.as_ref(),
                &native,
                &provider::UnsupportedProjection,
                &context(),
            )
            .await
            .unwrap()
            .metadata_complete
            {
                completed = true;
                break;
            }
        }
        assert!(completed);
        assert_eq!(
            owner
                .with_connection(|c| provider::metadata(c, &binding, &original.key))
                .unwrap()
                .version,
            original.version
        );
        assert_eq!(
            owner
                .with_connection(|c| Ok(c
                    .query_row(
                        "SELECT count(*) FROM mdreader_provider_registry_cache",
                        [],
                        |r| r.get::<_, u32>(0)
                    )
                    .unwrap()))
                .unwrap(),
            0
        );
    }
}
