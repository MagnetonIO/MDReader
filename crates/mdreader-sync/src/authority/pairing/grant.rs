use super::*;
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Grant {
    pub v: u8,
    pub account: String,
    pub library_id: String,
    pub pair_id: String,
    pub signing_thumb: String,
    pub wrapping_thumb: String,
    pub pair_nonce: String,
    pub sender_thumb: String,
    pub jwe_hash: String,
    pub approval_nonce: String,
    pub challenge_id: String,
    pub iat: u64,
    pub exp: u64,
    pub jti: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
    jwk: PublicJwk,
}
impl Grant {
    pub(super) fn verify(
        token: &str,
        request: &PairRequestPacket,
        pin: &PairApprovalPacket,
        jwe: &str,
        fresh: bool,
    ) -> AuthorityResult<Self> {
        if token.len() >= 4096 || jwe.len() >= 4096 {
            return Err(AuthorityError::Bounds);
        }
        pin.check(request)?;
        if pin.grant_hash != crate::blob_digest(token.as_bytes())
            || pin.jwe_hash != crate::blob_digest(jwe.as_bytes())
        {
            return Err(AuthorityError::Scope);
        }
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthorityError::Custody);
        }
        let header: Header = serde_json::from_slice(
            &crate::crypto::decode_canonical(parts[0]).map_err(|_| AuthorityError::Custody)?,
        )
        .map_err(|_| AuthorityError::Custody)?;
        if header.alg != "ES256"
            || header.typ != "mdreader-pair-grant+jwt"
            || header.jwk.thumbprint()? != pin.sender_thumb
        {
            return Err(AuthorityError::Scope);
        }
        if crate::crypto::decode_canonical(parts[2])
            .map_err(|_| AuthorityError::Custody)?
            .len()
            != 64
        {
            return Err(AuthorityError::Custody);
        }
        let jwk = serde_json::from_value::<jsonwebtoken::jwk::Jwk>(
            serde_json::to_value(&pin.sender_jwk).map_err(|_| AuthorityError::Custody)?,
        )
        .map_err(|_| AuthorityError::Custody)?;
        let key = jsonwebtoken::DecodingKey::from_jwk(&jwk).map_err(|_| AuthorityError::Custody)?;
        let mut policy = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
        policy.validate_exp = false;
        policy.validate_aud = false;
        policy.required_spec_claims.clear();
        let value = jsonwebtoken::decode::<Self>(token, &key, &policy)
            .map_err(|_| AuthorityError::Custody)?
            .claims;
        value.check(request, pin, fresh)?;
        Ok(value)
    }
    fn check(
        &self,
        request: &PairRequestPacket,
        pin: &PairApprovalPacket,
        fresh: bool,
    ) -> AuthorityResult<()> {
        for id in [&self.jti, &self.challenge_id] {
            crate::crypto::validate_uuid(id).map_err(|_| AuthorityError::Scope)?;
        }
        nonce(&self.approval_nonce)?;
        if self.v != 1
            || self.account != request.account
            || self.library_id != request.library_id
            || self.pair_id != request.pair_id
            || self.signing_thumb != request.signing_thumb
            || self.wrapping_thumb != request.wrapping_thumb
            || self.pair_nonce != request.nonce
            || self.sender_thumb != pin.sender_thumb
            || self.jwe_hash != pin.jwe_hash
            || self.exp <= self.iat
            || self.exp - self.iat > 300
        {
            return Err(AuthorityError::Scope);
        }
        if fresh
            && (self.iat > now()?.saturating_add(5)
                || self.exp <= now()?
                || self.iat.saturating_add(300) < now()?)
        {
            return Err(AuthorityError::Enrollment);
        }
        Ok(())
    }
}
