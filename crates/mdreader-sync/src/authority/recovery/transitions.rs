use super::*;
use custody::{EnrollmentState, Lifecycle, Phase};
use enrollment::Kind;
pub(in super::super) fn validate_pending(record: &CustodyRecord) -> AuthorityResult<()> {
    if let Some(bytes) = &record.pending_recovery {
        if !matches!(
            record.lifecycle,
            Lifecycle::V2 {
                phase: Phase::CandidatePending(CandidateKind::Recover),
                ..
            }
        ) || record
            .intent
            .as_ref()
            .is_none_or(|i| i.kind != Kind::Recover)
        {
            return Err(AuthorityError::Custody);
        }
        bundle::parse(
            bytes,
            record.scope.account_digest(),
            record.scope.library_id(),
        )?;
    }
    Ok(())
}
pub(in super::super) fn activation_transition(
    old: &CustodyRecord,
    new: &CustodyRecord,
    expected: Option<u64>,
) -> AuthorityResult<bool> {
    let Some(bytes) = &old.pending_recovery else {
        return Ok(false);
    };
    let material = bundle::parse(bytes, old.scope.account_digest(), old.scope.library_id())?;
    let (
        Lifecycle::V2 {
            wrapping: a,
            recovery: None,
            phase: Phase::CandidatePending(CandidateKind::Recover),
        },
        Lifecycle::V2 {
            wrapping: b,
            recovery: Some(recovery),
            phase: Phase::Enrolled,
        },
    ) = (&old.lifecycle, &new.lifecycle)
    else {
        return Ok(false);
    };
    Ok(expected == Some(old.generation)
        && new.generation
            == old
                .generation
                .checked_add(1)
                .ok_or(AuthorityError::Generation)?
        && old.reference == new.reference
        && old.registration == new.registration
        && old.scope == new.scope
        && old.pkcs8 == new.pkcs8
        && a == b
        && old.state == EnrollmentState::Pending
        && new.state == EnrollmentState::Confirmed
        && old.keys.is_empty()
        && new.keys == material.keys
        && recovery == &material.recovery
        && match (&old.intent, &new.intent) {
            (Some(a), Some(b)) => a.permits_confirmation(b),
            _ => false,
        }
        && new.pending_recovery.is_none())
}
