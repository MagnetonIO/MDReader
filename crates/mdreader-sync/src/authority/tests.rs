use super::*;
use super::{custody::test_support::MemoryVault, signing::SigningKey};
use crate::{
    transport::{Authority, Owner, TransportError, TransportResult},
    *,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde_json::{json, Value};
use sha2::Digest;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
const ISSUER: &str = "https://native-synthetic-issuer.example.test";
const API_AUDIENCE: &str = "https://mdreader.dev/sync";
const REDIRECT: &str = "http://127.0.0.1:43871/mdreader-sync/callback";
pub(super) static LOGIN_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub(super) fn registration(origin: &str) -> ReviewedOidcRegistration {
    ReviewedOidcRegistration::new(
        ISSUER,
        "native-synthetic",
        OidcEndpoints {
            authorization: format!("{origin}/__synthetic/oauth/authorize"),
            token: format!("{origin}/__synthetic/oauth/token"),
            jwks: format!("{origin}/__synthetic/keys"),
        },
        origin,
        API_AUDIENCE,
    )
    .unwrap()
}
pub(super) struct Issuer {
    key: EncodingKey,
    jwk: Value,
}
impl Issuer {
    pub(super) fn new() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let point = key.public_key().as_ref();
        Self {
            key: EncodingKey::from_ec_der(pkcs8.as_ref()),
            jwk: json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(&point[1..33]),"y":URL_SAFE_NO_PAD.encode(&point[33..]),"kid":"synthetic","alg":"ES256","use":"sig"}),
        }
    }
    fn claims() -> Value {
        let now = super::now().unwrap();
        json!({"iss":ISSUER,"sub":"synthetic-alice","aud":"native-synthetic","iat":now,"exp":now+300,"nonce":"synthetic-nonce"})
    }
    fn token(&self, claims: &Value) -> Zeroizing<String> {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("synthetic".into());
        Zeroizing::new(jsonwebtoken::encode(&header, claims, &self.key).unwrap())
    }
    fn access_claims() -> Value {
        let now = super::now().unwrap();
        json!({"iss":ISSUER,"sub":"synthetic-alice","aud":API_AUDIENCE,"client_id":"native-synthetic","scope":"sync:read sync:write","iat":now,"exp":now+300,"jti":uuid::Uuid::new_v4().to_string()})
    }
    fn access_token(&self, claims: &Value) -> Zeroizing<String> {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/support/public-test-rsa.json")).unwrap();
        let key = EncodingKey::from_rsa_pem(fixture["pem"].as_str().unwrap().as_bytes()).unwrap();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("synthetic-access".into());
        header.typ = Some("at+jwt".into());
        Zeroizing::new(jsonwebtoken::encode(&header, claims, &key).unwrap())
    }
    fn jwks(&self) -> Vec<u8> {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/support/public-test-rsa.json")).unwrap();
        serde_json::to_vec(&json!({"keys":[self.jwk, fixture["jwk"]]})).unwrap()
    }
    pub(super) fn identity(&self) -> VerifiedIdentity {
        access::verify_pair(
            &registration("https://sync.example.test"),
            self.token(&Self::claims()),
            self.access_token(&Self::access_claims()),
            &self.jwks(),
            "synthetic-nonce",
            super::now().unwrap(),
            super::now().unwrap() + 300,
        )
        .unwrap()
    }
}
#[test]
fn verified_identity_requires_signature_issuer_audience_nonce_and_fresh_time() {
    let issuer = Issuer::new();
    let reg = registration("https://sync.example.test");
    let claims = Issuer::claims();
    let now = super::now().unwrap();
    let verified = identity::verify(
        &reg,
        issuer.token(&claims),
        &issuer.jwks(),
        "synthetic-nonce",
        now,
    )
    .unwrap();
    assert_eq!(
        verified.account,
        blob_digest(br#"["https://native-synthetic-issuer.example.test","synthetic-alice"]"#)
    );
    for (field, value) in [
        ("iss", json!("https://evil.example.test")),
        ("aud", json!("other-client")),
        ("nonce", json!("wrong")),
        ("exp", json!(now - 1)),
        ("iat", json!(now + 60)),
        ("iat", json!(now - 600)),
        ("azp", json!("other-client")),
        ("aud", json!(["native-synthetic", "other-client"])),
    ] {
        let mut bad = claims.clone();
        bad[field] = value;
        assert!(
            identity::verify(
                &reg,
                issuer.token(&bad),
                &issuer.jwks(),
                "synthetic-nonce",
                now
            )
            .is_err(),
            "{field}"
        );
    }
    let other = Issuer::new();
    assert!(identity::verify(
        &reg,
        other.token(&claims),
        &issuer.jwks(),
        "synthetic-nonce",
        now
    )
    .is_err());
    let mut duplicate: Value = serde_json::from_slice(&issuer.jwks()).unwrap();
    duplicate["keys"]
        .as_array_mut()
        .unwrap()
        .push(issuer.jwk.clone());
    assert!(identity::verify(
        &reg,
        issuer.token(&claims),
        &serde_json::to_vec(&duplicate).unwrap(),
        "synthetic-nonce",
        now
    )
    .is_err());
    let mut multi = claims;
    multi["aud"] = json!(["native-synthetic", "other-client"]);
    multi["azp"] = json!("native-synthetic");
    identity::verify(
        &reg,
        issuer.token(&multi),
        &issuer.jwks(),
        "synthetic-nonce",
        now,
    )
    .unwrap();
}
#[derive(Default)]
struct CaptureBrowser {
    url: Mutex<Option<Zeroizing<String>>>,
}
impl NativeBrowser for CaptureBrowser {
    fn open<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AuthorityResult<()>> + Send + 'a>> {
        Box::pin(async move {
            *self.url.lock().unwrap() = Some(Zeroizing::new(url.into()));
            Ok(())
        })
    }
}
#[tokio::test]
async fn native_login_uses_fresh_pkce_and_consumes_invalid_callback_without_network() {
    let _serial = LOGIN_TEST.lock().await;
    let client = OidcClient::new(registration("https://sync.example.test")).unwrap();
    let browser = CaptureBrowser::default();
    let pending = client.begin(REDIRECT).unwrap();
    assert!(matches!(client.begin(REDIRECT), Err(AuthorityError::Busy)));
    pending.open(&browser).await.unwrap();
    assert!(pending.open(&browser).await.is_err());
    let url = url::Url::parse(browser.url.lock().unwrap().as_ref().unwrap()).unwrap();
    let query: BTreeMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(query["scope"], "openid sync:read sync:write");
    assert_eq!(query["audience"], API_AUDIENCE);
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["nonce"].len(), 43);
    assert_eq!(query["state"].len(), 43);
    assert_eq!(query["code_challenge"].len(), 43);
    assert!(!query.contains_key("client_secret"));
    assert!(matches!(
        client
            .complete(
                pending,
                "/mdreader-sync/callback?state=wrong&code=synthetic-code"
            )
            .await,
        Err(AuthorityError::Identity)
    ));
    let second = client.begin(REDIRECT).unwrap();
    let second_browser = CaptureBrowser::default();
    second.open(&second_browser).await.unwrap();
    let second_url = url::Url::parse(second_browser.url.lock().unwrap().as_ref().unwrap()).unwrap();
    let second_state = second_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    assert_ne!(query["state"], second_state);
    let callback =
        format!("/mdreader-sync/callback?state={second_state}&state={second_state}&code=x");
    assert!(matches!(
        client.complete(second, &callback).await,
        Err(AuthorityError::Identity)
    ));
    assert!(client
        .begin("http://localhost:43871/mdreader-sync/callback")
        .is_err());
}
#[tokio::test]
async fn pending_key_custody_reconciles_timeout_and_reopens_same_material() {
    let issuer = Issuer::new();
    let identity = issuer.identity();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let pending = PendingCustody::prepare(&identity, &reg, &library).unwrap();
    let reference = pending.reference();
    let memory = Arc::new(MemoryVault::new());
    let vault = SyncVaultClient::new(memory.clone()).unwrap();
    memory
        .delay
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        vault
            .create(&pending, Instant::now() + Duration::from_millis(10))
            .await
            .unwrap_err(),
        AuthorityError::Interrupted
    );
    memory
        .delay
        .store(false, std::sync::atomic::Ordering::SeqCst);
    // A preservation FIFO barrier must wait for the actual OS mutation even
    // after its original receiver timed out. It never infers mutation success.
    vault
        .settle(Instant::now() + Duration::from_secs(2))
        .await
        .unwrap();
    assert!(memory
        .records
        .lock()
        .unwrap()
        .contains_key(reference.opaque_reference()));
    let restored = vault
        .get(&reference, Instant::now() + Duration::from_secs(2))
        .await
        .unwrap()
        .unwrap();
    assert!(restored.same_material(&pending.record));
    assert_eq!(restored.generation, 1);
    vault
        .create(&pending, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    let reopened = memory.get(&reference).unwrap().unwrap();
    assert!(reopened.same_material(&restored));
    assert!(reopened.keys == restored.keys);
    let factory = NativeAuthorityFactory::new(
        OidcClient::new(registration("https://sync.example.test")).unwrap(),
        vault.clone(),
        SessionController::new(),
    );
    let summary = factory
        .custody_summary(&reference, &library)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summary.lifecycle, CustodyLifecycle::BootstrapPending);
    assert_eq!(summary.enrollment, None);
    assert_eq!(summary.pair_approval, PairApprovalSummary::None);
    assert_eq!(
        factory
            .custody_summary(&reference, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap_err(),
        AuthorityError::Scope
    );
    let absent =
        SyncCredentialReference::parse(&format!("native-sync-v1-{}", uuid::Uuid::new_v4()))
            .unwrap();
    assert!(factory
        .custody_summary(&absent, &library)
        .await
        .unwrap()
        .is_none());
    let before = memory.records.lock().unwrap()[reference.opaque_reference()].clone();
    let mut bytes: Value = serde_json::from_slice(&before).unwrap();
    bytes["scope"]["device_id"] = json!(URL_SAFE_NO_PAD.encode([8; 32]));
    memory.records.lock().unwrap().insert(
        reference.opaque_reference().into(),
        Zeroizing::new(serde_json::to_vec(&bytes).unwrap()),
    );
    assert!(vault
        .get(&reference, Instant::now() + Duration::from_secs(1))
        .await
        .is_err());
    memory
        .records
        .lock()
        .unwrap()
        .insert(reference.opaque_reference().into(), before);
    let restored = memory.get(&reference).unwrap().unwrap();
    assert_eq!(
        restored
            .check_identity(&identity, &reg, &uuid::Uuid::new_v4().to_string())
            .unwrap_err(),
        AuthorityError::Scope
    );
    let other_registration = ReviewedOidcRegistration::new(
        ISSUER,
        "different-native-client",
        OidcEndpoints {
            authorization: "https://sync.example.test/auth".into(),
            token: "https://sync.example.test/token".into(),
            jwks: "https://sync.example.test/keys".into(),
        },
        "https://sync.example.test",
        API_AUDIENCE,
    )
    .unwrap();
    assert!(restored
        .check_identity(&identity, &other_registration, &library)
        .is_err());
}

