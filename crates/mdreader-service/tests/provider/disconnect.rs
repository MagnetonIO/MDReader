//! Real SQLite coordinator + native factory: disconnect removes the source and
//! its custody, keeps save evidence, and reports best-effort revocation.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Memory custody whose delete can be made to fail.
#[derive(Default)]
struct FlakyVault {
    inner: MemoryVault,
    fail_delete: AtomicBool,
}
impl Vault for FlakyVault {
    fn get(&self, r: &CredentialReference) -> Result<Option<ProviderGrant>> {
        self.inner.get(r)
    }
    fn create(&self, r: &CredentialReference, g: &ProviderGrant) -> Result<()> {
        self.inner.create(r, g)
    }
    fn compare_exchange(&self, r: &CredentialReference, n: u64, g: &ProviderGrant) -> Result<()> {
        self.inner.compare_exchange(r, n, g)
    }
    fn delete(&self, r: &CredentialReference) -> Result<()> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(ProviderError::CredentialUnavailable);
        }
        self.inner.delete(r)
    }
}
/// URL, headers and body of one recorded request.
type Recorded = (String, BTreeMap<String, String>, Vec<u8>);
/// Records every request; revocation answers with `revoke_status`.
struct RevokeHttp {
    requests: Mutex<Vec<Recorded>>,
    revoke_status: u16,
}
#[async_trait::async_trait]
impl Transport for RevokeHttp {
    async fn send(&self, r: HttpRequest) -> Result<Response> {
        self.requests.lock().unwrap().push((
            r.url().to_owned(),
            r.headers().clone(),
            r.body().to_vec(),
        ));
        if r.url() == "https://api.dropboxapi.com/oauth2/token" {
            return Response::new(
                200,
                BTreeMap::new(),
                br#"{"access_token":"exchanged-access","expires_in":3600,"token_type":"bearer"}"#
                    .to_vec(),
            );
        }
        Response::new(self.revoke_status, BTreeMap::new(), b"{}".to_vec())
    }
}
struct Harness {
    owner: Arc<Mutex<rusqlite::Connection>>,
    coordinator: Coordinator,
    vault: Arc<FlakyVault>,
    http: Arc<RevokeHttp>,
    _directory: tempfile::TempDir,
}
fn google_registration() -> Registration {
    Registration::google_drive("synthetic.apps.googleusercontent.com".into()).unwrap()
}
fn dropbox_registration() -> Registration {
    Registration::dropbox_selected_folder("publicSynthetic".into()).unwrap()
}
fn dropbox_grant(expires: u64) -> ProviderGrant {
    ProviderGrant::new(
        &dropbox_registration(),
        AccountBinding::new("public-account").unwrap(),
        NativeSelection::DropboxRoot {
            path: RecordKey::new("/plans").unwrap(),
            recursive: true,
        },
        SecretBundle::new(
            "dropbox-access".into(),
            "dropbox-refresh".into(),
            expires,
            [9; 32],
        )
        .unwrap(),
    )
    .unwrap()
    .with_verified_scopes(DROPBOX_EDIT_SCOPES)
    .unwrap()
}
fn harness(grants: &[(&str, ProviderGrant)], revoke_status: u16) -> Harness {
    let directory = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(
        rusqlite::Connection::open(directory.path().join("library.sqlite3")).unwrap(),
    ));
    let vault = Arc::new(FlakyVault::default());
    for (reference, grant) in grants {
        vault
            .create(
                &CredentialReference::new((*reference).into()).unwrap(),
                grant,
            )
            .unwrap();
    }
    let http = Arc::new(RevokeHttp {
        requests: Mutex::new(Vec::new()),
        revoke_status,
    });
    let factory = NativeFactory::new(
        VaultClient::new(vault.clone()).unwrap(),
        vec![google_registration(), dropbox_registration()],
        http.clone(),
    )
    .unwrap();
    let coordinator = Coordinator::new(
        owner.clone(),
        Arc::new(UnsupportedProjection),
        Arc::new(factory),
    )
    .unwrap();
    Harness {
        owner,
        coordinator,
        vault,
        http,
        _directory: directory,
    }
}
impl Harness {
    fn admit(&self, reference: &str) -> SourceId {
        let admitted = self
            .coordinator
            .execute(ProviderRequest::Admit {
                credential_reference: reference.into(),
                name: DisplayText::new("Synthetic account").unwrap(),
            })
            .unwrap();
        SourceId::new(admitted["source_id"].as_str().unwrap()).unwrap()
    }
    fn disconnect(&self, source: &SourceId) -> Result<serde_json::Value> {
        self.coordinator.execute(ProviderRequest::Disconnect {
            source_id: source.clone(),
        })
    }
    fn listed(&self) -> Vec<serde_json::Value> {
        let value = self.coordinator.execute(ProviderRequest::List).unwrap();
        value.as_array().unwrap().clone()
    }
    fn stored(&self, reference: &str) -> bool {
        self.vault
            .get(&CredentialReference::new(reference.into()).unwrap())
            .unwrap()
            .is_some()
    }
    fn urls(&self) -> Vec<String> {
        let requests = self.http.requests.lock().unwrap();
        requests.iter().map(|(url, _, _)| url.clone()).collect()
    }
}

