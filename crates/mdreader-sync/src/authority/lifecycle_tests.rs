use super::super::{
    custody::{EnrollmentState, Lifecycle, Phase},
    enrollment::{Intent, Kind},
};
use super::*;
fn challenge(record: &CustodyRecord, kind: &str) -> String {
    let (signing, wrapping, recovery) = enrollment::bootstrap::public_material(record).unwrap();
    let thumb = |jwk: &Value| {
        wrapping::PublicJwk::parse(&serde_json::to_vec(jwk).unwrap())
            .unwrap()
            .thumbprint()
            .unwrap()
    };
    let current = now().unwrap();
    let claims = json!({"v":2,"kind":kind,"libraryId":record.scope.library_id(),"signingThumb":thumb(&signing),"wrappingThumb":thumb(&wrapping),"recoveryThumb":if kind=="bootstrap"{json!(thumb(&recovery))}else{Value::Null},"nonce":URL_SAFE_NO_PAD.encode([37;32]),"iss":"mdreader-sync","aud":API_AUDIENCE,"iat":current,"exp":current+300,"jti":uuid::Uuid::new_v4().to_string()});
    let mut header = Header::new(Algorithm::HS256);
    header.typ = Some("mdreader-enrollment+jwt".into());
    jsonwebtoken::encode(
        &header,
        &claims,
        &EncodingKey::from_secret(b"synthetic-challenge-only-32-byte-key"),
    )
    .unwrap()
}
fn attach_bootstrap(record: &mut CustodyRecord) {
    let challenge = challenge(record, "bootstrap");
    let claims = enrollment::challenge::Claims::parse(&challenge).unwrap();
    record.intent = Some(Intent {
        progress: Default::default(),
        kind: Kind::Bootstrap,
        recovery_grant: None,
        body: enrollment::bootstrap::body(record, &challenge).unwrap(),
        challenge,
        nonce: claims.nonce,
        expires: claims.exp,
    });
}
fn attach_recovery(record: &mut CustodyRecord, material: &[u8]) {
    let challenge = challenge(record, "recover");
    let claims = enrollment::challenge::Claims::parse(&challenge).unwrap();
    let bundle: Value = serde_json::from_slice(material).unwrap();
    let d = Zeroizing::new(
        crate::crypto::decode_canonical(bundle["recoveryPrivateJwk"]["d"].as_str().unwrap())
            .unwrap(),
    );
    use p256::pkcs8::EncodePrivateKey;
    let private = p256::SecretKey::from_slice(&d)
        .unwrap()
        .to_pkcs8_der()
        .unwrap();
    let (_, wrapping, _) = enrollment::bootstrap::public_material(record).unwrap();
    let wrapping_thumb = wrapping::PublicJwk::parse(&serde_json::to_vec(&wrapping).unwrap())
        .unwrap()
        .thumbprint()
        .unwrap();
    let iat = now().unwrap();
    let grant=SigningKey::load(private.as_bytes()).unwrap().recovery_grant(&json!({"v":1,"account":record.scope.account_digest(),"libraryId":record.scope.library_id(),"signingThumb":record.scope.device_id(),"wrappingThumb":wrapping_thumb,"nonce":claims.nonce,"challengeId":claims.jti,"iat":iat,"exp":claims.exp,"jti":uuid::Uuid::new_v4().to_string()})).unwrap();
    record.pending_recovery = Some(Zeroizing::new(material.to_vec()));
    record.intent = Some(Intent {
        progress: Default::default(),
        kind: Kind::Recover,
        body: recovery::request_body(record, &challenge, &grant).unwrap(),
        recovery_grant: Some(grant),
        challenge,
        nonce: claims.nonce,
        expires: claims.exp,
    });
}
#[tokio::test]
async fn nonce_bound_intent_reopens_exact_bytes_and_rejects_unbound_or_stale_login() {
    let _serial = LOGIN_TEST.lock().await;
    let issuer = Issuer::new();
    let reg = registration("https://sync.example.test");
    let pending =
        PendingCustody::prepare(&issuer.identity(), &reg, &uuid::Uuid::new_v4().to_string())
            .unwrap();
    let reference = pending.reference();
    let memory = Arc::new(MemoryVault::new());
    memory.create(&pending.record).unwrap();
    let controller = SessionController::new();
    let factory = NativeAuthorityFactory::new(
        OidcClient::new(reg).unwrap(),
        SyncVaultClient::new(memory.clone()).unwrap(),
        controller.clone(),
    );
    let mut record = pending.record.copy();
    attach_bootstrap(&mut record);
    record.generation += 1;
    memory.compare_exchange(1, &record).unwrap();
    let reopened = memory.get(&reference).unwrap().unwrap();
    assert_eq!(
        reopened.intent.as_ref().unwrap().body.as_bytes(),
        record.intent.as_ref().unwrap().body.as_bytes()
    );
    let login = VerifiedLogin {
        identity: issuer.identity(),
        enrollment: None,
        epoch: controller.epoch().unwrap(),
        controller: Arc::downgrade(&controller),
    };
    let binding = factory
        .prepare_bootstrap(&login, &reference, record.scope.library_id())
        .await
        .unwrap();
    assert!(matches!(
        factory
            .bootstrap(login, &reference, record.scope.library_id())
            .await,
        Err(AuthorityError::Identity)
    ));
    let attempt = factory.begin_enrollment_login(&binding, REDIRECT).unwrap();
    let browser = CaptureBrowser::default();
    attempt.open(&browser).await.unwrap();
    let url = browser.url.lock().unwrap().take().unwrap();
    let url = url::Url::parse(&url).unwrap();
    assert_eq!(
        url.query_pairs().find(|(k, _)| k == "nonce").unwrap().1,
        binding_nonce(&record)
    );
    controller.logout().unwrap();
    assert!(matches!(
        factory
            .verify_login(attempt, "/mdreader-sync/callback?state=bad&code=bad")
            .await,
        Err(AuthorityError::Inactive)
    ));
    assert!(factory.begin_enrollment_login(&binding, REDIRECT).is_err());
    let mut substituted = record.copy();
    substituted.intent.as_mut().unwrap().body.push(' ');
    assert!(custody::encode(&substituted).is_err());
    assert!(memory
        .get(&reference)
        .unwrap()
        .unwrap()
        .same_material(&record));
}
fn binding_nonce(record: &CustodyRecord) -> String {
    record.intent.as_ref().unwrap().nonce.clone()
}

