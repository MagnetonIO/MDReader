use super::*;
use custody::{Lifecycle, Phase};
use enrollment::{Intent, Kind};
use signing::SigningKey;

pub(super) fn own_request(record: &CustodyRecord) -> AuthorityResult<PairRequestPacket> {
    let intent = record
        .intent
        .as_ref()
        .filter(|i| i.kind == Kind::Pair)
        .ok_or(AuthorityError::Custody)?;
    let c = enrollment::challenge::Claims::parse(&intent.challenge)?;
    let (signing, wrapping, _) = enrollment::bootstrap::public_material(record)?;
    let packet = PairRequestPacket {
        v: 1,
        kind: "mdreader-pair-request".into(),
        account: record.scope.account_digest().into(),
        library_id: record.scope.library_id().into(),
        pair_id: c.jti,
        signing_jwk: PublicJwk::parse(
            &serde_json::to_vec(&signing).map_err(|_| AuthorityError::Custody)?,
        )?,
        wrapping_jwk: PublicJwk::parse(
            &serde_json::to_vec(&wrapping).map_err(|_| AuthorityError::Custody)?,
        )?,
        signing_thumb: record.scope.device_id().into(),
        wrapping_thumb: String::new(),
        nonce: intent.nonce.clone(),
        expires: intent.expires,
    };
    let mut packet = packet;
    packet.wrapping_thumb = packet.wrapping_jwk.thumbprint()?;
    packet.check_scope(record.scope.account_digest(), record.scope.library_id())?;
    Ok(packet)
}
impl NativeAuthorityFactory {
    /// Candidate uses its existing persisted signing/wrapping material, never new
    /// library keys. The challenge and exact request survive process restart.
    pub async fn prepare_pair_login(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<EnrollmentLogin> {
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        if !matches!(
            record.lifecycle,
            Lifecycle::V2 {
                phase: Phase::CandidatePending(CandidateKind::Pair),
                ..
            }
        ) {
            return Err(AuthorityError::LifecycleGated);
        }
        if let Some(intent) = &record.intent {
            return intent.binding(&record, login.epoch);
        }
        let response = enrollment::challenge::fetch(self, &record, Kind::Pair).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        record.intent = Some(Intent {
            progress: Default::default(),
            kind: Kind::Pair,
            recovery_grant: None,
            body: request_body(&record, &response.challenge)?,
            challenge: response.challenge,
            nonce: response.nonce,
            expires: response.expires,
        });
        persist(self, &mut record).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record
            .intent
            .as_ref()
            .ok_or(AuthorityError::Custody)?
            .binding(&record, login.epoch)
    }
    /// Explicit dispatch only; no automatic retry after an uncertain outcome.
    /// Response must exactly match the request reconstructed from local custody.
    pub async fn request_pair(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<PairRequestPacket> {
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        if !matches!(
            record.lifecycle,
            Lifecycle::V2 {
                phase: Phase::CandidatePending(CandidateKind::Pair),
                ..
            }
        ) {
            return Err(AuthorityError::LifecycleGated);
        }
        let expected = own_request(&record)?;
        expected.check_fresh()?;
        self.enrollment_dispatch(login, &mut record).await?;
        let body = &record.intent.as_ref().ok_or(AuthorityError::Custody)?.body;
        let url = self
            .client
            .registration
            .sync
            .join("/v1/pairings")
            .map_err(|_| AuthorityError::Configuration)?;
        let auth = SigningKey::load(&record.pkcs8)?.authorize(
            &login.identity,
            "POST",
            url.as_str(),
            body.as_bytes(),
        )?;
        let bytes = self
            .client
            .http
            .send(
                reqwest::Method::POST,
                url,
                Zeroizing::new(body.as_bytes().to_vec()),
                Some(auth),
                "application/json",
                MAX_PACKET,
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        let packet = PairRequestPacket::parse(text, record.scope.account_digest(), library)?;
        if packet != expected {
            return Err(AuthorityError::Scope);
        }
        Ok(packet)
    }
    /// Reopen the local public packet after a lost response; server delivery is
    /// still checked by the approving device. This does not grant candidate keys.
    pub async fn pair_request_packet(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<PairRequestPacket> {
        self.check_controller(&login.controller, login.epoch)?;
        let record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let packet = own_request(&record)?;
        packet.check_fresh()?;
        Ok(packet)
    }
}