#[test]
fn legacy_bytes_are_lossless_and_candidates_never_invent_library_authority() {
    let issuer = Issuer::new();
    let identity = issuer.identity();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let pending = PendingCustody::prepare(&identity, &reg, &library).unwrap();
    let encoded = custody::encode(&pending.record).unwrap();
    let mut old: Value = serde_json::from_slice(&encoded).unwrap();
    old["v"] = json!(1);
    for field in ["wrapping_private", "recovery_private", "phase"] {
        old.as_object_mut().unwrap().remove(field);
    }
    let original = Zeroizing::new(
        format!(" \n{}\n ", serde_json::to_string_pretty(&old).unwrap()).into_bytes(),
    );
    let legacy = custody::decode(&pending.reference(), original.clone()).unwrap();
    assert_eq!(
        custody::encode(&legacy).unwrap().as_slice(),
        original.as_slice()
    );
    assert_eq!(
        legacy.can_resume().unwrap_err(),
        AuthorityError::LifecycleGated
    );
    let mut changed = legacy.copy();
    changed.generation += 1;
    assert_eq!(
        custody::encode(&changed).unwrap_err(),
        AuthorityError::Generation
    );
    for kind in [CandidateKind::Pair, CandidateKind::Recover] {
        let mut candidate = PendingCustody::candidate(&identity, &reg, &library, kind).unwrap();
        assert!(candidate.record.keys.is_empty());
        let bytes = custody::encode(&candidate.record).unwrap();
        let restored = custody::decode(&candidate.reference(), bytes).unwrap();
        assert!(restored.same_material(&candidate.record));
        assert_eq!(
            restored.can_resume().unwrap_err(),
            AuthorityError::LifecycleGated
        );
        assert_eq!(
            candidate.record.confirm_bootstrap().unwrap_err(),
            AuthorityError::LifecycleGated
        );
        candidate.record.keys.insert(
            uuid::Uuid::new_v4().to_string(),
            Zeroizing::new(vec![7; 32]),
        );
        assert!(custody::encode(&candidate.record).is_err());
    }
}