#[tokio::test]
async fn explicit_recovery_retains_anchor_and_stages_keyless_until_exact_activation_cas() {
    let issuer = Issuer::new();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let mut original = PendingCustody::prepare(&issuer.identity(), &reg, &library)
        .unwrap()
        .record;
    original.confirm_bootstrap().unwrap();
    let active = synthetic_capability(&issuer, SessionController::new(), original.copy());
    assert!(active.export_recovery(false).is_err());
    let export = active.export_recovery(true).unwrap();
    let candidate =
        PendingCustody::candidate(&issuer.identity(), &reg, &library, CandidateKind::Recover)
            .unwrap();
    let reference = candidate.reference();
    let memory = Arc::new(MemoryVault::new());
    memory.create(&candidate.record).unwrap();
    let controller = SessionController::new();
    let factory = NativeAuthorityFactory::new(
        OidcClient::new(reg).unwrap(),
        SyncVaultClient::new(memory.clone()).unwrap(),
        controller.clone(),
    );
    let login = VerifiedLogin {
        identity: issuer.identity(),
        enrollment: None,
        epoch: 0,
        controller: Arc::downgrade(&controller),
    };
    assert!(factory
        .prepare_recovery(
            &login,
            &reference,
            &library,
            export.file_bytes(),
            Zeroizing::new(URL_SAFE_NO_PAD.encode([1; 32]))
        )
        .await
        .is_err());
    let pending = factory
        .prepare_recovery(
            &login,
            &reference,
            &library,
            export.file_bytes(),
            export.export_secret(),
        )
        .await
        .unwrap();
    assert!(factory
        .prepare_recovery_enrollment(&login, &pending, false)
        .await
        .is_err());
    assert!(memory.get(&reference).unwrap().unwrap().keys.is_empty());
    let file: Value = serde_json::from_slice(export.file_bytes()).unwrap();
    let raw = Zeroizing::new(crate::crypto::decode_canonical(&export.export_secret()).unwrap());
    let secret =
        wrapping::RecoverySecret::from_bytes(Zeroizing::new(raw.as_slice().try_into().unwrap()));
    let plaintext = wrapping::decrypt_recovery(file["jwe"].as_str().unwrap(), &secret).unwrap();
    let mut staged = candidate.record.copy();
    attach_recovery(&mut staged, plaintext.as_bytes());
    staged.generation += 1;
    memory.compare_exchange(1, &staged).unwrap();
    let reopened = memory.get(&reference).unwrap().unwrap();
    assert!(reopened.keys.is_empty());
    assert!(reopened.can_resume().is_err());
    assert!(reopened.pending_recovery.as_ref().unwrap().as_slice() == plaintext.as_bytes());
    let mut activated = reopened.copy();
    let Lifecycle::V2 { wrapping, .. } = &activated.lifecycle else {
        panic!()
    };
    activated.lifecycle = Lifecycle::V2 {
        wrapping: wrapping.clone(),
        recovery: Some(pending.material.recovery.clone()),
        phase: Phase::Enrolled,
    };
    activated.keys = pending.material.keys.clone();
    activated.pending_recovery = None;
    activated.state = EnrollmentState::Confirmed;
    activated.confirm_intent().unwrap();
    activated.generation += 1;
    let mut wrong = activated.copy();
    *wrong.keys.values_mut().next().unwrap() = Zeroizing::new(vec![0; 32]);
    assert!(memory.compare_exchange(2, &wrong).is_err());
    assert!(memory.get(&reference).unwrap().unwrap().keys.is_empty());
    memory.compare_exchange(2, &activated).unwrap();
    let reopened = memory.get(&reference).unwrap().unwrap();
    assert_eq!(reopened.keys, original.keys);
    let Lifecycle::V2 {
        recovery: Some(a), ..
    } = &reopened.lifecycle
    else {
        panic!()
    };
    let Lifecycle::V2 {
        recovery: Some(b), ..
    } = &original.lifecycle
    else {
        panic!()
    };
    assert_eq!(
        SigningKey::load(a).unwrap().device,
        SigningKey::load(b).unwrap().device
    );
    assert!(memory.compare_exchange(2, &activated).is_err());
}

