//! Explicit different-library connection keeps the entire original vault record.
use super::*;
use std::time::{Duration, Instant};
impl NativeAuthorityFactory {
    pub async fn prepare_retained_candidate(
        &self,
        login: &VerifiedLogin,
        previous: &SyncCredentialReference,
        previous_library: &str,
        library: &str,
        kind: CandidateKind,
    ) -> AuthorityResult<PendingCustody> {
        self.check_controller(&login.controller, login.epoch)?;
        let record = self
            .vault
            .get(previous, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, previous_library)?;
        if previous_library == library
            || record.state != custody::EnrollmentState::Pending
            || !matches!(
                &record.lifecycle,
                custody::Lifecycle::V2 {
                    phase: custody::Phase::BootstrapPending,
                    ..
                }
            )
            || !record
                .intent
                .as_ref()
                .is_some_and(|intent| intent.owner_summary().is_ok_and(|s| s.uncertain))
        {
            return Err(AuthorityError::LifecycleGated);
        }
        // No CAS/update/delete of previous material. New candidate has no keys;
        // only normal verified pairing/recovery can ever activate it.
        PendingCustody::candidate(&login.identity, &self.client.registration, library, kind)
    }
}