#[test]
fn v2_custody_reopens_persistent_anchors_and_cas_preserves_uncertain_material() {
    let issuer = Issuer::new();
    let identity = issuer.identity();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let pending = PendingCustody::prepare(&identity, &reg, &library).unwrap();
    let reference = pending.reference();
    let memory = MemoryVault::new();
    memory.create(&pending.record).unwrap();
    let before = memory.records.lock().unwrap()[reference.opaque_reference()].clone();
    let mut reopened = memory.get(&reference).unwrap().unwrap();
    assert!(reopened.same_material(&pending.record));
    assert_eq!(
        reopened.test_public_material(),
        pending.record.test_public_material()
    );
    reopened
        .check_recovery_anchor(&reopened.test_public_material().1)
        .unwrap();
    assert!(reopened.check_recovery_anchor(&Issuer::new().jwk).is_err());
    reopened.generation += 1;
    reopened.confirm_bootstrap().unwrap();
    assert_eq!(
        memory.compare_exchange(9, &reopened).unwrap_err(),
        AuthorityError::Generation
    );
    assert_eq!(
        memory.records.lock().unwrap()[reference.opaque_reference()].as_slice(),
        before.as_slice()
    );
    memory.compare_exchange(1, &reopened).unwrap();
    let confirmed = memory.get(&reference).unwrap().unwrap();
    assert!(confirmed.same_material(&reopened));
    confirmed.can_resume().unwrap();
    assert_eq!(
        confirmed.test_public_material(),
        pending.record.test_public_material()
    );
    assert!(memory.compare_exchange(1, &reopened).is_err());
}
#[test]
fn private_records_and_identities_never_diagnose_key_or_token_content() {
    let issuer = Issuer::new();
    let identity = issuer.identity();
    let pending = PendingCustody::prepare(
        &identity,
        &registration("https://sync.example.test"),
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
    let formatted = format!(
        "{identity:?} {pending:?} {:?} {:?}",
        pending.reference(),
        pending.record
    );
    assert!(!formatted.contains(identity.token.as_str()));
    assert!(!formatted.contains(&URL_SAFE_NO_PAD.encode(pending.record.pkcs8.as_slice())));
    assert!(!formatted.contains(pending.reference().opaque_reference()));
    assert!(formatted.contains("redacted"));
    SyncKeyringVault::new("com.mdreader.synthetic-test").unwrap(); // Construction has no OS I/O.
}
#[tokio::test]
async fn logout_during_login_rejects_late_completion_before_any_network() {
    let _serial = LOGIN_TEST.lock().await;
    let controller = SessionController::new();
    let factory = NativeAuthorityFactory::new(
        OidcClient::new(registration("https://sync.example.test")).unwrap(),
        SyncVaultClient::new(Arc::new(MemoryVault::new())).unwrap(),
        controller.clone(),
    );
    let login = factory.begin_login(REDIRECT).unwrap();
    controller.logout().unwrap();
    assert!(matches!(
        factory
            .verify_login(login, "/mdreader-sync/callback?state=wrong&code=x")
            .await,
        Err(AuthorityError::Inactive)
    ));
    assert!(controller.active_scope_barrier().read().unwrap().is_none());
}
struct Fixture {
    child: Child,
    _temp: tempfile::TempDir,
    origin: String,
    cert: reqwest::Certificate,
}
struct LoopbackBrowser(reqwest::Client);
impl NativeBrowser for LoopbackBrowser {
    fn open<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AuthorityResult<()>> + Send + 'a>> {
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;
            let response = self
                .0
                .get(url)
                .send()
                .await
                .map_err(|_| AuthorityError::Interrupted)?;
            if response.status().as_u16() != 302 {
                return Err(AuthorityError::Identity);
            }
            let redirect = url::Url::parse(
                response
                    .headers()
                    .get("location")
                    .ok_or(AuthorityError::Identity)?
                    .to_str()
                    .map_err(|_| AuthorityError::Identity)?,
            )
            .map_err(|_| AuthorityError::Identity)?;
            if redirect.host_str() != Some("127.0.0.1") || redirect.scheme() != "http" {
                return Err(AuthorityError::Identity);
            }
            let port = redirect.port().ok_or(AuthorityError::Identity)?;
            let mut stream = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .map_err(|_| AuthorityError::Interrupted)?;
            let request = Zeroizing::new(format!(
                "GET {}?{} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n",
                redirect.path(),
                redirect.query().ok_or(AuthorityError::Identity)?
            ));
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|_| AuthorityError::Interrupted)?;
            // Real OS browser launch returns immediately; callback server responds separately.
            Ok(())
        })
    }
}
impl Fixture {
    fn start() -> Self {
        let web = std::env::var("MDREADER_SYNC_WEB_WORKTREE")
            .expect("explicit synthetic Worker worktree required");
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
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let value: Value = serde_json::from_str(&line).expect("synthetic bridge must start");
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
    fn http(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(self.cert.clone())
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }
    async fn json(&self, path: &str, body: Value) -> Value {
        let response = self
            .http()
            .post(format!("{}{path}", self.origin))
            .body(serde_json::to_vec(&body).unwrap())
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
    }
    fn factory(
        &self,
        memory: Arc<MemoryVault>,
        controller: Arc<SessionController>,
    ) -> NativeAuthorityFactory {
        NativeAuthorityFactory::new(
            OidcClient::test_with_root(registration(&self.origin), self.cert.clone()).unwrap(),
            SyncVaultClient::new(memory).unwrap(),
            controller,
        )
    }
    async fn login(
        &self,
        factory: &NativeAuthorityFactory,
        enrollment: Option<&EnrollmentLogin>,
    ) -> VerifiedLogin {
        let socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let redirect = format!(
            "http://127.0.0.1:{}/mdreader-sync/callback",
            socket.local_addr().unwrap().port()
        );
        drop(socket);
        let browser = LoopbackBrowser(self.http());
        let deadline = Instant::now() + Duration::from_secs(10);
        match enrollment {
            Some(binding) => factory
                .login_enrollment_desktop(binding, &redirect, &browser, deadline)
                .await
                .unwrap(),
            None => factory
                .login_desktop(&redirect, &browser, deadline)
                .await
                .unwrap(),
        }
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
struct TestOwner {
    db: Mutex<rusqlite::Connection>,
    binding: Arc<RwLock<Option<SyncScope>>>,
}
impl Owner for TestOwner {
    fn with_store<T>(
        &self,
        expected: &SyncScope,
        action: impl FnOnce(&mut SyncStore<'_>) -> Result<T>,
    ) -> TransportResult<T> {
        let binding = self.binding.read().unwrap();
        if binding.as_ref() != Some(expected) {
            return Err(TransportError::Authority);
        }
        let mut db = self.db.lock().unwrap();
        Ok(action(&mut SyncStore::attach(&mut db, expected.clone())?)?)
    }
}
#[tokio::test]
#[ignore = "explicit local HTTPS synthetic OIDC code+PKCE and production Worker fixture only"]
async fn actual_native_oidc_custody_lost_bootstrap_reconnect_transport_and_revocation() {
    let fixture = Fixture::start();
    let memory = Arc::new(MemoryVault::new());
    let controller = SessionController::new();
    let factory = fixture.factory(memory.clone(), controller.clone());
    let library = uuid::Uuid::new_v4().to_string();
    let discovery = fixture.login(&factory, None).await;
    let pending = factory.prepare_pending(&discovery, &library).unwrap();
    let reference = pending.reference();
    let local = tempfile::TempDir::new().unwrap();
    let path = local.path().join("synthetic-owner.sqlite");
    let mut db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;CREATE TABLE native_sync_reference(value TEXT);").unwrap();
    migrate(&mut db).unwrap();
    db.execute(
        "INSERT INTO native_sync_reference VALUES(?)",
        [reference.opaque_reference()],
    )
    .unwrap();
    factory.persist_pending(&pending).await.unwrap();
    let signing = SigningKey::load(&pending.record.pkcs8).unwrap();
    let binding = factory
        .prepare_bootstrap(&discovery, &reference, &library)
        .await
        .unwrap();
    let exact_bootstrap = memory
        .get(&reference)
        .unwrap()
        .unwrap()
        .intent
        .as_ref()
        .unwrap()
        .body
        .clone();
    drop(discovery);
    let login = fixture.login(&factory, Some(&binding)).await;
    fixture
        .json("/__synthetic/control", json!({"dropBootstrap":true}))
        .await;
    assert!(matches!(
        factory.bootstrap(login, &reference, &library).await,
        Err(AuthorityError::Interrupted)
    ));
    let uncertain = memory.get(&reference).unwrap().unwrap();
    assert_eq!(uncertain.intent.as_ref().unwrap().body, exact_bootstrap);
    let wire: Value = serde_json::from_slice(&custody::encode(&uncertain).unwrap()).unwrap();
    assert_eq!(wire["intent"]["progress"]["state"], json!("uncertain"));
    assert!(controller.active_scope_barrier().read().unwrap().is_none());
    let login = fixture.login(&factory, None).await;
    let active = factory.resume(login, &reference, &library).await.unwrap();
    let scope = active.scope().unwrap();
    let owner = TestOwner {
        db: Mutex::new(db),
        binding: controller.active_scope_barrier(),
    };
    let persisted = memory.get(&reference).unwrap().unwrap();
    let (key_id, raw) = persisted.keys.iter().next().unwrap();
    let key = LibraryKey::from_bytes(key_id.clone(), raw.clone()).unwrap();
    let context = EncryptionContext {
        library_id: library.clone(),
        item_id: uuid::Uuid::new_v4().to_string(),
        revision_id: uuid::Uuid::new_v4().to_string(),
        key_id: key_id.clone(),
    };
    owner
        .with_store(&scope, |s| {
            s.seal_enqueue(
                &uuid::Uuid::new_v4().to_string(),
                &[],
                &key,
                &context,
                b"synthetic authority root",
            )
        })
        .unwrap();
    let transport =
        crate::transport::Transport::test_with_root(&fixture.origin, fixture.cert.clone()).unwrap();
    let report = active
        .run(&transport, &owner, crate::transport::Budget::default())
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
    assert!(active
        .authorize(
            "GET",
            "https://evil.example.test/v1/changes?after=0&limit=1",
            &[]
        )
        .is_err());
    assert!(active
        .authorize(
            "POST",
            &format!("{}/v1/enrollment/bootstrap", fixture.origin),
            b"{}"
        )
        .is_err());
    controller.logout().unwrap();
    assert!(active.scope().is_err());
    drop(owner);
    drop(factory);
    let restarted_controller = SessionController::new();
    let restarted = fixture.factory(memory.clone(), restarted_controller.clone());
    assert!(restarted_controller
        .active_scope_barrier()
        .read()
        .unwrap()
        .is_none());
    let db = rusqlite::Connection::open(&path).unwrap();
    let opaque: String = db
        .query_row("SELECT value FROM native_sync_reference", [], |r| r.get(0))
        .unwrap();
    let stored_reference = SyncCredentialReference::parse(&opaque).unwrap();
    let before = memory.get(&stored_reference).unwrap().unwrap();
    assert!(before.same_material(&persisted));
    let login = fixture.login(&restarted, None).await;
    let resumed = restarted
        .resume(login, &stored_reference, &library)
        .await
        .unwrap();
    assert_eq!(resumed.scope().unwrap(), scope);
    assert!(active.scope().is_err());
    let owner = TestOwner {
        db: Mutex::new(db),
        binding: restarted_controller.active_scope_barrier(),
    };
    let report = transport
        .run(&owner, &resumed, crate::transport::Budget::default())
        .await
        .unwrap();
    assert_eq!(
        (
            report.committed,
            report.received,
            report.applied,
            report.acknowledged
        ),
        (0, 1, 0, 0)
    );
    // A second native device imports the exact browser-compatible encrypted file.
    let export = resumed.export_recovery(true).unwrap();
    let recovery_controller = SessionController::new();
    let recovery_factory = fixture.factory(memory.clone(), recovery_controller.clone());
    let recovery_login = fixture.login(&recovery_factory, None).await;
    let recovery_candidate = recovery_factory
        .prepare_candidate(&recovery_login, &library, CandidateKind::Recover)
        .unwrap();
    let recovery_reference = recovery_candidate.reference();
    recovery_factory
        .persist_pending(&recovery_candidate)
        .await
        .unwrap();
    let imported = recovery_factory
        .prepare_recovery(
            &recovery_login,
            &recovery_reference,
            &library,
            export.file_bytes(),
            export.export_secret(),
        )
        .await
        .unwrap();
    let binding = recovery_factory
        .prepare_recovery_enrollment(&recovery_login, &imported, true)
        .await
        .unwrap();
    assert!(memory
        .get(&recovery_reference)
        .unwrap()
        .unwrap()
        .keys
        .is_empty());
    drop(imported);
    drop(recovery_login);
    let login = fixture.login(&recovery_factory, Some(&binding)).await;
    fixture
        .json("/__synthetic/control", json!({"dropRecover":true}))
        .await;
    assert!(matches!(
        recovery_factory
            .recover(login, &recovery_reference, &library, true)
            .await,
        Err(AuthorityError::Interrupted)
    ));
    assert!(memory
        .get(&recovery_reference)
        .unwrap()
        .unwrap()
        .keys
        .is_empty());
    drop(recovery_factory);
    let recovered_factory = fixture.factory(memory.clone(), SessionController::new());
    let login = fixture.login(&recovered_factory, None).await;
    let recovered = recovered_factory
        .reconcile_recovery(login, &recovery_reference, &library, true)
        .await
        .unwrap();
    assert_eq!(
        memory.get(&recovery_reference).unwrap().unwrap().keys,
        persisted.keys
    );
    assert_eq!(
        recovered.scope().unwrap().account_digest(),
        scope.account_digest()
    );
    assert_eq!(recovered.scope().unwrap().library_id(), scope.library_id());
    pairing_http::exercise(
        &fixture,
        memory.clone(),
        &recovered_factory,
        &recovered,
        &library,
    )
    .await;
    drop(recovered);
    let verified = fixture.login(&restarted, None).await;
    let url = factory_url(&fixture.origin, &scope);
    let auth = signing
        .authorize(&verified.identity, "DELETE", &url, &[])
        .unwrap();
    let headers = auth.into_headers();
    let revoked = fixture
        .http()
        .delete(url)
        .header("authorization", headers.0)
        .header("mdreader-proof", headers.1)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status().as_u16(), 200);
    assert!(matches!(
        transport
            .run(&owner, &resumed, crate::transport::Budget::default())
            .await,
        Err(TransportError::Forbidden)
    ));
    assert!(restarted_controller
        .active_scope_barrier()
        .read()
        .unwrap()
        .is_none());
    assert!(resumed.scope().is_err());
    assert!(memory.get(&stored_reference).unwrap().is_some());
    let login = fixture.login(&restarted, None).await;
    assert!(matches!(
        restarted.resume(login, &stored_reference, &library).await,
        Err(AuthorityError::Enrollment)
    ));
    // Model a restored uncertain pre-confirmation vault record for the now-revoked
    // same device. 403 cannot prove never-enrolled; explicit retry must stay pending.
    let confirmed = memory.get(&stored_reference).unwrap().unwrap();
    let mut restored: Value =
        serde_json::from_slice(&custody::encode(&confirmed).unwrap()).unwrap();
    restored["enrollment"] = json!("pending");
    restored["phase"] = json!({"state":"bootstrap_pending"});
    let old_challenge = restored["intent"]["challenge"].as_str().unwrap();
    let mut claims: Value = serde_json::from_slice(
        &crate::crypto::decode_canonical(old_challenge.split('.').nth(1).unwrap()).unwrap(),
    )
    .unwrap();
    claims["iat"] = json!(now().unwrap() - 305);
    claims["exp"] = json!(now().unwrap() - 5);
    let mut header = Header::new(Algorithm::HS256);
    header.typ = Some("mdreader-enrollment+jwt".into());
    let expired = jsonwebtoken::encode(
        &header,
        &claims,
        &EncodingKey::from_secret(b"synthetic-only-server-secret-at-least-32-bytes"),
    )
    .unwrap();
    restored["intent"]["challenge"] = json!(expired);
    restored["intent"]["expires"] = claims["exp"].clone();
    let mut body: Value =
        serde_json::from_str(restored["intent"]["body"].as_str().unwrap()).unwrap();
    body["challenge"] = json!(expired);
    let prior_body = serde_json::to_string(&body).unwrap();
    restored["intent"]["body"] = json!(prior_body);
    restored["intent"]["progress"]["state"] = json!("uncertain");
    restored["intent"]["progress"]["receipt"] = Value::Null;
    memory.records.lock().unwrap().insert(
        stored_reference.0.clone(),
        Zeroizing::new(serde_json::to_vec(&restored).unwrap()),
    );
    let login = fixture.login(&restarted, None).await;
    let EnrollmentProgress::Login(binding) = restarted
        .renew_enrollment(login, &stored_reference, &library, true)
        .await
        .unwrap()
    else {
        panic!("revoked device must not activate")
    };
    let renewed = memory.get(&stored_reference).unwrap().unwrap();
    assert_eq!(renewed.keys, persisted.keys);
    let receipt: Value = serde_json::from_slice(&custody::encode(&renewed).unwrap()).unwrap();
    assert_eq!(
        receipt["intent"]["progress"]["history"][0]["digest"],
        json!(blob_digest(prior_body.as_bytes()))
    );
    assert_eq!(
        receipt["intent"]["progress"]["history"][0]["uncertain"],
        json!(true)
    );
    let login = fixture.login(&restarted, Some(&binding)).await;
    assert!(restarted
        .bootstrap(login, &stored_reference, &library)
        .await
        .is_err());
    let refused = memory.get(&stored_reference).unwrap().unwrap();
    assert!(refused.state == custody::EnrollmentState::Pending);
    assert_eq!(refused.keys, persisted.keys);
    assert!(restarted_controller
        .active_scope_barrier()
        .read()
        .unwrap()
        .is_none());
    let stats = fixture.json("/__synthetic/control", json!({})).await;
    assert_eq!(stats["oauthExchanges"], json!(18));
    assert_eq!(stats["pkceRejected"], json!(0));
}
fn factory_url(origin: &str, scope: &SyncScope) -> String {
    format!("{origin}/v1/devices/{}", scope.device_id())
}

// Test-only construction deliberately cannot be reached through production activation.
fn synthetic_capability(
    issuer: &Issuer,
    controller: Arc<SessionController>,
    record: CustodyRecord,
) -> NativeAuthority {
    let identity = issuer.identity();
    let registration = Arc::new(registration("https://sync.example.test"));
    let signer = SigningKey::load(&record.pkcs8).unwrap();
    let lease = controller
        .activate(controller.epoch().unwrap(), record.scope.clone())
        .unwrap();
    NativeAuthority {
        record,
        signer,
        identity,
        registration,
        controller,
        lease,
    }
}
#[test]
fn late_401_and_drop_of_old_capability_preserve_same_scope_replacement() {
    let issuer = Issuer::new();
    let controller = SessionController::new();
    let mut record = PendingCustody::prepare(
        &issuer.identity(),
        &registration("https://sync.example.test"),
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap()
    .record;
    record.confirm_bootstrap().unwrap();
    let old = synthetic_capability(&issuer, controller.clone(), record.copy());
    let old_generation = old.generation().unwrap();
    let replacement = synthetic_capability(&issuer, controller.clone(), record);
    let replacement_generation = replacement.generation().unwrap();
    assert!(replacement_generation > old_generation);
    old.rejected(crate::transport::AuthorityRejection::Authentication);
    assert!(old.scope().is_err());
    assert!(replacement.scope().is_ok());
    drop(old);
    assert_eq!(replacement.generation().unwrap(), replacement_generation);
    assert!(controller.active_scope_barrier().read().unwrap().is_some());
    replacement.rejected(crate::transport::AuthorityRejection::Forbidden);
    assert!(replacement.scope().is_err());
    assert!(controller.active_scope_barrier().read().unwrap().is_none());
}
#[test]
fn expiry_and_capability_drop_clear_owner_binding_without_erasing_keys() {
    let issuer = Issuer::new();
    let controller = SessionController::new();
    let mut record = PendingCustody::prepare(
        &issuer.identity(),
        &registration("https://sync.example.test"),
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap()
    .record;
    record.confirm_bootstrap().unwrap();
    let mut active = synthetic_capability(&issuer, controller.clone(), record.copy());
    active.identity.expires = super::now().unwrap();
    assert!(active.scope().is_err());
    assert!(controller.active_scope_barrier().read().unwrap().is_none());
    assert!(!active.record.keys.is_empty());
    let fresh = synthetic_capability(&issuer, controller.clone(), record);
    assert!(fresh.scope().is_ok());
    drop(fresh);
    assert!(controller.active_scope_barrier().read().unwrap().is_none());
}
#[test]
fn signing_policy_requires_exact_origin_query_envelope_hash_and_scope() {
    let issuer = Issuer::new();
    let controller = SessionController::new();
    let mut record = PendingCustody::prepare(
        &issuer.identity(),
        &registration("https://sync.example.test"),
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap()
    .record;
    record.confirm_bootstrap().unwrap();
    let active = synthetic_capability(&issuer, controller, record);
    let key_id = active.record.keys.keys().next().unwrap();
    let key = LibraryKey::from_bytes(key_id.clone(), active.record.keys[key_id].clone()).unwrap();
    let context = EncryptionContext {
        library_id: active.record.scope.library_id().into(),
        item_id: uuid::Uuid::new_v4().to_string(),
        revision_id: uuid::Uuid::new_v4().to_string(),
        key_id: key_id.clone(),
    };
    let prepared = encrypt(&key, &context, b"synthetic content").unwrap();
    let operation = uuid::Uuid::new_v4();
    let url = format!(
        "https://sync.example.test/v1/blobs/{}?operationId={operation}&libraryId={}",
        prepared.blob_id(),
        context.library_id
    );
    assert!(active
        .authorize("PUT", &url, prepared.envelope_utf8())
        .is_ok());
    assert!(active.authorize("PUT", &url, b"{} ").is_err());
    assert!(active
        .authorize(
            "PUT",
            &url.replace(prepared.blob_id(), &"0".repeat(64)),
            prepared.envelope_utf8()
        )
        .is_err());
    for path in [
        "/v1/changes?after=%30&limit=1",
        "/v1/changes?limit=1&after=0",
        "/v1/changes?after=0&limit=101",
        "/v1/changes?after=0&limit=1#fragment",
        "/v1/enrollment/bootstrap",
    ] {
        assert!(active
            .authorize("GET", &format!("https://sync.example.test{path}"), &[])
            .is_err());
    }
    assert!(active
        .authorize(
            "GET",
            "https://sync.example.test/v1/changes?after=0&limit=1",
            &[]
        )
        .is_ok());
    assert!(active
        .authorize(
            "GET",
            "https://other.example.test/v1/changes?after=0&limit=1",
            &[]
        )
        .is_err());
}

#[test]
fn api_access_token_is_bound_to_login_subject_client_resource_and_scopes() {
    let issuer = Issuer::new();
    let reg = registration("https://sync.example.test");
    let now = super::now().unwrap();
    let id = issuer.token(&Issuer::claims());
    let claims = Issuer::access_claims();
    let verify = |token| {
        access::verify_pair(
            &reg,
            id.clone(),
            token,
            &issuer.jwks(),
            "synthetic-nonce",
            now,
            now + 300,
        )
    };
    let access = issuer.access_token(&claims);
    let identity = verify(access.clone()).unwrap();
    assert_eq!(identity.token.as_str(), access.as_str());
    assert_eq!(identity.login_token.as_str(), id.as_str());
    assert!(
        verify(id.clone()).is_err(),
        "ID token cannot authorize the API"
    );
    let mut unsupported = Header::new(Algorithm::ES256);
    unsupported.kid = Some("synthetic".into());
    unsupported.typ = Some("at+jwt".into());
    assert!(
        verify(Zeroizing::new(
            jsonwebtoken::encode(&unsupported, &claims, &issuer.key).unwrap()
        ))
        .is_err(),
        "API signing algorithm is pinned to RS256"
    );
    for (field, value) in [
        ("sub", json!("another-user")),
        ("client_id", json!("other-client")),
        ("aud", json!("native-synthetic")),
        ("scope", json!("sync:read")),
        ("azp", json!("other-client")),
        ("exp", json!(now - 1)),
        ("iat", json!(now - 600)),
        ("jti", json!("")),
    ] {
        let mut bad = claims.clone();
        bad[field] = value;
        assert!(verify(issuer.access_token(&bad)).is_err(), "{field}");
    }
    assert!(access::verify_pair(
        &reg,
        id.clone(),
        access.clone(),
        &issuer.jwks(),
        "synthetic-nonce",
        now,
        now
    )
    .is_err());
    let pending =
        PendingCustody::prepare(&identity, &reg, &uuid::Uuid::new_v4().to_string()).unwrap();
    let signer = SigningKey::load(&pending.record.pkcs8).unwrap();
    let normal = signer
        .authorize(
            &identity,
            "GET",
            "https://sync.example.test/v1/enrollment",
            &[],
        )
        .unwrap()
        .into_headers();
    assert_eq!(
        normal.0.to_str().unwrap(),
        format!("Bearer {}", access.as_str())
    );
    assert!(normal.2.is_none());
    let payload = |header: &reqwest::header::HeaderValue| -> Value {
        serde_json::from_slice(
            &crypto::decode_canonical(header.to_str().unwrap().split('.').nth(1).unwrap()).unwrap(),
        )
        .unwrap()
    };
    let proof = payload(&normal.1);
    assert_eq!(proof["v"], 2);
    assert_eq!(
        proof["ath"],
        URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(access.as_bytes()))
    );
    assert!(proof.get("lth").is_none());
    let candidate = signer
        .authorize(
            &identity,
            "POST",
            "https://sync.example.test/v1/enrollment/bootstrap",
            b"{}",
        )
        .unwrap()
        .into_headers();
    assert_eq!(candidate.2.unwrap().to_str().unwrap(), id.as_str());
    assert_eq!(
        payload(&candidate.1)["lth"],
        URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(id.as_bytes()))
    );
}

#[path = "lifecycle_tests.rs"]
mod lifecycle;
#[path = "pairing_http_tests.rs"]
mod pairing_http;
