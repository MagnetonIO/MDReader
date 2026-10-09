//! Explicit same-identity replacement, with durable result reconciliation.
use super::*;
use custody::{EnrollmentState, Lifecycle, Phase};
use enrollment::{Intent, Kind};
use renewal_state::{Renewal, RenewalPhase};
use serde::Deserialize;
use signing::SigningKey;

pub enum PairRenewalPreparation {
    Login(EnrollmentLogin),
    Complete(PairRenewalResult),
}
pub enum PairRenewalResult {
    Request(PairRequestPacket),
    ApprovalRequired,
    NotCommitted,
}
#[derive(Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum Receipt {
    Renewed {
        v: u8,
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "previousPairId")]
        previous_pair_id: String,
        #[serde(rename = "bodyDigest")]
        body_digest: String,
        request: PairRequestPacket,
    },
    Approved {
        v: u8,
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "previousPairId")]
        previous_pair_id: String,
        #[serde(rename = "bodyDigest")]
        body_digest: String,
    },
    NotCommitted {
        v: u8,
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "previousPairId")]
        previous_pair_id: String,
        #[serde(rename = "bodyDigest")]
        body_digest: String,
    },
}
fn require_candidate(record: &CustodyRecord) -> AuthorityResult<()> {
    if record.state != EnrollmentState::Pending
        || record.pairing.is_some()
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
    Ok(())
}
fn proposed(record: &CustodyRecord, renewal: &Renewal) -> AuthorityResult<PairRequestPacket> {
    let mut copy = record.copy();
    copy.intent = Some(renewal.next.clone());
    copy.pair_renewal = None;
    candidate::own_request(&copy)
}
impl NativeAuthorityFactory {
    /// Stage a fresh challenge alongside the old request. No key/library replacement.
    /// Uncertain proposals reconcile atomically before another proposal is possible.
    pub async fn prepare_pair_renewal(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<PairRenewalPreparation> {
        let mut record = self.renewal_record(login, reference, library).await?;
        if let Some(renewal) = &record.pair_renewal {
            if renewal.phase == RenewalPhase::ApprovalRequired {
                return Ok(PairRenewalPreparation::Complete(
                    PairRenewalResult::ApprovalRequired,
                ));
            }
            if renewal.phase == RenewalPhase::Prepared && renewal.next.expires > now()? {
                return Ok(PairRenewalPreparation::Login(
                    renewal.next.binding(&record, login.epoch)?,
                ));
            }
            let result = self
                .reconcile_pair_renewal(login, reference, library)
                .await?;
            if !matches!(result, PairRenewalResult::NotCommitted) {
                return Ok(PairRenewalPreparation::Complete(result));
            }
            record = self.renewal_record(login, reference, library).await?;
        }
        let old = record
            .intent
            .as_ref()
            .ok_or(AuthorityError::LifecycleGated)?;
        if old.kind != Kind::Pair || old.expires > now()? {
            return Err(AuthorityError::LifecycleGated);
        }
        let response = enrollment::challenge::fetch(self, &record, Kind::Pair).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let mut next = Intent {
            progress: Default::default(),
            kind: Kind::Pair,
            recovery_grant: None,
            body: request_body(&record, &response.challenge)?,
            challenge: response.challenge,
            nonce: response.nonce,
            expires: response.expires,
        };
        next.renew_from(record.intent.as_ref().ok_or(AuthorityError::Custody)?)?;
        record.pair_renewal = Some(Renewal::new(&record, next)?);
        persist(self, &mut record).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        Ok(PairRenewalPreparation::Login(
            record
                .pair_renewal
                .as_ref()
                .ok_or(AuthorityError::Custody)?
                .next
                .binding(&record, login.epoch)?,
        ))
    }
    /// Nonce-bound dispatch uses exact proposal bytes, marking uncertainty first.
    pub async fn request_pair_renewal(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<PairRenewalResult> {
        let mut record = self.renewal_record(login, reference, library).await?;
        login
            .enrollment
            .as_ref()
            .ok_or(AuthorityError::Identity)?
            .check(&record, login.epoch)?;
        let renewal = record
            .pair_renewal
            .as_mut()
            .ok_or(AuthorityError::LifecycleGated)?;
        if renewal.phase != RenewalPhase::Prepared {
            return Err(AuthorityError::LifecycleGated);
        }
        let claims = enrollment::challenge::Claims::parse(&renewal.next.challenge)?;
        if login
            .enrollment
            .as_ref()
            .is_none_or(|e| !e.matches_intent(&renewal.next))
        {
            return Err(AuthorityError::Scope);
        }
        claims.check_fresh(&self.client.registration.api_audience)?;
        renewal.phase = RenewalPhase::Uncertain;
        persist(self, &mut record).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        let body = record
            .pair_renewal
            .as_ref()
            .ok_or(AuthorityError::Custody)?
            .body
            .as_bytes()
            .to_vec();
        let bytes = self
            .renewal_http(login, &record, "/v1/pairings/renew", body)
            .await?;
        self.apply_pair_renewal(login, record, &bytes).await
    }
    /// Ordinary login probes a durable receipt or atomically retires an absent
    /// operation. A delayed POST cannot commit after not_committed is returned.
    pub async fn reconcile_pair_renewal(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<PairRenewalResult> {
        let record = self.renewal_record(login, reference, library).await?;
        let renewal = record
            .pair_renewal
            .as_ref()
            .ok_or(AuthorityError::LifecycleGated)?;
        if renewal.phase == RenewalPhase::ApprovalRequired {
            return Ok(PairRenewalResult::ApprovalRequired);
        }
        let previous = candidate::own_request(&record)?;
        let body = serde_json::to_vec(&serde_json::json!({"operationId":renewal.operation_id,"previousPairId":previous.pair_id,"bodyDigest":renewal.digest(),"libraryId":library})).map_err(|_| AuthorityError::Custody)?;
        let bytes = self
            .renewal_http(login, &record, "/v1/pairings/renew/reconcile", body)
            .await?;
        self.apply_pair_renewal(login, record, &bytes).await
    }
    async fn renewal_record(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<CustodyRecord> {
        self.check_controller(&login.controller, login.epoch)?;
        let record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        require_candidate(&record)?;
        Ok(record)
    }
    async fn renewal_http(
        &self,
        login: &VerifiedLogin,
        record: &CustodyRecord,
        path: &str,
        body: Vec<u8>,
    ) -> AuthorityResult<Zeroizing<Vec<u8>>> {
        let url = self
            .client
            .registration
            .sync
            .join(path)
            .map_err(|_| AuthorityError::Configuration)?;
        let auth = SigningKey::load(&record.pkcs8)?.authorize(
            &login.identity,
            "POST",
            url.as_str(),
            &body,
        )?;
        self.client
            .http
            .send(
                reqwest::Method::POST,
                url,
                Zeroizing::new(body),
                Some(auth),
                "application/json",
                8192,
            )
            .await
    }
    async fn apply_pair_renewal(
        &self,
        login: &VerifiedLogin,
        mut record: CustodyRecord,
        bytes: &[u8],
    ) -> AuthorityResult<PairRenewalResult> {
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(
            &login.identity,
            &self.client.registration,
            record.scope.library_id(),
        )?;
        let receipt: Receipt =
            serde_json::from_slice(bytes).map_err(|_| AuthorityError::Enrollment)?;
        let (v, operation, previous, digest) = match &receipt {
            Receipt::Renewed {
                v,
                operation_id,
                previous_pair_id,
                body_digest,
                ..
            }
            | Receipt::Approved {
                v,
                operation_id,
                previous_pair_id,
                body_digest,
            }
            | Receipt::NotCommitted {
                v,
                operation_id,
                previous_pair_id,
                body_digest,
            } => (*v, operation_id, previous_pair_id, body_digest),
        };
        let renewal = record
            .pair_renewal
            .as_ref()
            .ok_or(AuthorityError::Custody)?;
        if v != 1
            || operation != &renewal.operation_id
            || previous != &candidate::own_request(&record)?.pair_id
            || digest != &renewal.digest()
        {
            return Err(AuthorityError::Scope);
        }
        let result = match receipt {
            Receipt::Renewed { request, .. } => {
                if request != proposed(&record, renewal)? {
                    return Err(AuthorityError::Scope);
                }
                record.intent = Some(renewal.next.dispatched());
                record.pair_renewal = None;
                PairRenewalResult::Request(request)
            }
            Receipt::Approved { .. } => {
                if renewal.phase != RenewalPhase::Uncertain {
                    return Err(AuthorityError::Generation);
                }
                self.enrollment_anchor(login, &record).await?;
                record
                    .pair_renewal
                    .as_mut()
                    .ok_or(AuthorityError::Custody)?
                    .phase = RenewalPhase::ApprovalRequired;
                PairRenewalResult::ApprovalRequired
            }
            Receipt::NotCommitted { .. } => {
                record.pair_renewal = None;
                PairRenewalResult::NotCommitted
            }
        };
        persist(self, &mut record).await?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(
            &login.identity,
            &self.client.registration,
            record.scope.library_id(),
        )?;
        Ok(result)
    }
}
