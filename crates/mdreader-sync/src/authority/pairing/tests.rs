use super::*;
use crate::authority::{
    custody::{test_support::MemoryVault, EnrollmentState, Lifecycle, Phase},
    tests::{registration, Issuer},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};

#[test]
fn browser_pair_fixture_requires_exact_out_of_band_pin_and_context() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/pairing-v1.json")).unwrap();
    let request = PairRequestPacket::parse(
        &fixture["request"].to_string(),
        fixture["request"]["account"].as_str().unwrap(),
        fixture["request"]["libraryId"].as_str().unwrap(),
    )
    .unwrap();
    let pin = PairApprovalPacket::parse(&fixture["approvalPacket"].to_string(), &request).unwrap();
    let grant = fixture["grant"].as_str().unwrap();
    let jwe = fixture["jwe"].as_str().unwrap();
    grant::Grant::verify(grant, &request, &pin, jwe, false).unwrap();
    let raw =
        crate::crypto::decode_canonical(fixture["recipientPrivateJwk"]["d"].as_str().unwrap())
            .unwrap();
    let key = wrapping::WrappingKey::from_bytes(Zeroizing::new(raw.try_into().unwrap())).unwrap();
    let plain =
        wrapping::decrypt_pair(jwe, &key, &request.context(&pin.sender_thumb).unwrap()).unwrap();
    assert_eq!(
        material::parse(plain.as_bytes(), &request).unwrap().len(),
        1
    );
    let mut wrong_pin = pin.clone();
    wrong_pin.sender_jwk = request.signing_jwk.clone();
    wrong_pin.sender_thumb = request.signing_thumb.clone();
    assert!(grant::Grant::verify(grant, &request, &wrong_pin, jwe, false).is_err());
    let mut wrong_hash = pin.clone();
    wrong_hash.grant_hash = "0".repeat(64);
    assert!(grant::Grant::verify(grant, &request, &wrong_hash, jwe, false).is_err());
    let mut wrong_context = request.clone();
    wrong_context.nonce = URL_SAFE_NO_PAD.encode([93; 32]);
    assert!(wrapping::decrypt_pair(
        jwe,
        &key,
        &wrong_context.context(&pin.sender_thumb).unwrap()
    )
    .is_err());
    let duplicate = fixture["request"]
        .to_string()
        .replacen("\"v\":1", "\"v\":1,\"v\":1", 1);
    assert!(PairRequestPacket::parse(&duplicate, &request.account, &request.library_id).is_err());
    let duplicate_jwk = fixture["request"].to_string().replacen(
        "\"kty\":\"EC\"",
        "\"kty\":\"EC\",\"kty\":\"EC\"",
        1,
    );
    assert!(
        PairRequestPacket::parse(&duplicate_jwk, &request.account, &request.library_id).is_err()
    );
}

fn pair_intent(record: &CustodyRecord) -> enrollment::Intent {
    pair_intent_at(record, now().unwrap())
}
pub(super) fn pair_intent_at(record: &CustodyRecord, current: u64) -> enrollment::Intent {
    let (signing, wrapping, _) = enrollment::bootstrap::public_material(record).unwrap();
    let thumb = |jwk: &Value| {
        PublicJwk::parse(&serde_json::to_vec(jwk).unwrap())
            .unwrap()
            .thumbprint()
            .unwrap()
    };
    let nonce = URL_SAFE_NO_PAD.encode([39; 32]);
    let claims = json!({"v":2,"kind":"pair","libraryId":record.scope.library_id(),"signingThumb":thumb(&signing),"wrappingThumb":thumb(&wrapping),"recoveryThumb":null,"nonce":nonce,"iss":"mdreader-sync","aud":"https://mdreader.dev/sync","iat":current,"exp":current+300,"jti":uuid::Uuid::new_v4().to_string()});
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.typ = Some("mdreader-enrollment+jwt".into());
    let challenge = jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"synthetic-only-pair-server-secret"),
    )
    .unwrap();
    enrollment::Intent {
        progress: Default::default(),
        kind: enrollment::Kind::Pair,
        recovery_grant: None,
        body: request_body(record, &challenge).unwrap(),
        challenge,
        nonce,
        expires: current + 300,
    }
}

