use super::*;
use custody::{Lifecycle, Phase};
use enrollment::challenge::Claims;
use serde::{Deserialize, Serialize};
use signing::SigningKey;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Grant {
    pub(super) v: u8,
    pub(super) account: String,
    pub(super) library_id: String,
    pub(super) signing_thumb: String,
    pub(super) wrapping_thumb: String,
    pub(super) nonce: String,
    pub(super) challenge_id: String,
    pub(super) iat: u64,
    pub(super) exp: u64,
    pub(super) jti: String,
}
fn held_recovery(record: &CustodyRecord) -> AuthorityResult<Zeroizing<Vec<u8>>> {
    if let Some(bytes) = &record.pending_recovery {
        return Ok(bundle::parse(
            bytes,
            record.scope.account_digest(),
            record.scope.library_id(),
        )?
        .recovery);
    }
    if let Lifecycle::V2 {
        recovery: Some(raw),
        phase: Phase::Enrolled,
        ..
    } = &record.lifecycle
    {
        return Ok(raw.clone());
    }
    Err(AuthorityError::LifecycleGated)
}
pub(in super::super) fn request_body(
    record: &CustodyRecord,
    challenge: &str,
    grant: &str,
) -> AuthorityResult<String> {
    if grant.len() >= 4096 {
        return Err(AuthorityError::Bounds);
    }
    let c = Claims::parse(challenge)?;
    c.check_record(record)?;
    let parts: Vec<_> = grant.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthorityError::Custody);
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Header {
        alg: String,
        typ: String,
    }
    let header: Header = serde_json::from_slice(
        &crate::crypto::decode_canonical(parts[0]).map_err(|_| AuthorityError::Custody)?,
    )
    .map_err(|_| AuthorityError::Custody)?;
    if header.alg != "ES256" || header.typ != "mdreader-recovery-grant+jwt" {
        return Err(AuthorityError::Custody);
    }
    let signing = SigningKey::load(&held_recovery(record)?)?;
    let jwk = serde_json::from_value(signing.jwk).map_err(|_| AuthorityError::Custody)?;
    let key = jsonwebtoken::DecodingKey::from_jwk(&jwk).map_err(|_| AuthorityError::Custody)?;
    let mut policy = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
    // Durable old intent must remain readable after expiry; network use separately checks freshness.
    policy.validate_exp = false;
    policy.validate_aud = false;
    policy.required_spec_claims.clear();
    let value: Grant = jsonwebtoken::decode::<Grant>(grant, &key, &policy)
        .map_err(|_| AuthorityError::Custody)?
        .claims;
    let (_, wrapping, _) = enrollment::bootstrap::public_material(record)?;
    let thumb = wrapping::PublicJwk::parse(
        &serde_json::to_vec(&wrapping).map_err(|_| AuthorityError::Custody)?,
    )?
    .thumbprint()?;
    crate::crypto::validate_uuid(&value.jti).map_err(|_| AuthorityError::Custody)?;
    if value.v != 1
        || value.account != record.scope.account_digest()
        || value.library_id != record.scope.library_id()
        || value.signing_thumb != record.scope.device_id()
        || value.wrapping_thumb != thumb
        || value.nonce != c.nonce
        || value.challenge_id != c.jti
        || value.exp > c.exp
        || value.exp <= value.iat
        || value.exp - value.iat > 300
    {
        return Err(AuthorityError::Custody);
    }
    let (signing, wrapping, _) = enrollment::bootstrap::public_material(record)?;
    serde_json::to_string(&serde_json::json!({"challenge":challenge,"signingJwk":signing,"wrappingJwk":wrapping,"recoveryGrant":grant}))
        .map_err(|_|AuthorityError::Custody)
}

pub(in super::super) fn renew_intent(
    record: &CustodyRecord,
    response: enrollment::challenge::Response,
) -> AuthorityResult<enrollment::Intent> {
    let c = Claims::parse(&response.challenge)?;
    let (_, wrapping, _) = enrollment::bootstrap::public_material(record)?;
    let thumb = wrapping::PublicJwk::parse(
        &serde_json::to_vec(&wrapping).map_err(|_| AuthorityError::Custody)?,
    )?
    .thumbprint()?;
    let current = now()?;
    let grant = SigningKey::load(&held_recovery(record)?)?.recovery_grant(&Grant {
        v: 1,
        account: record.scope.account_digest().into(),
        library_id: record.scope.library_id().into(),
        signing_thumb: record.scope.device_id().into(),
        wrapping_thumb: thumb,
        nonce: c.nonce,
        challenge_id: c.jti,
        iat: current,
        exp: c.exp.min(current + 300),
        jti: uuid::Uuid::new_v4().to_string(),
    })?;
    Ok(enrollment::Intent {
        progress: Default::default(),
        kind: enrollment::Kind::Recover,
        body: request_body(record, &response.challenge, &grant)?,
        recovery_grant: Some(grant),
        challenge: response.challenge,
        nonce: response.nonce,
        expires: response.expires,
    })
}
