//! Synthetic custody serialization/CAS tests. Worker cases separately use real SQLite.
use super::*;
use crate::authority::{
    custody::test_support::MemoryVault,
    tests::{registration, Issuer},
};
use renewal_state::{Renewal, RenewalPhase};
#[test]
fn replacement_proposal_reopens_without_replacing_old_identity_and_fences_cas() {
    let issuer = Issuer::new();
    let identity = issuer.identity();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let pending =
        PendingCustody::candidate(&identity, &reg, &library, CandidateKind::Pair).unwrap();
    let memory = MemoryVault::new();
    memory.create(&pending.record).unwrap();
    let mut old = pending.record.copy();
    old.intent = Some(tests::pair_intent_at(&old, now().unwrap() - 301));
    old.generation += 1;
    memory.compare_exchange(1, &old).unwrap();
    let original = candidate::own_request(&old).unwrap();
    let mut next = tests::pair_intent_at(&old, now().unwrap());
    next.renew_from(old.intent.as_ref().unwrap()).unwrap();
    let mut staged = old.copy();
    staged.pair_renewal = Some(Renewal::new(&old, next).unwrap());
    staged.generation += 1;
    memory.compare_exchange(2, &staged).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&custody::encode(&staged).unwrap()).unwrap()
            ["v"],
        3
    );
    let reopened = memory.get(&old.reference).unwrap().unwrap();
    assert!(candidate::own_request(&reopened).unwrap() == original);
    assert!(reopened.keys.is_empty());
    assert!(reopened.pkcs8 == old.pkcs8);
    assert!(reopened.lifecycle.same_keys(&old.lifecycle));
    let mut uncertain = reopened.copy();
    uncertain.pair_renewal.as_mut().unwrap().phase = RenewalPhase::Uncertain;
    uncertain.generation += 1;
    memory.compare_exchange(3, &uncertain).unwrap();
    let mut altered = uncertain.copy();
    altered.pair_renewal.as_mut().unwrap().operation_id = uuid::Uuid::new_v4().to_string();
    altered.generation += 1;
    assert!(memory.compare_exchange(4, &altered).is_err());
    let mut installed = uncertain.copy();
    installed.intent = Some(uncertain.pair_renewal.as_ref().unwrap().next.dispatched());
    installed.pair_renewal = None;
    installed.generation += 1;
    assert!(memory.compare_exchange(3, &installed).is_err());
    memory.compare_exchange(4, &installed).unwrap();
    let reopened = memory.get(&old.reference).unwrap().unwrap();
    assert!(candidate::own_request(&reopened).unwrap() != original);
    assert!(reopened.pkcs8 == old.pkcs8 && reopened.lifecycle.same_keys(&old.lifecycle));
    assert!(
        reopened
            .intent
            .as_ref()
            .unwrap()
            .owner_summary()
            .unwrap()
            .uncertain
    );
    assert!(reopened.keys.is_empty() && reopened.scope == old.scope);
    let wire: serde_json::Value =
        serde_json::from_slice(&custody::encode(&reopened).unwrap()).unwrap();
    assert_eq!(
        wire["intent"]["progress"]["history"][0]["challenge_id"],
        original.pair_id
    );
}
#[test]
fn renewal_clear_does_not_reset_library_or_permit_key_replacement() {
    let issuer = Issuer::new();
    let identity = issuer.identity();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let mut original = PendingCustody::candidate(&identity, &reg, &library, CandidateKind::Pair)
        .unwrap()
        .record;
    original.intent = Some(tests::pair_intent_at(&original, now().unwrap() - 301));
    let mut next = tests::pair_intent_at(&original, now().unwrap());
    next.renew_from(original.intent.as_ref().unwrap()).unwrap();
    original.pair_renewal = Some(Renewal::new(&original, next).unwrap());
    let memory = MemoryVault::new();
    memory.create(&original).unwrap();
    let mut cleared = original.copy();
    cleared.pair_renewal = None;
    cleared.generation += 1;
    let mut reset = cleared.copy();
    reset.scope = crate::SyncScope::new(
        original.scope.account_digest().into(),
        uuid::Uuid::new_v4().to_string(),
        original.scope.device_id().into(),
    )
    .unwrap();
    assert!(memory.compare_exchange(1, &reset).is_err());
    memory.compare_exchange(1, &cleared).unwrap();
    let held = memory.get(&original.reference).unwrap().unwrap();
    assert!(
        held.intent == original.intent
            && held.pkcs8 == original.pkcs8
            && held.scope == original.scope
    );
    assert!(held.pair_renewal.is_none());
}
