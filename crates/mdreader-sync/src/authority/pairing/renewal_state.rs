//! A replacement proposal never overwrites the original OOB identity before receipt.
use super::*;
use custody::{EnrollmentState, Lifecycle, Phase};
use enrollment::{Intent, Kind};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in super::super) enum RenewalPhase {
    Prepared,
    Uncertain,
    ApprovalRequired,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Renewal {
    pub operation_id: String,
    pub next: Intent,
    pub body: String,
    pub phase: RenewalPhase,
}
impl Renewal {
    pub(super) fn new(record: &CustodyRecord, next: Intent) -> AuthorityResult<Self> {
        let mut value = Self {
            operation_id: uuid::Uuid::new_v4().to_string(),
            next,
            body: String::new(),
            phase: RenewalPhase::Prepared,
        };
        value.body = value.expected_body(record)?;
        Ok(value)
    }
    fn expected_body(&self, record: &CustodyRecord) -> AuthorityResult<String> {
        let previous = candidate::own_request(record)?;
        let (signing, wrapping, _) = enrollment::bootstrap::public_material(record)?;
        serde_json::to_string(&serde_json::json!({"operationId":self.operation_id,"previousPairId":previous.pair_id,"previousNonce":previous.nonce,"challenge":self.next.challenge,"signingJwk":signing,"wrappingJwk":wrapping})).map_err(|_| AuthorityError::Custody)
    }
    pub(super) fn digest(&self) -> String {
        crate::blob_digest(self.body.as_bytes())
    }
}
pub(in super::super) fn validate(record: &CustodyRecord) -> AuthorityResult<()> {
    let Some(value) = &record.pair_renewal else {
        return Ok(());
    };
    crate::crypto::validate_uuid(&value.operation_id).map_err(|_| AuthorityError::Custody)?;
    if record.state != EnrollmentState::Pending
        || record.pairing.is_some()
        || record.pending_recovery.is_some()
        || !matches!(
            record.lifecycle,
            Lifecycle::V2 {
                phase: Phase::CandidatePending(CandidateKind::Pair),
                ..
            }
        )
        || value.body.len() > 8192
        || value.next.kind != Kind::Pair
        || value.expected_body(record)? != value.body
    {
        return Err(AuthorityError::Custody);
    }
    value.next.validate(record)?;
    let old = record.intent.as_ref().ok_or(AuthorityError::Custody)?;
    if !old.permits_update(&value.next)? || value.next.challenge == old.challenge {
        return Err(AuthorityError::Custody);
    }
    Ok(())
}
/// Called only after same-keys admission. Terminal result installation is exactly
/// one generation, preserving original intent until the specific response is held.
pub(in super::super) fn transition(
    old: &CustodyRecord,
    new: &CustodyRecord,
    expected: Option<u64>,
) -> AuthorityResult<bool> {
    if expected != Some(old.generation)
        || new.generation
            != old
                .generation
                .checked_add(1)
                .ok_or(AuthorityError::Generation)?
        || old.state != new.state
        || !old.lifecycle.same_phase(&new.lifecycle)
        || old.pending_recovery != new.pending_recovery
        || old.pairing != new.pairing
    {
        return Ok(false);
    }
    Ok(match (&old.pair_renewal, &new.pair_renewal) {
        (None, Some(b)) => old.intent == new.intent && b.phase == RenewalPhase::Prepared,
        (Some(a), Some(b)) => {
            let mut wanted = a.clone();
            wanted.phase = b.phase;
            old.intent == new.intent
                && &wanted == b
                && matches!(
                    (a.phase, b.phase),
                    (RenewalPhase::Prepared, RenewalPhase::Uncertain)
                        | (RenewalPhase::Uncertain, RenewalPhase::ApprovalRequired)
                )
        }
        (Some(a), None) if a.phase != RenewalPhase::ApprovalRequired => {
            old.intent == new.intent || new.intent.as_ref() == Some(&a.next.dispatched())
        }
        _ => false,
    })
}
pub(super) fn summary(record: &CustodyRecord) -> PairRenewalSummary {
    match record.pair_renewal.as_ref().map(|r| r.phase) {
        None => PairRenewalSummary::None,
        Some(RenewalPhase::Prepared) => PairRenewalSummary::Prepared,
        Some(RenewalPhase::Uncertain) => PairRenewalSummary::Uncertain,
        Some(RenewalPhase::ApprovalRequired) => PairRenewalSummary::ApprovalRequired,
    }
}
