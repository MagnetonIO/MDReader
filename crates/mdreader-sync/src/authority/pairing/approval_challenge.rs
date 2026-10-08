//! Opaque approval challenges are inspected only after exact pinned HTTPS fetch.
use super::*;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Challenge {
    v: u8,
    kind: String,
    account: String,
    library_id: String,
    pair_id: String,
    signing_thumb: String,
    wrapping_thumb: String,
    pair_nonce: String,
    sender_thumb: String,
    jwe_hash: String,
    pub(super) nonce: String,
    iss: String,
    aud: String,
    iat: u64,
    pub(super) exp: u64,
    pub(super) jti: String,
}
impl Challenge {
    pub(super) fn parse(
        token: &str,
        request: &PairRequestPacket,
        pin: &PairApprovalPacket,
        audience: Option<&str>,
        fresh: bool,
    ) -> AuthorityResult<Self> {
        if token.len() >= 4096 {
            return Err(AuthorityError::Bounds);
        }
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthorityError::Custody);
        }
        let decode =
            |part: &str| crate::crypto::decode_canonical(part).map_err(|_| AuthorityError::Custody);
        let header: Header =
            serde_json::from_slice(&decode(parts[0])?).map_err(|_| AuthorityError::Custody)?;
        let c: Self =
            serde_json::from_slice(&decode(parts[1])?).map_err(|_| AuthorityError::Custody)?;
        crate::crypto::validate_uuid(&c.jti).map_err(|_| AuthorityError::Scope)?;
        nonce(&c.nonce)?;
        if header.alg != "HS256"
            || header.typ != "mdreader-enrollment+jwt"
            || decode(parts[2])?.len() != 32
            || c.v != 2
            || c.kind != "approve"
            || c.iss != "mdreader-sync"
            || c.account != request.account
            || c.library_id != request.library_id
            || c.pair_id != request.pair_id
            || c.signing_thumb != request.signing_thumb
            || c.wrapping_thumb != request.wrapping_thumb
            || c.pair_nonce != request.nonce
            || c.sender_thumb != pin.sender_thumb
            || c.jwe_hash != pin.jwe_hash
            || c.exp <= c.iat
            || c.exp - c.iat > 300
            || audience.is_some_and(|aud| c.aud != aud)
        {
            return Err(AuthorityError::Scope);
        }
        if fresh
            && (c.iat > now()?.saturating_add(5)
                || c.exp <= now()?
                || c.exp > now()?.saturating_add(300))
        {
            return Err(AuthorityError::Enrollment);
        }
        Ok(c)
    }
}
