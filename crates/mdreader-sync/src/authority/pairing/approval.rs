use super::approval_challenge::Challenge;
use super::*;
use serde::{Deserialize, Serialize};
use state::{ApprovalState, Slot};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Body {
    pub challenge: String,
    pub grant: String,
    pub jwe: String,
}
pub(super) fn validate_body(
    body: &Body,
    request: &PairRequestPacket,
    pin: &PairApprovalPacket,
) -> AuthorityResult<()> {
    let c = Challenge::parse(&body.challenge, request, pin, None, false)?;
    let g = grant::Grant::verify(&body.grant, request, pin, &body.jwe, false)?;
    if g.approval_nonce != c.nonce || g.challenge_id != c.jti || g.exp > c.exp {
        return Err(AuthorityError::Scope);
    }
    Ok(())
}
async fn active_record(
    factory: &NativeAuthorityFactory,
    authority: &NativeAuthority,
) -> AuthorityResult<CustodyRecord> {
    authority.check()?;
    if !std::sync::Arc::ptr_eq(&authority.controller, &factory.controller)
        || authority.registration.digest != factory.client.registration.digest
    {
        return Err(AuthorityError::Scope);
    }
    let record = factory
        .vault
        .get(
            &authority.record.reference,
            Instant::now() + Duration::from_secs(5),
        )
        .await?
        .ok_or(AuthorityError::Custody)?;
    authority.check()?;
    record.check_identity(
        &authority.identity,
        &factory.client.registration,
        authority.record.scope.library_id(),
    )?;
    if record.scope != authority.record.scope
        || record.pkcs8 != authority.record.pkcs8
        || record.keys != authority.record.keys
        || record.state != custody::EnrollmentState::Confirmed
        || !record.lifecycle.same_keys(&authority.record.lifecycle)
    {
        return Err(AuthorityError::Generation);
    }
    Ok(record)
}
async fn send(
    factory: &NativeAuthorityFactory,
    authority: &NativeAuthority,
    path: &str,
    body: Vec<u8>,
    limit: usize,
) -> AuthorityResult<Zeroizing<Vec<u8>>> {
    authority.check()?;
    let url = factory
        .client
        .registration
        .sync
        .join(path)
        .map_err(|_| AuthorityError::Configuration)?;
    let auth = authority
        .signer
        .authorize(&authority.identity, "POST", url.as_str(), &body)?;
    let result = factory
        .client
        .http
        .send(
            reqwest::Method::POST,
            url,
            Zeroizing::new(body),
            Some(auth),
            "application/json",
            limit,
        )
        .await;
    if matches!(result, Err(AuthorityError::Enrollment)) {
        let _ = authority.controller.reject(&authority.lease);
    }
    authority.check()?;
    result
}
impl NativeAuthorityFactory {
    /// Packet must come from a user-verified separate channel. Creating approval
    /// is explicit, and its immutable ciphertext/body are vaulted before dispatch.
    /// A pending slot requires retry/finish; this never silently replaces it.
    pub async fn approve_pair(
        &self,
        authority: &NativeAuthority,
        trusted_request: &str,
        confirmed: bool,
    ) -> AuthorityResult<PairApprovalPacket> {
        if !confirmed {
            return Err(AuthorityError::LifecycleGated);
        }
        let mut record = active_record(self, authority).await?;
        if record.pairing.is_some() {
            return Err(AuthorityError::Busy);
        }
        let request = PairRequestPacket::parse(
            trusted_request,
            record.scope.account_digest(),
            record.scope.library_id(),
        )?;
        request.check_fresh()?;
        if request.signing_thumb == record.scope.device_id() {
            return Err(AuthorityError::Scope);
        }
        let jwe = wrapping::encrypt_pair(
            &material::export(&record)?,
            &request.wrapping_jwk,
            &request.context(record.scope.device_id())?,
        )?;
        let sender = PublicJwk::parse(
            &serde_json::to_vec(&authority.signer.jwk).map_err(|_| AuthorityError::Custody)?,
        )?;
        let mut packet = PairApprovalPacket {
            v: 1,
            kind: "mdreader-pair-approval".into(),
            account: request.account.clone(),
            library_id: request.library_id.clone(),
            pair_id: request.pair_id.clone(),
            sender_jwk: sender,
            sender_thumb: record.scope.device_id().into(),
            grant_hash: String::new(),
            jwe_hash: crate::blob_digest(jwe.as_bytes()),
        };
        let response = send(
            self,
            authority,
            &format!("/v1/pairings/{}/challenge", request.pair_id),
            serde_json::to_vec(&serde_json::json!({"jweHash":packet.jwe_hash}))
                .map_err(|_| AuthorityError::Custody)?,
            8192,
        )
        .await?;
        let response: enrollment::challenge::Response =
            serde_json::from_slice(&response).map_err(|_| AuthorityError::Enrollment)?;
        let c = Challenge::parse(
            &response.challenge,
            &request,
            &packet,
            Some(&self.client.registration.api_audience),
            true,
        )?;
        if c.nonce != response.nonce || c.exp != response.expires {
            return Err(AuthorityError::Scope);
        }
        let current = now()?;
        let claims = grant::Grant {
            v: 1,
            account: request.account.clone(),
            library_id: request.library_id.clone(),
            pair_id: request.pair_id.clone(),
            signing_thumb: request.signing_thumb.clone(),
            wrapping_thumb: request.wrapping_thumb.clone(),
            pair_nonce: request.nonce.clone(),
            sender_thumb: packet.sender_thumb.clone(),
            jwe_hash: packet.jwe_hash.clone(),
            approval_nonce: c.nonce,
            challenge_id: c.jti,
            iat: current,
            exp: c.exp.min(current.saturating_add(300)),
            jti: uuid::Uuid::new_v4().to_string(),
        };
        let grant = authority.signer.pair_grant(&claims)?;
        packet.grant_hash = crate::blob_digest(grant.as_bytes());
        let body = Body {
            challenge: response.challenge,
            grant,
            jwe,
        };
        validate_body(&body, &request, &packet)?;
        record.pairing = Some(Slot::Sending {
            request,
            body,
            packet,
            state: ApprovalState::Prepared,
        });
        authority.check()?;
        persist(self, &mut record).await?;
        authority.check()?;
        self.retry_pair_approval(authority, true).await
    }
    /// Explicit retry sends byte-identical approval, including JWE/grant/challenge.
    /// HTTP has no redirect, proxy or retry; uncertain results retain this slot.
    pub async fn retry_pair_approval(
        &self,
        authority: &NativeAuthority,
        confirmed: bool,
    ) -> AuthorityResult<PairApprovalPacket> {
        if !confirmed {
            return Err(AuthorityError::LifecycleGated);
        }
        let mut record = active_record(self, authority).await?;
        let Some(Slot::Sending { state, .. }) = &mut record.pairing else {
            return Err(AuthorityError::LifecycleGated);
        };
        if *state == ApprovalState::Confirmed {
            let Some(Slot::Sending { packet, .. }) = record.pairing else {
                return Err(AuthorityError::Custody);
            };
            return Ok(packet);
        }
        if *state == ApprovalState::Prepared {
            *state = ApprovalState::Uncertain;
            persist(self, &mut record).await?;
            authority.check()?;
        }
        let Some(Slot::Sending { request, body, .. }) = &record.pairing else {
            return Err(AuthorityError::Custody);
        };
        let bytes = send(
            self,
            authority,
            &format!("/v1/pairings/{}/approve", request.pair_id),
            serde_json::to_vec(body).map_err(|_| AuthorityError::Custody)?,
            4096,
        )
        .await?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Receipt {
            account: String,
            library_id: String,
            device: String,
            enrolled: bool,
        }
        let receipt: Receipt =
            serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        if !receipt.enrolled
            || receipt.account != request.account
            || receipt.library_id != request.library_id
            || receipt.device != request.signing_thumb
        {
            return Err(AuthorityError::Scope);
        }
        let Some(Slot::Sending { state, packet, .. }) = &mut record.pairing else {
            return Err(AuthorityError::Custody);
        };
        *state = ApprovalState::Confirmed;
        let packet = packet.clone();
        persist(self, &mut record).await?;
        authority.check()?;
        Ok(packet)
    }
    /// Explicit acknowledgement that the public packet has been handed to the
    /// candidate. Clears only confirmed slot; uncertain approval cannot be lost.
    pub async fn finish_pair_handoff(
        &self,
        authority: &NativeAuthority,
        confirmed: bool,
    ) -> AuthorityResult<()> {
        if !confirmed {
            return Err(AuthorityError::LifecycleGated);
        }
        let mut record = active_record(self, authority).await?;
        if !matches!(
            record.pairing,
            Some(Slot::Sending {
                state: ApprovalState::Confirmed,
                ..
            })
        ) {
            return Err(AuthorityError::LifecycleGated);
        }
        record.pairing = None;
        persist(self, &mut record).await?;
        authority.check()
    }
}
