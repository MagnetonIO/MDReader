use super::*;
pub(in super::super) fn admission(
    old: &CustodyRecord,
    new: &CustodyRecord,
    expected: Option<u64>,
) -> AuthorityResult<()> {
    if super::super::recovery::activation_transition(old, new, expected)? {
        return Ok(());
    }
    if super::super::pairing::activation_transition(old, new, expected)? {
        return Ok(());
    }
    if !old.same_keys(new) {
        return Err(AuthorityError::Generation);
    }
    if super::super::pairing::renewal_transition(old, new, expected)? {
        return Ok(());
    }
    if old.pair_renewal != new.pair_renewal {
        // Only an approval-won proposal may become a separately trusted receive slot.
        if !super::super::pairing::approval_won_receive(old, new) {
            return Err(AuthorityError::Generation);
        }
    }
    if super::super::pairing::slot_transition(old, new, expected)? {
        return Ok(());
    }
    match expected {
        None if old.generation == new.generation
            && old.state == new.state
            && old.lifecycle.same_phase(&new.lifecycle)
            && old.intent == new.intent
            && old.pending_recovery == new.pending_recovery
            && old.pairing == new.pairing
            && old.pair_renewal == new.pair_renewal =>
        {
            Ok(())
        }
        Some(g)
            if old.generation == g
                && new.generation == g.checked_add(1).ok_or(AuthorityError::Generation)?
                && old.state == EnrollmentState::Pending
                && new.state == EnrollmentState::Pending
                && old.lifecycle.same_phase(&new.lifecycle)
                && old.pairing == new.pairing
                && old.pair_renewal == new.pair_renewal
                && new.intent.is_some()
                && match (&old.intent, &new.intent) {
                    (None, Some(_)) => true,
                    (Some(a), Some(b)) if old.pending_recovery == new.pending_recovery => {
                        a.permits_update(b)?
                    }
                    _ => false,
                } =>
        {
            Ok(())
        }
        Some(g)
            if old.generation == g
                && new.generation == g.checked_add(1).ok_or(AuthorityError::Generation)?
                && old.state == EnrollmentState::Pending
                && new.state == EnrollmentState::Confirmed
                && match (&old.intent, &new.intent) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.permits_confirmation(b),
                    _ => false,
                }
                && old.pending_recovery == new.pending_recovery
                && old.pairing == new.pairing
                && old.pair_renewal == new.pair_renewal
                && old.lifecycle.can_confirm_to(&new.lifecycle) =>
        {
            Ok(())
        }
        _ => Err(AuthorityError::Generation),
    }
}
