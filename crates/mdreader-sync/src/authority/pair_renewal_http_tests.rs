//! Actual native authority + local HTTPS OIDC + production SQLite Worker.
//! Custody uses serialized synthetic MemoryVault slots, never a private profile.
use super::*;
use pairing::PairRenewalPreparation;
use pairing::PairRenewalResult;

async fn candidate_request(
    fixture: &Fixture,
    memory: Arc<MemoryVault>,
    library: &str,
) -> (NativeAuthorityFactory, SyncCredentialReference, String) {
    let factory = fixture.factory(memory, SessionController::new());
    let discovery = fixture.login(&factory, None).await;
    let candidate = factory
        .prepare_candidate(&discovery, library, CandidateKind::Pair)
        .unwrap();
    let reference = candidate.reference();
    factory.persist_pending(&candidate).await.unwrap();
    fixture
        .json("/__synthetic/control", json!({"shortPairChallenge":true}))
        .await;
    let binding = factory
        .prepare_pair_login(&discovery, &reference, library)
        .await
        .unwrap();
    drop(discovery);
    let login = fixture.login(&factory, Some(&binding)).await;
    let packet = factory
        .request_pair(&login, &reference, library)
        .await
        .unwrap()
        .out_of_band_text()
        .unwrap();
    (factory, reference, packet)
}

async fn expire(record: &CustodyRecord) {
    let expires = record.intent.as_ref().unwrap().expires;
    let remaining = expires.saturating_sub(now().unwrap());
    assert!(
        remaining <= 8,
        "only bounded fixture challenges may expire here"
    );
    tokio::time::sleep(Duration::from_secs(remaining + 1)).await;
    assert!(now().unwrap() > expires);
}

fn unchanged_candidate(old: &CustodyRecord, current: &CustodyRecord) {
    assert_eq!(old.reference, current.reference);
    assert_eq!(old.scope, current.scope);
    assert!(old.pkcs8 == current.pkcs8);
    assert!(old.lifecycle.same_keys(&current.lifecycle));
    assert!(current.keys.is_empty());
    assert!(current.pairing.is_none());
}

async fn renewal_login(
    fixture: &Fixture,
    factory: &NativeAuthorityFactory,
    reference: &SyncCredentialReference,
    library: &str,
) -> VerifiedLogin {
    let discovery = fixture.login(factory, None).await;
    let binding = match factory
        .prepare_pair_renewal(&discovery, reference, library)
        .await
        .unwrap()
    {
        PairRenewalPreparation::Login(binding) => binding,
        PairRenewalPreparation::Complete(_) => panic!("new proposal requires nonce login"),
    };
    drop(discovery);
    fixture.login(factory, Some(&binding)).await
}

