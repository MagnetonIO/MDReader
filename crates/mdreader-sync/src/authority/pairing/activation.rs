use super::*;
use custody::{EnrollmentState, Lifecycle, Phase};
use serde::Deserialize;
impl NativeAuthorityFactory {
    /// Requires a separately user-verified approval packet, never sender material
    /// taken from server delivery. Pins both hashes before any decryption.
    pub async fn accept_pair(
        &self,
        login: VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
        trusted_approval: &str,
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
        if record.pairing.is_some()
            || !matches!(
                record.lifecycle,
                Lifecycle::V2 {
                    phase: Phase::CandidatePending(CandidateKind::Pair),
                    ..
                }
            )
        {
            return Err(AuthorityError::LifecycleGated);
        }
        let request = candidate::own_request(&record)?;
        let approval_won = record
            .pair_renewal
            .as_ref()
            .is_some_and(|r| r.phase == renewal_state::RenewalPhase::ApprovalRequired);
        if record.pair_renewal.is_some() && !approval_won {
            return Err(AuthorityError::LifecycleGated);
        }
        if approval_won {
            self.enrollment_anchor(&login, &record).await?;
        } else {
            request.check_fresh()?;
        }
        let packet = PairApprovalPacket::parse(trusted_approval, &request)?;
        let url = self
            .client
            .registration
            .sync
            .join(&format!("/v1/pairings/{}", request.pair_id))
            .map_err(|_| AuthorityError::Configuration)?;
        let auth = signing::SigningKey::load(&record.pkcs8)?.authorize(
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
                16384,
            )
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Delivered {
            approved: bool,
            grant: Option<String>,
            jwe: Option<String>,
        }
        let delivered: Delivered =
            serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        if !delivered.approved {
            return Err(AuthorityError::Enrollment);
        }
        let grant = delivered.grant.ok_or(AuthorityError::Enrollment)?;
        let jwe = delivered.jwe.ok_or(AuthorityError::Enrollment)?;
        let verified = grant::Grant::verify(&grant, &request, &packet, &jwe, !approval_won)?;
        if approval_won
            && (verified.iat >= request.expires || verified.iat > now()?.saturating_add(5))
        {
            return Err(AuthorityError::Enrollment);
        }
        state::decrypt(&record, &request, &packet, &jwe)?;
        record.pair_renewal = None;
        record.pairing = Some(Slot::Receiving {
            request,
            packet,
            grant,
            jwe,
        });
        persist(self, &mut record).await?;
        self.check_controller(&login.controller, login.epoch)?;
        self.reconcile_pair(login, reference, library, true).await
    }
    /// Restart reconciliation consumes only the previously user-verified packet
    /// and immutable encrypted delivery held in the OS vault. Candidate remains
    /// keyless until signed enrollment scope succeeds and one generation CAS.
    pub async fn reconcile_pair(
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
        let Some(Slot::Receiving {
            request,
            packet,
            jwe,
            ..
        }) = &record.pairing
        else {
            return Err(AuthorityError::LifecycleGated);
        };
        let keys = state::decrypt(&record, request, packet, jwe)?;
        self.enrollment_anchor(&login, &record).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let generation = record.generation;
        let Lifecycle::V2 { phase, .. } = &mut record.lifecycle else {
            return Err(AuthorityError::LifecycleGated);
        };
        *phase = Phase::Enrolled;
        record.state = EnrollmentState::Confirmed;
        record.keys = keys;
        record.pairing = None;
        record.confirm_intent()?;
        record.generation = generation
            .checked_add(1)
            .ok_or(AuthorityError::Generation)?;
        self.vault
            .exchange(generation, &record, Instant::now() + Duration::from_secs(5))
            .await?;
        self.check_controller(&login.controller, login.epoch)?;
        let current = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        if current.generation != record.generation || !current.same_material(&record) {
            return Err(AuthorityError::Generation);
        }
        login.identity.check(&self.client.registration)?;
        let signer = signing::SigningKey::load(&record.pkcs8)?;
        let lease = self
            .controller
            .activate(login.epoch, record.scope.clone())?;
        Ok(NativeAuthority {
            record,
            signer,
            identity: login.identity,
            registration: self.client.registration.clone(),
            controller: self.controller.clone(),
            lease,
        })
    }
}
