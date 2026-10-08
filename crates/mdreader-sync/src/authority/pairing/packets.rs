use super::*;
use serde::{Deserialize, Serialize};

/// Public OOB packet. Import must be separately confirmed by the receiving user;
/// accepting a packet from a server roster is never a trusted-channel operation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairRequestPacket {
    pub(super) v: u8,
    pub(super) kind: String,
    pub(super) account: String,
    pub(super) library_id: String,
    pub(super) pair_id: String,
    pub(super) signing_jwk: PublicJwk,
    pub(super) wrapping_jwk: PublicJwk,
    pub(super) signing_thumb: String,
    pub(super) wrapping_thumb: String,
    pub(super) nonce: String,
    pub(super) expires: u64,
}
impl PairRequestPacket {
    /// Public JSON for an explicit separate-channel copy/QR step; contains no secrets.
    pub fn out_of_band_text(&self) -> AuthorityResult<String> {
        let text = serde_json::to_string(self).map_err(|_| AuthorityError::Scope)?;
        if text.len() > MAX_PACKET {
            return Err(AuthorityError::Bounds);
        }
        Ok(text)
    }
    pub(super) fn parse(text: &str, account: &str, library: &str) -> AuthorityResult<Self> {
        if text.len() > MAX_PACKET {
            return Err(AuthorityError::Bounds);
        }
        let packet: Self = serde_json::from_str(text).map_err(|_| AuthorityError::Scope)?;
        packet.check_scope(account, library)?;
        Ok(packet)
    }
    pub(super) fn check_scope(&self, account: &str, library: &str) -> AuthorityResult<()> {
        crate::crypto::validate_digest(&self.account).map_err(|_| AuthorityError::Scope)?;
        for id in [&self.library_id, &self.pair_id] {
            crate::crypto::validate_uuid(id).map_err(|_| AuthorityError::Scope)?;
        }
        nonce(&self.nonce)?;
        if self.v != 1
            || self.kind != "mdreader-pair-request"
            || self.account != account
            || self.library_id != library
            || self.signing_jwk.thumbprint()? != self.signing_thumb
            || self.wrapping_jwk.thumbprint()? != self.wrapping_thumb
            || self.signing_thumb == self.wrapping_thumb
            || self.expires == 0
        {
            return Err(AuthorityError::Scope);
        }
        Ok(())
    }
    /// Expired durable packets remain readable; every new user approval requires freshness.
    pub(super) fn check_fresh(&self) -> AuthorityResult<()> {
        let current = now()?;
        if self.expires <= current || self.expires > current.saturating_add(300) {
            return Err(AuthorityError::Enrollment);
        }
        Ok(())
    }
    pub(super) fn context(&self, sender: &str) -> AuthorityResult<PairContext> {
        PairContext::new(
            &self.account,
            &self.library_id,
            sender,
            &self.pair_id,
            &self.signing_thumb,
            &self.wrapping_thumb,
            &self.nonce,
        )
    }
}

/// Sender identity is pinned only from this user-verified OOB packet, never a roster.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairApprovalPacket {
    pub(super) v: u8,
    pub(super) kind: String,
    pub(super) account: String,
    pub(super) library_id: String,
    pub(super) pair_id: String,
    pub(super) sender_jwk: PublicJwk,
    pub(super) sender_thumb: String,
    pub(super) grant_hash: String,
    pub(super) jwe_hash: String,
}
impl PairApprovalPacket {
    /// Public JSON for the reverse separate-channel handoff; contains no secrets.
    pub fn out_of_band_text(&self) -> AuthorityResult<String> {
        let text = serde_json::to_string(self).map_err(|_| AuthorityError::Scope)?;
        if text.len() > MAX_PACKET {
            return Err(AuthorityError::Bounds);
        }
        Ok(text)
    }
    pub(super) fn parse(text: &str, request: &PairRequestPacket) -> AuthorityResult<Self> {
        if text.len() > MAX_PACKET {
            return Err(AuthorityError::Bounds);
        }
        let packet: Self = serde_json::from_str(text).map_err(|_| AuthorityError::Scope)?;
        packet.check(request)?;
        Ok(packet)
    }
    pub(super) fn check(&self, request: &PairRequestPacket) -> AuthorityResult<()> {
        for hash in [&self.grant_hash, &self.jwe_hash] {
            crate::crypto::validate_digest(hash).map_err(|_| AuthorityError::Scope)?;
        }
        if self.v != 1
            || self.kind != "mdreader-pair-approval"
            || self.account != request.account
            || self.library_id != request.library_id
            || self.pair_id != request.pair_id
            || self.sender_jwk.thumbprint()? != self.sender_thumb
            || self.sender_thumb == request.signing_thumb
        {
            return Err(AuthorityError::Scope);
        }
        Ok(())
    }
}
