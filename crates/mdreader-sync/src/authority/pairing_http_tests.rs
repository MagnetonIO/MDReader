//! Extend the existing single local HTTPS/Worker fixture, never live identity.
use super::*;
pub(super) async fn exercise(
    fixture: &Fixture,
    memory: Arc<MemoryVault>,
    approver_factory: &NativeAuthorityFactory,
    approver: &NativeAuthority,
    library: &str,
) {
    let candidate_controller = SessionController::new();
    let candidate_factory = fixture.factory(memory.clone(), candidate_controller.clone());
    let discovery = fixture.login(&candidate_factory, None).await;
    let candidate = candidate_factory
        .prepare_candidate(&discovery, library, CandidateKind::Pair)
        .unwrap();
    let reference = candidate.reference();
    candidate_factory.persist_pending(&candidate).await.unwrap();
    let binding = candidate_factory
        .prepare_pair_login(&discovery, &reference, library)
        .await
        .unwrap();
    drop(discovery);
    let login = fixture.login(&candidate_factory, Some(&binding)).await;
    let request = candidate_factory
        .request_pair(&login, &reference, library)
        .await
        .unwrap()
        .out_of_band_text()
        .unwrap();
    drop(login);
    assert!(memory.get(&reference).unwrap().unwrap().keys.is_empty());
    assert!(candidate_controller
        .active_scope_barrier()
        .read()
        .unwrap()
        .is_none());
    fixture
        .json("/__synthetic/control", json!({"dropPairApproval":true}))
        .await;
    assert!(matches!(
        approver_factory
            .approve_pair(approver, &request, true)
            .await,
        Err(AuthorityError::Interrupted)
    ));
    let before = memory.get(&approver.record.reference).unwrap().unwrap();
    let wire: Value = serde_json::from_slice(&custody::encode(&before).unwrap()).unwrap();
    assert_eq!(wire["pairing"]["state"], json!("uncertain"));
    // A native restart reopens exact vaulted approval; fresh identity signs only
    // the new request proof. Ciphertext, approval grant and challenge stay equal.
    approver_factory.controller.logout().unwrap();
    let restarted = fixture.factory(memory.clone(), SessionController::new());
    let fresh = fixture.login(&restarted, None).await;
    let active = restarted
        .resume(fresh, &approver.record.reference, library)
        .await
        .unwrap();
    let packet = restarted
        .retry_pair_approval(&active, true)
        .await
        .unwrap()
        .out_of_band_text()
        .unwrap();
    let after = memory.get(&approver.record.reference).unwrap().unwrap();
    let after: Value = serde_json::from_slice(&custody::encode(&after).unwrap()).unwrap();
    assert_eq!(wire["pairing"]["body"], after["pairing"]["body"]);
    let stats = fixture.json("/__synthetic/control", json!({})).await;
    let hashes = stats["pairApprovalHashes"].as_array().unwrap();
    assert_eq!(hashes.len(), 2);
    assert_eq!(hashes[0], hashes[1]);
    restarted.finish_pair_handoff(&active, true).await.unwrap();
    let login = fixture.login(&candidate_factory, None).await;
    assert!(matches!(
        candidate_factory
            .accept_pair(login, &reference, library, &packet, false)
            .await,
        Err(AuthorityError::LifecycleGated)
    ));
    let mut bad: Value = serde_json::from_str(&packet).unwrap();
    bad["grantHash"] = json!("0".repeat(64));
    let login = fixture.login(&candidate_factory, None).await;
    assert!(matches!(
        candidate_factory
            .accept_pair(login, &reference, library, &bad.to_string(), true)
            .await,
        Err(AuthorityError::Scope)
    ));
    assert!(memory.get(&reference).unwrap().unwrap().pairing.is_none());
    let login = fixture.login(&candidate_factory, None).await;
    fixture
        .json("/__synthetic/control", json!({"dropPairEnrollment":true}))
        .await;
    assert!(matches!(
        candidate_factory
            .accept_pair(login, &reference, library, &packet, true)
            .await,
        Err(AuthorityError::Interrupted)
    ));
    let staged = memory.get(&reference).unwrap().unwrap();
    assert!(staged.keys.is_empty());
    assert!(staged.pairing.is_some());
    candidate_controller.logout().unwrap();
    drop(candidate_factory);
    let reopened = fixture.factory(memory.clone(), SessionController::new());
    let login = fixture.login(&reopened, None).await;
    let paired = reopened
        .reconcile_pair(login, &reference, library, true)
        .await
        .unwrap();
    assert_eq!(paired.record.keys, approver.record.keys);
    assert_eq!(paired.scope().unwrap().library_id(), library);
    assert!(matches!(
        paired.export_recovery(true),
        Err(AuthorityError::LifecycleGated)
    ));
    reopened.controller.logout().unwrap();
    assert!(paired.scope().is_err());
}
