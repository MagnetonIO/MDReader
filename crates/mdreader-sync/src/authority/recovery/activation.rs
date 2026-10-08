use super::grant::{request_body, Grant};
use super::*;
use custody::{EnrollmentState, Lifecycle, Phase};
use enrollment::{challenge::Claims, Intent, Kind};
use serde::Deserialize;
use signing::SigningKey;
impl NativeAuthorityFactory {
    /// Explicit activation intent is persisted in the OS vault before network enrollment.
    /// Library keys remain unavailable to transport until receipt+anchor reconciliation.
    pub async fn prepare_recovery_enrollment(
        &self,
        login: &VerifiedLogin,
        pending: &PendingRecovery,
        confirmed: bool,
    ) -> AuthorityResult<EnrollmentLogin> {
        if !confirmed {
            return Err(AuthorityError::LifecycleGated);
        }
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(&pending.reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(
            &login.identity,
            &self.client.registration,
            &pending.material.library,
        )?;
        pending.check(&record, login.epoch)?;
        if record.intent.is_some() || record.pending_recovery.is_some() {
            return Err(AuthorityError::Generation);
        }
        let (signing, wrapping, _) = enrollment::bootstrap::public_material(&record)?;
        let request=Zeroizing::new(serde_json::to_vec(&serde_json::json!({"kind":"recover","libraryId":pending.material.library,"signingJwk":signing,"wrappingJwk":wrapping,"recoveryJwk":null})).map_err(|_|AuthorityError::Custody)?);
        let url = self
            .client
            .registration
            .sync
            .join("/v1/enrollment/challenge")
            .map_err(|_| AuthorityError::Configuration)?;
        let bytes = self
            .client
            .http
            .send(
                reqwest::Method::POST,
                url,
                request,
                None,
                "application/json",
                8192,
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        pending.check(&record, login.epoch)?;
        login.identity.check(&self.client.registration)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Response {
            challenge: String,
            nonce: String,
            expires: u64,
        }
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        let c = Claims::parse(&response.challenge)?;
        c.check_record(&record)?;
        c.check_fresh(&self.client.registration.api_audience)?;
        if c.nonce != response.nonce || c.exp != response.expires {
            return Err(AuthorityError::Scope);
        }
        let (_, wrapping, _) = enrollment::bootstrap::public_material(&record)?;
        let thumb = wrapping::PublicJwk::parse(
            &serde_json::to_vec(&wrapping).map_err(|_| AuthorityError::Custody)?,
        )?
        .thumbprint()?;
        let iat = now()?;
        let grant = SigningKey::load(&pending.material.recovery)?.recovery_grant(&Grant {
            v: 1,
            account: pending.material.account.clone(),
            library_id: pending.material.library.clone(),
            signing_thumb: record.scope.device_id().into(),
            wrapping_thumb: thumb,
            nonce: c.nonce.clone(),
            challenge_id: c.jti.clone(),
            iat,
            exp: c.exp.min(iat + 300),
            jti: uuid::Uuid::new_v4().to_string(),
        })?;
        record.pending_recovery = Some(bundle::encode(&pending.material)?);
        let body = request_body(&record, &response.challenge, &grant)?;
        record.intent = Some(Intent {
            progress: Default::default(),
            kind: Kind::Recover,
            recovery_grant: Some(grant),
            challenge: response.challenge,
            nonce: response.nonce,
            expires: response.expires,
            body,
        });
        let generation = record.generation;
        record.generation = generation
            .checked_add(1)
            .ok_or(AuthorityError::Generation)?;
        self.vault
            .exchange(generation, &record, Instant::now() + Duration::from_secs(5))
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        let current = self
            .vault
            .get(&pending.reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        if current.generation != record.generation || !current.same_material(&record) {
            return Err(AuthorityError::Generation);
        }
        record
            .intent
            .as_ref()
            .ok_or(AuthorityError::Custody)?
            .binding(&record, login.epoch)
    }
    /// Reopen the same durable request after interruption without importing/generating keys.
    pub async fn recovery_enrollment_login(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<EnrollmentLogin> {
        self.check_controller(&login.controller, login.epoch)?;
        let record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let intent = record
            .intent
            .as_ref()
            .ok_or(AuthorityError::LifecycleGated)?;
        if intent.kind != Kind::Recover || record.pending_recovery.is_none() {
            return Err(AuthorityError::LifecycleGated);
        }
        intent.binding(&record, login.epoch)
    }
    pub async fn recover(
        &self,
        login: VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
        confirmed: bool,
    ) -> AuthorityResult<NativeAuthority> {
        if !confirmed {
            return Err(AuthorityError::LifecycleGated);
        }
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        login
            .enrollment
            .as_ref()
            .ok_or(AuthorityError::Identity)?
            .check(&record, login.epoch)?;
        self.enrollment_dispatch(&login, &mut record).await?;
        let intent = record.intent.as_ref().ok_or(AuthorityError::Custody)?;
        if intent.kind != Kind::Recover {
            return Err(AuthorityError::LifecycleGated);
        }
        let url = self
            .client
            .registration
            .sync
            .join("/v1/enrollment/recover")
            .map_err(|_| AuthorityError::Configuration)?;
        let auth = SigningKey::load(&record.pkcs8)?.authorize(
            &login.identity,
            "POST",
            url.as_str(),
            intent.body.as_bytes(),
        )?;
        let bytes = self
            .client
            .http
            .send(
                reqwest::Method::POST,
                url,
                Zeroizing::new(intent.body.as_bytes().to_vec()),
                Some(auth),
                "application/json",
                4096,
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        receipt(&bytes, &record)?;
        self.reconcile_recovery(login, reference, library, confirmed)
            .await
    }
    /// A fresh ordinary verified login can reconcile a lost/expired recovery POST reply.
    /// No key is installed until exact enrolled scope and held recovery anchor agree.
    pub async fn reconcile_recovery(
        &self,
        login: VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
        confirmed: bool,
    ) -> AuthorityResult<NativeAuthority> {
        if !confirmed {
            return Err(AuthorityError::LifecycleGated);
        }
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let material = bundle::parse(
            record
                .pending_recovery
                .as_ref()
                .ok_or(AuthorityError::LifecycleGated)?,
            record.scope.account_digest(),
            library,
        )?;
        let url = self
            .client
            .registration
            .sync
            .join("/v1/enrollment")
            .map_err(|_| AuthorityError::Configuration)?;
        let auth = SigningKey::load(&record.pkcs8)?.authorize(
            &login.identity,
            "GET",
            url.as_str(),
            &[],
        )?;
        let bytes = self
            .client
            .http
            .send(
                reqwest::Method::GET,
                url,
                Zeroizing::new(Vec::new()),
                Some(auth),
                "application/json",
                4096,
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Response {
            account: String,
            library_id: String,
            device: String,
            enrolled: bool,
            recovery_jwk: wrapping::PublicJwk,
        }
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        if !response.enrolled
            || response.account != record.scope.account_digest()
            || response.library_id != library
            || response.device != record.scope.device_id()
            || response.recovery_jwk.thumbprint()? != SigningKey::load(&material.recovery)?.device
        {
            return Err(AuthorityError::Scope);
        }
        let generation = record.generation;
        let Lifecycle::V2 { wrapping, .. } = &record.lifecycle else {
            return Err(AuthorityError::Custody);
        };
        record.lifecycle = Lifecycle::V2 {
            wrapping: wrapping.clone(),
            recovery: Some(material.recovery),
            phase: Phase::Enrolled,
        };
        record.keys = material.keys;
        record.pending_recovery = None;
        record.state = EnrollmentState::Confirmed;
        record.confirm_intent()?;
        record.generation = generation
            .checked_add(1)
            .ok_or(AuthorityError::Generation)?;
        self.vault
            .exchange(generation, &record, Instant::now() + Duration::from_secs(5))
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        self.resume(login, reference, library).await
    }
}
fn receipt(bytes: &[u8], record: &CustodyRecord) -> AuthorityResult<()> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Receipt {
        account: String,
        library_id: String,
        device: String,
        enrolled: bool,
    }
    let value: Receipt = serde_json::from_slice(bytes).map_err(|_| AuthorityError::Enrollment)?;
    if !value.enrolled
        || value.account != record.scope.account_digest()
        || value.library_id != record.scope.library_id()
        || value.device != record.scope.device_id()
    {
        return Err(AuthorityError::Scope);
    }
    Ok(())
}