#[tokio::test]
#[ignore = "Explicit native/browser JOSE recovery payload and signing anchor conformance"]
async fn native_recovery_payload_and_anchor_interoperate_with_browser_jose() {
    let issuer = Issuer::new();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let mut original = PendingCustody::prepare(&issuer.identity(), &reg, &library)
        .unwrap()
        .record;
    original.confirm_bootstrap().unwrap();
    let active = synthetic_capability(&issuer, SessionController::new(), original.copy());
    let export = active.export_recovery(true).unwrap();
    let Lifecycle::V2 {
        recovery: Some(private),
        ..
    } = &original.lifecycle
    else {
        panic!()
    };
    let current = now().unwrap();
    let grant=SigningKey::load(private).unwrap().recovery_grant(&json!({"account":original.scope.account_digest(),"libraryId":library,"iat":current,"exp":current+300})).unwrap();
    let request = json!({"notice":"SYNTHETIC PUBLIC TEST KEYS","file":serde_json::from_slice::<Value>(export.file_bytes()).unwrap(),"secret":export.export_secret().as_str(),"grant":grant});
    let mut child = Command::new("node")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/verify-native-recovery.mjs"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "synthetic recovery JOSE conformance failed"
    );
    let candidate =
        PendingCustody::candidate(&issuer.identity(), &reg, &library, CandidateKind::Recover)
            .unwrap();
    let reference = candidate.reference();
    let memory = Arc::new(MemoryVault::new());
    memory.create(&candidate.record).unwrap();
    let controller = SessionController::new();
    let factory = NativeAuthorityFactory::new(
        OidcClient::new(reg).unwrap(),
        SyncVaultClient::new(memory).unwrap(),
        controller.clone(),
    );
    let login = VerifiedLogin {
        identity: issuer.identity(),
        enrollment: None,
        epoch: 0,
        controller: Arc::downgrade(&controller),
    };
    let pending = factory
        .prepare_recovery(
            &login,
            &reference,
            &library,
            &output.stdout,
            export.export_secret(),
        )
        .await
        .unwrap();
    assert_eq!(pending.material.keys, original.keys);
    assert_eq!(
        SigningKey::load(&pending.material.recovery).unwrap().device,
        SigningKey::load(private).unwrap().device
    );
}
