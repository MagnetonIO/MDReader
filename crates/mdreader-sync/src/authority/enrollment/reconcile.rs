use super::*;
use signing::SigningKey;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;
/// Explicit expired-intent retry never creates new keys or infers enrollment.
pub enum EnrollmentProgress {
    Enrolled(Box<NativeAuthority>),
    Login(EnrollmentLogin),
}
impl NativeAuthorityFactory {
    pub(in super::super) async fn enrollment_anchor(
        &self,
        login: &VerifiedLogin,
        record: &CustodyRecord,
    ) -> AuthorityResult<wrapping::PublicJwk> {
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(
            &login.identity,
            &self.client.registration,
            record.scope.library_id(),
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
        record.check_identity(
            &login.identity,
            &self.client.registration,
            record.scope.library_id(),
        )?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Response {
            account: String,
            library_id: String,
            device: String,
            enrolled: bool,
            recovery_jwk: wrapping::PublicJwk,
        }
        let receipt: Response =
            serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        if !receipt.enrolled
            || receipt.account != record.scope.account_digest()
            || receipt.library_id != record.scope.library_id()
            || receipt.device != record.scope.device_id()
        {
            return Err(AuthorityError::Scope);
        }
        receipt.recovery_jwk.thumbprint()?;
        Ok(receipt.recovery_jwk)
    }
    /// Explicit user retry after expiry first probes the same account/device.
    /// A denied probe does not prove absence; the unchanged Worker sticky-history
    /// and device-known checks remain authoritative and may reject the next POST.
    pub async fn renew_enrollment(
        &self,
        login: VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
        confirmed: bool,
    ) -> AuthorityResult<EnrollmentProgress> {
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
        let intent = record
            .intent
            .as_ref()
            .ok_or(AuthorityError::LifecycleGated)?;
        let unexpired = intent.expires > now()?;
        if record.state != custody::EnrollmentState::Pending
            || (unexpired && (intent.kind != Kind::Bootstrap || !intent.owner_summary()?.uncertain))
        {
            return Err(AuthorityError::Generation);
        }
        let kind = intent.kind;
        match self.enrollment_anchor(&login, &record).await {
            Ok(_) => {
                return match kind {
                    Kind::Bootstrap => self
                        .resume(login, reference, library)
                        .await
                        .map(|authority| EnrollmentProgress::Enrolled(Box::new(authority))),
                    Kind::Recover => self
                        .reconcile_recovery(login, reference, library, true)
                        .await
                        .map(|authority| EnrollmentProgress::Enrolled(Box::new(authority))),
                    Kind::Pair => self
                        .reconcile_pair(login, reference, library, true)
                        .await
                        .map(|authority| EnrollmentProgress::Enrolled(Box::new(authority))),
                }
            }
            Err(AuthorityError::Enrollment) => {}
            Err(error) => return Err(error),
        }
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        if unexpired {
            // Explicit uncertain retry reuses the exact persisted bytes/challenge.
            // Denial never established absence: Worker sticky history remains the
            // authority, including an authenticated existing-library conflict.
            return Ok(EnrollmentProgress::Login(
                record
                    .intent
                    .as_ref()
                    .ok_or(AuthorityError::Custody)?
                    .binding(&record, login.epoch)?,
            ));
        }
        if kind == Kind::Pair {
            // A denied probe cannot prove that the original pair was not approved.
            // Retain its OOB identity instead of issuing a replacement challenge.
            return Err(AuthorityError::LifecycleGated);
        }
        let response = challenge::fetch(self, &record, kind).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let mut next = match kind {
            Kind::Bootstrap => Intent {
                progress: Default::default(),
                kind,
                recovery_grant: None,
                body: bootstrap::body(&record, &response.challenge)?,
                challenge: response.challenge,
                nonce: response.nonce,
                expires: response.expires,
            },
            Kind::Recover => super::super::recovery::renew_intent(&record, response)?,
            // Preserve the original request/OOB identity after expiry. A new pair
            // attempt requires its own explicitly selected trusted-channel exchange.
            Kind::Pair => return Err(AuthorityError::LifecycleGated),
        };
        next.renew_from(record.intent.as_ref().ok_or(AuthorityError::Custody)?)?;
        let generation = record.generation;
        record.intent = Some(next);
        record.generation = generation
            .checked_add(1)
            .ok_or(AuthorityError::Generation)?;
        self.vault
            .exchange(generation, &record, Instant::now() + Duration::from_secs(5))
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let current = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        if current.generation != record.generation || !current.same_material(&record) {
            return Err(AuthorityError::Generation);
        }
        Ok(EnrollmentProgress::Login(
            record
                .intent
                .as_ref()
                .ok_or(AuthorityError::Custody)?
                .binding(&record, login.epoch)?,
        ))
    }
}