#[test]
fn disconnect_removes_source_and_custody_then_revokes_and_never_replays() {
    let h = harness(&[(REFERENCE, synthetic_grant(u64::MAX / 2))], 200);
    let source = h.admit(REFERENCE);
    assert_eq!(h.listed().len(), 1);
    let result = h.disconnect(&source).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"disconnected":true,"credential":"deleted","revocation":"revoked"})
    );
    assert!(h.listed().is_empty());
    assert!(!h.stored(REFERENCE));
    {
        let requests = h.http.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let (url, _, body) = &requests[0];
        assert_eq!(url, "https://oauth2.googleapis.com/revoke");
        let form: BTreeMap<_, _> = url::form_urlencoded::parse(body).into_owned().collect();
        assert_eq!(
            form.get("token").map(String::as_str),
            Some("synthetic-refresh")
        );
    }
    assert!(h
        .coordinator
        .execute(ProviderRequest::Documents {
            source_id: source.clone(),
            offset: 0,
            limit: 10
        })
        .is_err());
    // A late replay of the consumed reference cannot recreate the source.
    h.vault
        .create(
            &CredentialReference::new(REFERENCE.into()).unwrap(),
            &synthetic_grant(u64::MAX / 2),
        )
        .unwrap();
    assert!(matches!(
        h.coordinator.execute(ProviderRequest::Admit {
            credential_reference: REFERENCE.into(),
            name: DisplayText::new("Delayed replay").unwrap(),
        }),
        Err(ProviderError::Conflict)
    ));
    assert!(h.listed().is_empty());
}

#[test]
fn provider_revoke_failure_is_reported_without_blocking_local_disconnect() {
    let h = harness(&[(REFERENCE, synthetic_grant(u64::MAX / 2))], 500);
    let source = h.admit(REFERENCE);
    let result = h.disconnect(&source).unwrap();
    assert_eq!(result["revocation"], "failed");
    assert_eq!(result["credential"], "deleted");
    assert!(h.listed().is_empty());
    assert!(!h.stored(REFERENCE));
}

#[test]
fn dropbox_revoke_exchanges_an_expired_token_and_uses_bearer_auth() {
    let h = harness(&[(REFERENCE, dropbox_grant(1))], 200);
    let source = h.admit(REFERENCE);
    assert_eq!(h.disconnect(&source).unwrap()["revocation"], "revoked");
    assert_eq!(
        h.urls(),
        [
            "https://api.dropboxapi.com/oauth2/token",
            "https://api.dropboxapi.com/2/auth/token/revoke"
        ]
    );
    let requests = h.http.requests.lock().unwrap();
    let form: BTreeMap<_, _> = url::form_urlencoded::parse(&requests[0].2)
        .into_owned()
        .collect();
    assert_eq!(
        form.get("refresh_token").map(String::as_str),
        Some("dropbox-refresh")
    );
    assert_eq!(requests[1].1["Authorization"], "Bearer exchanged-access");
}

#[test]
fn custody_delete_failure_keeps_a_paused_source_that_can_be_disconnected_again() {
    let h = harness(&[(REFERENCE, synthetic_grant(u64::MAX / 2))], 200);
    let source = h.admit(REFERENCE);
    h.vault.fail_delete.store(true, Ordering::SeqCst);
    assert!(h.disconnect(&source).is_err());
    let listed = h.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["enabled"], false);
    assert!(h.stored(REFERENCE));
    assert!(h.urls().is_empty(), "no revoke before custody is deleted");
    h.vault.fail_delete.store(false, Ordering::SeqCst);
    assert_eq!(h.disconnect(&source).unwrap()["revocation"], "revoked");
    assert!(h.listed().is_empty());
    assert!(!h.stored(REFERENCE));
}

#[test]
fn a_shared_reference_is_neither_deleted_nor_revoked() {
    let h = harness(
        &[
            (REFERENCE, synthetic_grant(u64::MAX / 2)),
            (SECOND_REFERENCE, synthetic_grant(u64::MAX / 2)),
        ],
        200,
    );
    let first = h.admit(REFERENCE);
    let second = h.admit(SECOND_REFERENCE);
    h.owner
        .lock()
        .unwrap()
        .execute(
            "UPDATE mdreader_provider_registry_sources SET credential_reference=?1",
            [REFERENCE],
        )
        .unwrap();
    let result = h.disconnect(&first).unwrap();
    assert_eq!(result["credential"], "shared");
    assert_eq!(result["revocation"], "not_attempted");
    assert!(h.stored(REFERENCE));
    assert!(h.urls().is_empty());
    let listed = h.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["source_id"], second.as_str());
}

#[test]
fn unsaved_save_evidence_survives_disconnect_and_stays_recoverable() {
    let h = harness(&[(REFERENCE, synthetic_grant(u64::MAX / 2))], 200);
    let source = h.admit(REFERENCE);
    let binding = h
        .owner
        .with_connection(|c| configuration(c, &source))
        .unwrap()
        .descriptor
        .binding()
        .clone();
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("1").unwrap(),
        content_hash: None,
        conditional_write_token: None,
    };
    h.owner
        .with_connection(|c| {
            mdreader_provider::journal::prepare(
                c,
                &binding,
                &RecordKey::new("selected").unwrap(),
                &version,
                &MarkdownBytes::new(b"unsaved local text".to_vec()).unwrap(),
            )
        })
        .unwrap();
    let drafts = |h: &Harness| {
        h.coordinator
            .execute(ProviderRequest::RecoveredDrafts {
                offset: 0,
                limit: 10,
            })
            .unwrap()["drafts"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(drafts(&h), 1);
    h.disconnect(&source).unwrap();
    assert!(h.listed().is_empty());
    assert_eq!(drafts(&h), 1);
}