#[test]
fn paired_keys_require_exact_durable_delivery_generation_transition() {
    let issuer = Issuer::new();
    let identity = issuer.identity();
    let reg = registration("https://sync.example.test");
    let library = uuid::Uuid::new_v4().to_string();
    let mut sender = PendingCustody::prepare(&identity, &reg, &library)
        .unwrap()
        .record;
    sender.confirm_bootstrap().unwrap();
    let candidate =
        PendingCustody::candidate(&identity, &reg, &library, CandidateKind::Pair).unwrap();
    let memory = MemoryVault::new();
    memory.create(&candidate.record).unwrap();
    let mut staged = candidate.record.copy();
    staged.intent = Some(pair_intent(&staged));
    staged.generation += 1;
    memory.compare_exchange(1, &staged).unwrap();
    let request = candidate::own_request(&staged).unwrap();
    let signer = signing::SigningKey::load(&sender.pkcs8).unwrap();
    let jwe = wrapping::encrypt_pair(
        &material::export(&sender).unwrap(),
        &request.wrapping_jwk,
        &request.context(&signer.device).unwrap(),
    )
    .unwrap();
    let mut packet = PairApprovalPacket {
        v: 1,
        kind: "mdreader-pair-approval".into(),
        account: request.account.clone(),
        library_id: library,
        pair_id: request.pair_id.clone(),
        sender_jwk: PublicJwk::parse(&serde_json::to_vec(&signer.jwk).unwrap()).unwrap(),
        sender_thumb: signer.device.clone(),
        grant_hash: String::new(),
        jwe_hash: crate::blob_digest(jwe.as_bytes()),
    };
    let current = now().unwrap();
    let grant = signer
        .pair_grant(&grant::Grant {
            v: 1,
            account: request.account.clone(),
            library_id: request.library_id.clone(),
            pair_id: request.pair_id.clone(),
            signing_thumb: request.signing_thumb.clone(),
            wrapping_thumb: request.wrapping_thumb.clone(),
            pair_nonce: request.nonce.clone(),
            sender_thumb: signer.device.clone(),
            jwe_hash: packet.jwe_hash.clone(),
            approval_nonce: URL_SAFE_NO_PAD.encode([41; 32]),
            challenge_id: uuid::Uuid::new_v4().to_string(),
            iat: current,
            exp: current + 300,
            jti: uuid::Uuid::new_v4().to_string(),
        })
        .unwrap();
    packet.grant_hash = crate::blob_digest(grant.as_bytes());
    staged.pairing = Some(Slot::Receiving {
        request,
        packet,
        grant,
        jwe,
    });
    staged.generation += 1;
    memory.compare_exchange(2, &staged).unwrap();
    let reopened = memory.get(&staged.reference).unwrap().unwrap();
    assert!(reopened.keys.is_empty());
    let mut activated = reopened.copy();
    activated.keys = sender.keys.clone();
    activated.state = EnrollmentState::Confirmed;
    let Lifecycle::V2 { phase, .. } = &mut activated.lifecycle else {
        panic!("synthetic v2");
    };
    *phase = Phase::Enrolled;
    activated.pairing = None;
    activated.generation += 1;
    activated.confirm_intent().unwrap();
    let mut substituted = activated.copy();
    substituted.keys.values_mut().next().unwrap()[0] ^= 1;
    assert!(memory.compare_exchange(3, &substituted).is_err());
    assert!(memory
        .get(&staged.reference)
        .unwrap()
        .unwrap()
        .keys
        .is_empty());
    memory.compare_exchange(3, &activated).unwrap();
    assert!(memory.compare_exchange(3, &activated).is_err());
    let reopened = memory.get(&staged.reference).unwrap().unwrap();
    assert_eq!(reopened.keys, sender.keys);
    assert!(matches!(
        reopened.lifecycle,
        Lifecycle::V2 {
            recovery: None,
            phase: Phase::Enrolled,
            ..
        }
    ));
}
