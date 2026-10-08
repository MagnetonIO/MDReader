use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

struct TestOwner {
    db: Mutex<rusqlite::Connection>,
    scope: RwLock<Option<SyncScope>>,
}
impl Owner for TestOwner {
    fn with_store<T>(
        &self,
        expected: &SyncScope,
        action: impl FnOnce(&mut SyncStore<'_>) -> Result<T>,
    ) -> TransportResult<T> {
        let scope = self.scope.read().unwrap();
        if scope.as_ref() != Some(expected) {
            return Err(TransportError::Authority);
        }
        let mut db = self.db.lock().unwrap();
        Ok(action(&mut SyncStore::attach(&mut db, expected.clone())?)?)
    }
}
struct TestAuthority {
    scope: SyncScope,
    encoding: jsonwebtoken::EncodingKey,
    jwk: Value,
    token: zeroize::Zeroizing<String>,
    login_token: zeroize::Zeroizing<String>,
    library: LibraryKey,
    epoch: std::sync::atomic::AtomicU64,
}
impl TestAuthority {
    fn candidate(library_id: String) -> Self {
        let rng = SystemRandom::new();
        let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.as_ref(), &rng).unwrap();
        let point = key.public_key().as_ref();
        let jwk = json!({"crv":"P-256","kty":"EC","x":URL_SAFE_NO_PAD.encode(&point[1..33]),"y":URL_SAFE_NO_PAD.encode(&point[33..])});
        let device = URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&jwk).unwrap()));
        let account =
            blob_digest(br#"["https://native-synthetic-issuer.example.test","synthetic-alice"]"#);
        Self {
            scope: SyncScope::new(account, library_id, device).unwrap(),
            encoding: jsonwebtoken::EncodingKey::from_ec_der(der.as_ref()),
            jwk,
            token: zeroize::Zeroizing::new(String::new()),
            login_token: zeroize::Zeroizing::new(String::new()),
            library: LibraryKey::generate().unwrap(),
            epoch: std::sync::atomic::AtomicU64::new(1),
        }
    }
}
impl Authority for TestAuthority {
    fn scope(&self) -> TransportResult<SyncScope> {
        Ok(self.scope.clone())
    }
    fn generation(&self) -> TransportResult<u64> {
        Ok(self.epoch.load(std::sync::atomic::Ordering::SeqCst))
    }
    fn authorize(&self, method: &str, url: &str, body: &[u8]) -> TransportResult<Authorization> {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.typ = Some("mdreader-proof+jwt".into());
        header.jwk = Some(serde_json::from_value(self.jwk.clone()).unwrap());
        let mut payload = json!({"v":2,"htm":method,"htu":url,"ath":URL_SAFE_NO_PAD.encode(Sha256::digest(self.token.as_bytes())),"bht":URL_SAFE_NO_PAD.encode(Sha256::digest(body)),"iat":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),"jti":uuid::Uuid::new_v4().to_string()});
        if method == "POST" && url.ends_with("/v1/enrollment/bootstrap") {
            payload["lth"] =
                json!(URL_SAFE_NO_PAD.encode(Sha256::digest(self.login_token.as_bytes())));
        }
        let proof = zeroize::Zeroizing::new(
            jsonwebtoken::encode(&header, &payload, &self.encoding).unwrap(),
        );
        Authorization::new(&self.token, &proof)
    }
    fn authenticate_root(
        &self,
        scope: &SyncScope,
        change: &RemoteChange,
        body: &[u8],
    ) -> TransportResult<PreparedRevision> {
        Ok(PreparedRevision::from_wire(
            &self.library,
            &EncryptionContext {
                library_id: scope.library_id().into(),
                item_id: change.item_id.clone(),
                revision_id: change.revision_id.clone(),
                key_id: self.library.key_id().into(),
            },
            change.blob.as_ref().ok_or(TransportError::Protocol)?,
            body,
        )?)
    }
}
struct Fixture {
    child: Child,
    _temp: tempfile::TempDir,
    origin: String,
    cert: reqwest::Certificate,
}
impl Fixture {
    fn start() -> Self {
        let web = std::env::var("MDREADER_SYNC_WEB_WORKTREE")
            .expect("set MDREADER_SYNC_WEB_WORKTREE to the synthetic Worker worktree");
        let temp = tempfile::TempDir::new().unwrap();
        let mut child = Command::new("node")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/support/worker-bridge.mjs"
            ))
            .current_dir(&web)
            .arg(web)
            .arg(temp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        let value: Value = serde_json::from_str(&ready).expect("synthetic local bridge must start");
        let cert = reqwest::Certificate::from_pem(
            &std::fs::read(value["cert"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        Self {
            child,
            _temp: temp,
            origin: value["origin"].as_str().unwrap().into(),
            cert,
        }
    }
    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(self.cert.clone())
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }
    async fn json(&self, path: &str, body: Value) -> Value {
        let result = self
            .client()
            .post(format!("{}{path}", self.origin))
            .body(serde_json::to_vec(&body).unwrap())
            .send()
            .await
            .unwrap();
        assert!(
            result.status().is_success(),
            "synthetic fixture request failed"
        );
        serde_json::from_slice(&result.bytes().await.unwrap()).unwrap()
    }
    async fn enroll(&self) -> TestAuthority {
        let mut authority = TestAuthority::candidate(uuid::Uuid::new_v4().to_string());
        // Keys are generated/held in native Rust. The local synthetic issuer holds only its
        // issuer signing key and returns a standard RS256 ID token with challenge nonce.
        let wrap = TestAuthority::candidate(authority.scope.library_id().into());
        let recovery = TestAuthority::candidate(authority.scope.library_id().into());
        let c=self.json("/v1/enrollment/challenge",json!({"kind":"bootstrap","libraryId":authority.scope.library_id(),"signingJwk":authority.jwk,"wrappingJwk":wrap.jwk,"recoveryJwk":recovery.jwk})).await;
        let token = self
            .json("/__synthetic/token", json!({"nonce":c["nonce"]}))
            .await;
        authority.token = zeroize::Zeroizing::new(token["token"].as_str().unwrap().into());
        authority.login_token =
            zeroize::Zeroizing::new(token["loginToken"].as_str().unwrap().into());
        let body=serde_json::to_vec(&json!({"challenge":c["challenge"],"signingJwk":authority.jwk,"wrappingJwk":wrap.jwk,"recoveryJwk":recovery.jwk})).unwrap();
        let url = format!("{}/v1/enrollment/bootstrap", self.origin);
        let auth = authority.authorize("POST", &url, &body).unwrap();
        let result = self
            .client()
            .post(url)
            .header(AUTHORIZATION, auth.bearer)
            .header("mdreader-proof", auth.proof)
            .header("mdreader-login-token", authority.login_token.as_str())
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            result.status().as_u16(),
            201,
            "native synthetic enrollment must succeed"
        );
        let receipt: Value = serde_json::from_slice(&result.bytes().await.unwrap()).unwrap();
        assert_eq!(receipt["account"], authority.scope.account_digest());
        assert_eq!(receipt["libraryId"], authority.scope.library_id());
        assert_eq!(receipt["device"], authority.scope.device_id());
        authority
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
        }
        for _ in 0..100 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn owner(temp: &tempfile::TempDir, scope: &SyncScope) -> TestOwner {
    let mut db = rusqlite::Connection::open(temp.path().join("synthetic-owner.sqlite")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;CREATE TABLE IF NOT EXISTS owner_projection(value TEXT);").unwrap();
    migrate(&mut db).unwrap();
    TestOwner {
        db: Mutex::new(db),
        scope: RwLock::new(Some(scope.clone())),
    }
}
fn enqueue(owner: &TestOwner, auth: &TestAuthority) -> OutboxEntry {
    let context = EncryptionContext {
        library_id: auth.scope.library_id().into(),
        item_id: uuid::Uuid::new_v4().to_string(),
        revision_id: uuid::Uuid::new_v4().to_string(),
        key_id: auth.library.key_id().into(),
    };
    owner
        .with_store(&auth.scope, |s| {
            s.seal_enqueue(
                &uuid::Uuid::new_v4().to_string(),
                &[],
                &auth.library,
                &context,
                b"synthetic native root",
            )
        })
        .unwrap()
}
#[test]
fn production_endpoint_and_sensitive_headers_fail_closed() {
    for invalid in [
        "http://localhost:1234",
        "https://user:secret@example.test",
        "https://example.test/v1",
        "https://example.test/?x=1",
        "https://example.test/#x",
    ] {
        assert!(Transport::new(invalid).is_err());
    }
    assert!(Authorization::new("bad\ntoken", "proof").is_err());
    assert!(Transport::new("https://sync.example.test").is_ok());
}
#[tokio::test]
#[ignore = "requires explicit synthetic local Worker bundle; no live issuer"]
async fn actual_worker_immutable_retry_scoped_receipts_staging_and_applied_only_ack() {
    let fixture = Fixture::start();
    let auth = fixture.enroll().await;
    let temp = tempfile::TempDir::new().unwrap();
    let owner = owner(&temp, &auth.scope);
    let entry = enqueue(&owner, &auth);
    let transport = Transport::build(&fixture.origin, Some(fixture.cert.clone())).unwrap();
    fixture
        .json("/__synthetic/control", json!({"weakUpload":true}))
        .await;
    assert!(matches!(
        transport.run(&owner, &auth, Budget::default()).await,
        Err(TransportError::Local(_))
    ));
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| s.entry(&entry.operation.operation_id))
            .unwrap()
            .unwrap()
            .state,
        OutboxState::Pending
    );
    fixture
        .json("/__synthetic/control", json!({"dropCommit":true}))
        .await;
    assert!(matches!(
        transport.run(&owner, &auth, Budget::default()).await,
        Err(TransportError::Retryable)
    ));
    let report = transport
        .run(&owner, &auth, Budget::default())
        .await
        .unwrap();
    assert_eq!(
        (
            report.committed,
            report.received,
            report.applied,
            report.acknowledged
        ),
        (1, 1, 0, 0)
    );
    let stats = fixture.json("/__synthetic/control", json!({})).await;
    let hashes = stats["uploadHashes"].as_array().unwrap();
    assert_eq!(hashes.len(), 3);
    assert!(hashes.iter().all(|h| h == &hashes[0]));
    assert_eq!(stats["commitHashes"][0], stats["commitHashes"][1]);
    assert_eq!(stats["acks"], json!([]));
    let blob = entry.operation.blob.clone().unwrap();
    let body = entry.envelope_utf8.as_ref().unwrap();
    let downloaded = transport
        .download_resources(&owner, &auth, std::slice::from_ref(&blob), body.len())
        .await
        .unwrap();
    assert_eq!(downloaded[0].blob, blob);
    assert_eq!(&downloaded[0].envelope_utf8, body);
    assert!(matches!(
        transport
            .download_resources(&owner, &auth, std::slice::from_ref(&blob), body.len() - 1)
            .await,
        Err(TransportError::Quota)
    ));
    assert!(matches!(
        transport
            .download_resources(&owner, &auth, &[blob.clone(), blob], body.len() * 2)
            .await,
        Err(TransportError::Protocol)
    ));
    assert_eq!(
        owner.with_store(&auth.scope, |s| s.cursors()).unwrap(),
        CursorState {
            received: 1,
            applied: 0,
            server_acknowledged: 0
        }
    );
    drop(owner);
    let owner = self::owner(&temp, &auth.scope);
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| s.cursors())
            .unwrap()
            .received,
        1
    );
    owner
        .with_store(&auth.scope, |s| {
            s.mark_applied(1, |tx| {
                tx.execute(
                    "INSERT INTO owner_projection VALUES('applied typed fixture only')",
                    [],
                )?;
                Err(SyncError::NotReady)
            })
        })
        .unwrap_err();
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| s.cursors())
            .unwrap()
            .applied,
        0
    );
    assert_eq!(
        owner
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM owner_projection", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
    owner
        .with_store(&auth.scope, |s| {
            s.mark_applied(1, |tx| {
                tx.execute(
                    "INSERT INTO owner_projection VALUES('applied typed fixture only')",
                    [],
                )?;
                Ok(())
            })
        })
        .unwrap();
    fixture
        .json("/__synthetic/control", json!({"dropAck":true}))
        .await;
    assert!(matches!(
        transport
            .run(
                &owner,
                &auth,
                Budget {
                    outgoing: 0,
                    pages: 0,
                    page_size: 1,
                    ..Budget::default()
                }
            )
            .await,
        Err(TransportError::Retryable)
    ));
    assert_eq!(
        owner.with_store(&auth.scope, |s| s.cursors()).unwrap(),
        CursorState {
            received: 1,
            applied: 1,
            server_acknowledged: 0
        }
    );
    drop(owner);
    let owner = self::owner(&temp, &auth.scope);
    let report = transport
        .run(
            &owner,
            &auth,
            Budget {
                outgoing: 0,
                pages: 0,
                page_size: 1,
                ..Budget::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (report.received, report.applied, report.acknowledged),
        (1, 1, 1)
    );
    let context = EncryptionContext {
        library_id: auth.scope.library_id().into(),
        item_id: entry.operation.item_id.clone(),
        revision_id: uuid::Uuid::new_v4().to_string(),
        key_id: auth.library.key_id().into(),
    };
    let branch = owner
        .with_store(&auth.scope, |s| {
            s.seal_enqueue(
                &uuid::Uuid::new_v4().to_string(),
                &[],
                &auth.library,
                &context,
                b"synthetic concurrent branch",
            )
        })
        .unwrap();
    let report = transport
        .run(&owner, &auth, Budget::default())
        .await
        .unwrap();
    assert_eq!(
        (
            report.conflicts,
            report.received,
            report.applied,
            report.acknowledged
        ),
        (1, 2, 1, 1)
    );
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| s.heads(&entry.operation.item_id))
            .unwrap()
            .len(),
        2
    );
    owner
        .with_store(&auth.scope, |s| {
            s.enqueue_tombstone(
                &uuid::Uuid::new_v4().to_string(),
                &entry.operation.item_id,
                &uuid::Uuid::new_v4().to_string(),
                &[
                    entry.operation.revision_id.clone(),
                    branch.operation.revision_id.clone(),
                ],
            )
        })
        .unwrap();
    let report = transport
        .run(&owner, &auth, Budget::default())
        .await
        .unwrap();
    assert_eq!(
        (report.received, report.applied, report.acknowledged),
        (3, 1, 1)
    );
    let heads = owner
        .with_store(&auth.scope, |s| s.heads(&entry.operation.item_id))
        .unwrap();
    assert_eq!(heads.len(), 1);
    assert!(heads[0].change.deleted);
    let url = format!("{}/v1/devices/{}", fixture.origin, auth.scope.device_id());
    let signed = auth.authorize("DELETE", &url, b"").unwrap();
    let revoked = fixture
        .client()
        .delete(url)
        .header(AUTHORIZATION, signed.bearer)
        .header("mdreader-proof", signed.proof)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status().as_u16(), 200);

    assert!(matches!(
        transport.run(&owner, &auth, Budget::default()).await,
        Err(TransportError::Forbidden)
    ));
}
#[tokio::test]
#[ignore = "requires explicit synthetic local Worker bundle; no live issuer"]
async fn actual_worker_owner_unlocked_scope_revalidated_single_run_and_cancel_safe() {
    let fixture = Fixture::start();
    let auth = fixture.enroll().await;
    let temp = tempfile::TempDir::new().unwrap();
    let owner = Arc::new(owner(&temp, &auth.scope));
    enqueue(&owner, &auth);
    let transport = Transport::build(&fixture.origin, Some(fixture.cert.clone())).unwrap();
    fixture
        .json("/__synthetic/control", json!({"stall":true}))
        .await;
    let run = transport.run(owner.as_ref(), &auth, Budget::default());
    tokio::pin!(run);
    let checks = async {
        for _ in 0..100 {
            if fixture.json("/__synthetic/control", json!({})).await["waiting"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            owner.db.try_lock().is_ok(),
            "owner cannot be locked across HTTP await"
        );
        let other = Transport::build(&fixture.origin, Some(fixture.cert.clone())).unwrap();
        assert!(matches!(
            other.run(owner.as_ref(), &auth, Budget::default()).await,
            Err(TransportError::Busy)
        ));
        *owner.scope.write().unwrap() = None;
        fixture
            .json("/__synthetic/control", json!({"release":true}))
            .await;
    };
    let (result, ()) = tokio::join!(&mut run, checks);
    assert!(matches!(result, Err(TransportError::Authority)));
    *owner.scope.write().unwrap() = Some(auth.scope.clone());
    assert_eq!(
        owner.with_store(&auth.scope, |s| s.pending(1)).unwrap()[0].state,
        OutboxState::Pending
    );
    let report = transport
        .run(owner.as_ref(), &auth, Budget::default())
        .await
        .unwrap();
    assert_eq!(report.committed, 1);
}

#[tokio::test]
#[ignore = "requires explicit synthetic local Worker bundle; no live issuer"]
async fn actual_worker_cancel_releases_scope_gate_and_retry_reuses_saved_bytes() {
    let fixture = Fixture::start();
    let auth = fixture.enroll().await;
    let temp = tempfile::TempDir::new().unwrap();
    let owner = owner(&temp, &auth.scope);
    let entry = enqueue(&owner, &auth);
    let transport = Transport::build(&fixture.origin, Some(fixture.cert.clone())).unwrap();
    fixture
        .json("/__synthetic/control", json!({"stall":true}))
        .await;
    let mut run = Box::pin(transport.run(&owner, &auth, Budget::default()));
    let waiting = async {
        for _ in 0..100 {
            if fixture.json("/__synthetic/control", json!({})).await["waiting"] == true {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("fixture upload must reach network barrier");
    };
    tokio::select! { result=&mut run=>panic!("unexpected completion: {result:?}"), ()=waiting=>{} }
    drop(run);
    assert!(owner.db.try_lock().is_ok());
    fixture
        .json("/__synthetic/control", json!({"release":true}))
        .await;
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| s.entry(&entry.operation.operation_id))
            .unwrap()
            .unwrap()
            .state,
        OutboxState::Pending
    );
    let report = transport
        .run(&owner, &auth, Budget::default())
        .await
        .unwrap();
    assert_eq!(report.committed, 1);
    let stats = fixture.json("/__synthetic/control", json!({})).await;
    assert_eq!(stats["uploadHashes"][0], stats["uploadHashes"][1]);
}

#[tokio::test]
#[ignore = "requires explicit synthetic local Worker bundle; no live issuer"]
async fn actual_worker_rejects_redirect_bounds_and_changed_native_authority() {
    let fixture = Fixture::start();
    let auth = fixture.enroll().await;
    let temp = tempfile::TempDir::new().unwrap();
    let owner = owner(&temp, &auth.scope);
    let transport = Transport::build(&fixture.origin, Some(fixture.cert.clone())).unwrap();
    fixture
        .json("/__synthetic/control", json!({"redirect":true}))
        .await;
    assert!(matches!(
        transport.run(&owner, &auth, Budget::default()).await,
        Err(TransportError::Protocol)
    ));
    fixture
        .json("/__synthetic/control", json!({"oversized":true}))
        .await;
    assert!(matches!(
        transport.run(&owner, &auth, Budget::default()).await,
        Err(TransportError::Quota)
    ));
    assert_eq!(
        owner.with_store(&auth.scope, |s| s.cursors()).unwrap(),
        CursorState::default()
    );
    let entry = enqueue(&owner, &auth);
    assert!(matches!(
        transport
            .run(
                &owner,
                &auth,
                Budget {
                    transfer_bytes: 1,
                    ..Budget::default()
                }
            )
            .await,
        Err(TransportError::Quota)
    ));
    fixture
        .json("/__synthetic/control", json!({"stall":true}))
        .await;
    let run = transport.run(&owner, &auth, Budget::default());
    tokio::pin!(run);
    let checks = async {
        for _ in 0..100 {
            if fixture.json("/__synthetic/control", json!({})).await["waiting"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        auth.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        fixture
            .json("/__synthetic/control", json!({"release":true}))
            .await;
    };
    let (result, ()) = tokio::join!(&mut run, checks);
    assert!(matches!(result, Err(TransportError::Authority)));
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| s.entry(&entry.operation.operation_id))
            .unwrap()
            .unwrap()
            .state,
        OutboxState::Pending
    );
    let report = transport
        .run(&owner, &auth, Budget::default())
        .await
        .unwrap();
    assert_eq!(report.committed, 1);
}

#[test]
fn transport_diagnostics_discard_custom_owner_error_text() {
    let error = SyncError::Database(rusqlite::Error::ToSqlConversionFailure(Box::new(
        std::io::Error::other("synthetic-private-owner-content"),
    )));
    let transport = TransportError::from(error);
    assert!(matches!(
        transport,
        TransportError::Local(LocalFailure::Persistence)
    ));
    assert!(!format!("{transport:?} {transport}").contains("synthetic-private-owner-content"));
}