#[tokio::test]
#[ignore = "explicit local HTTPS synthetic OIDC code+PKCE and production Worker fixture only"]
async fn actual_native_expired_pair_renewal_lost_reply_reopen_and_original_approval() {
    let mut fixture = Fixture::start();
    let memory = Arc::new(MemoryVault::new());
    let approver_factory = fixture.factory(memory.clone(), SessionController::new());
    let library = uuid::Uuid::new_v4().to_string();
    let discovery = fixture.login(&approver_factory, None).await;
    let pending = approver_factory
        .prepare_pending(&discovery, &library)
        .unwrap();
    let approver_reference = pending.reference();
    approver_factory.persist_pending(&pending).await.unwrap();
    let binding = approver_factory
        .prepare_bootstrap(&discovery, &approver_reference, &library)
        .await
        .unwrap();
    drop(discovery);
    let login = fixture.login(&approver_factory, Some(&binding)).await;
    let approver = approver_factory
        .bootstrap(login, &approver_reference, &library)
        .await
        .unwrap();
    let original_library = memory.get(&approver_reference).unwrap().unwrap();

    let (factory, reference, original_packet) =
        candidate_request(&fixture, memory.clone(), &library).await;
    let original = memory.get(&reference).unwrap().unwrap();
    expire(&original).await;
    let login = renewal_login(&fixture, &factory, &reference, &library).await;
    let prepared = memory.get(&reference).unwrap().unwrap();
    unchanged_candidate(&original, &prepared);
    assert_eq!(
        prepared.intent.as_ref().unwrap().body,
        original.intent.as_ref().unwrap().body
    );
    let proposal_body = prepared.pair_renewal.as_ref().unwrap().body.clone();
    fixture
        .json("/__synthetic/control", json!({"dropPairRenewal":true}))
        .await;
    assert!(matches!(
        factory
            .request_pair_renewal(&login, &reference, &library)
            .await,
        Err(AuthorityError::Interrupted)
    ));
    drop(login);
    let uncertain = memory.get(&reference).unwrap().unwrap();
    unchanged_candidate(&original, &uncertain);
    assert_eq!(uncertain.pair_renewal.as_ref().unwrap().body, proposal_body);
    assert_eq!(
        uncertain.intent.as_ref().unwrap().body,
        original.intent.as_ref().unwrap().body
    );
    factory.controller.logout().unwrap();
    drop(factory);

    // Reopen serialized custody with a new controller and ordinary actual login.
    // The committed SQLite receipt installs its exact request without a new POST.
    let reopened = fixture.factory(memory.clone(), SessionController::new());
    let login = fixture.login(&reopened, None).await;
    let next_packet = match reopened
        .prepare_pair_renewal(&login, &reference, &library)
        .await
        .unwrap()
    {
        PairRenewalPreparation::Complete(PairRenewalResult::Request(packet)) => {
            packet.out_of_band_text().unwrap()
        }
        _ => panic!("lost committed reply must reconcile, never prepare another request"),
    };
    assert_ne!(next_packet, original_packet);
    let installed = memory.get(&reference).unwrap().unwrap();
    unchanged_candidate(&original, &installed);
    assert!(installed.pair_renewal.is_none());
    let old_packet: Value = serde_json::from_str(&original_packet).unwrap();
    let next: Value = serde_json::from_str(&next_packet).unwrap();
    assert_ne!(old_packet["pairId"], next["pairId"]);
    for field in [
        "account",
        "libraryId",
        "signingJwk",
        "wrappingJwk",
        "signingThumb",
        "wrappingThumb",
    ] {
        assert_eq!(old_packet[field], next[field]);
    }
    assert_eq!(
        reopened
            .pair_request_packet(&login, &reference, &library)
            .await
            .unwrap()
            .out_of_band_text()
            .unwrap(),
        next_packet
    );
    drop(login);
    let approval = approver_factory
        .approve_pair(&approver, &next_packet, true)
        .await
        .unwrap()
        .out_of_band_text()
        .unwrap();
    approver_factory
        .finish_pair_handoff(&approver, true)
        .await
        .unwrap();
    let login = fixture.login(&reopened, None).await;
    let enrolled = reopened
        .accept_pair(login, &reference, &library, &approval, true)
        .await
        .unwrap();
    assert_eq!(enrolled.record.keys, approver.record.keys);
    assert!(enrolled.record.pkcs8 == original.pkcs8);
    assert!(enrolled.record.lifecycle.same_keys(&original.lifecycle));
    reopened.controller.logout().unwrap();

    // Approval already won before expiry. Restart retains the original request;
    // only a separately trusted original packet may turn keyless custody enrolled.
    let (factory, reference, original_packet) =
        candidate_request(&fixture, memory.clone(), &library).await;
    let original = memory.get(&reference).unwrap().unwrap();
    let approval = approver_factory
        .approve_pair(&approver, &original_packet, true)
        .await
        .unwrap()
        .out_of_band_text()
        .unwrap();
    approver_factory
        .finish_pair_handoff(&approver, true)
        .await
        .unwrap();
    expire(&original).await;
    let login = renewal_login(&fixture, &factory, &reference, &library).await;
    assert!(matches!(
        factory
            .request_pair_renewal(&login, &reference, &library)
            .await
            .unwrap(),
        PairRenewalResult::ApprovalRequired
    ));
    drop(login);
    let retained = memory.get(&reference).unwrap().unwrap();
    unchanged_candidate(&original, &retained);
    assert_eq!(
        retained.intent.as_ref().unwrap().body,
        original.intent.as_ref().unwrap().body
    );
    factory.controller.logout().unwrap();
    drop(factory);
    let reopened = fixture.factory(memory.clone(), SessionController::new());
    let login = fixture.login(&reopened, None).await;
    assert!(matches!(
        reopened
            .prepare_pair_renewal(&login, &reference, &library)
            .await
            .unwrap(),
        PairRenewalPreparation::Complete(PairRenewalResult::ApprovalRequired)
    ));
    drop(login);
    let login = fixture.login(&reopened, None).await;
    assert!(matches!(
        reopened
            .accept_pair(login, &reference, &library, &approval, false)
            .await,
        Err(AuthorityError::LifecycleGated)
    ));
    unchanged_candidate(&original, &memory.get(&reference).unwrap().unwrap());
    let mut altered: Value = serde_json::from_str(&approval).unwrap();
    altered["grantHash"] = json!("0".repeat(64));
    let login = fixture.login(&reopened, None).await;
    assert!(matches!(
        reopened
            .accept_pair(login, &reference, &library, &altered.to_string(), true)
            .await,
        Err(AuthorityError::Scope)
    ));
    unchanged_candidate(&original, &memory.get(&reference).unwrap().unwrap());
    let login = fixture.login(&reopened, None).await;
    let enrolled = reopened
        .accept_pair(login, &reference, &library, &approval, true)
        .await
        .unwrap();
    assert_eq!(enrolled.record.keys, approver.record.keys);
    assert!(enrolled.record.pkcs8 == original.pkcs8);
    assert!(enrolled.record.lifecycle.same_keys(&original.lifecycle));
    reopened.controller.logout().unwrap();
    let current_library = memory.get(&approver_reference).unwrap().unwrap();
    assert!(current_library.pkcs8 == original_library.pkcs8);
    assert_eq!(current_library.keys, original_library.keys);
    assert_eq!(current_library.scope, original_library.scope);
    let stats = fixture.json("/__synthetic/control", json!({})).await;
    assert_eq!(stats["pairRenewalHashes"].as_array().unwrap().len(), 2);
    assert_eq!(stats["pairReconcileHashes"].as_array().unwrap().len(), 1);
    assert_eq!(
        stats["pairRenewalHashes"][0],
        json!(crate::blob_digest(proposal_body.as_bytes()))
    );
    assert_eq!(stats["oauthExchanges"], 16);
    assert_eq!(stats["pkceRejected"], 0);
    approver_factory.controller.logout().unwrap();
    fixture.stop_normally().await;
}
