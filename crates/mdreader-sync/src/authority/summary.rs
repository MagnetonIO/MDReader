//! Native owner state only: no keys, identity/token claims, references or grants.
use super::*;
use custody::{Lifecycle, Phase};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyLifecycle {
    LegacyPending,
    LegacyEnrolled,
    BootstrapPending,
    PairCandidate { trusted_approval_staged: bool },
    RecoveryCandidate { material_staged: bool },
    Enrolled { recovery_export_available: bool },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrollmentSummary {
    pub uncertain: bool,
    pub expired: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairApprovalSummary {
    None,
    Prepared,
    Uncertain,
    Confirmed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustodySummary {
    pub lifecycle: CustodyLifecycle,
    pub enrollment: Option<EnrollmentSummary>,
    pub pair_approval: PairApprovalSummary,
}
impl NativeAuthorityFactory {
    /// The service must share this exact controller with transport/projection,
    /// rather than supplying a second controller that cannot invalidate leases.
    pub fn session_controller(&self) -> Arc<SessionController> {
        self.controller.clone()
    }
    /// Explicit owner inspection only. No I/O occurs merely by constructing the
    /// factory. Absence is reported; it never creates replacement private material.
    pub async fn custody_summary(
        &self,
        reference: &SyncCredentialReference,
        library: &str,
    ) -> AuthorityResult<Option<CustodySummary>> {
        crate::crypto::validate_uuid(library).map_err(|_| AuthorityError::Scope)?;
        let epoch = self.controller.epoch()?;
        let record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?;
        self.controller.check_epoch(epoch)?;
        let Some(record) = record else {
            return Ok(None);
        };
        if record.registration != self.client.registration.digest
            || record.scope.library_id() != library
        {
            return Err(AuthorityError::Scope);
        }
        let lifecycle = match &record.lifecycle {
            Lifecycle::Legacy(_) if record.state == custody::EnrollmentState::Pending => {
                CustodyLifecycle::LegacyPending
            }
            Lifecycle::Legacy(_) => CustodyLifecycle::LegacyEnrolled,
            Lifecycle::V2 {
                phase: Phase::BootstrapPending,
                ..
            } => CustodyLifecycle::BootstrapPending,
            Lifecycle::V2 {
                phase: Phase::CandidatePending(CandidateKind::Pair),
                ..
            } => CustodyLifecycle::PairCandidate {
                trusted_approval_staged: record.pairing.is_some(),
            },
            Lifecycle::V2 {
                phase: Phase::CandidatePending(CandidateKind::Recover),
                ..
            } => CustodyLifecycle::RecoveryCandidate {
                material_staged: record.pending_recovery.is_some(),
            },
            Lifecycle::V2 {
                phase: Phase::Enrolled,
                recovery,
                ..
            } => CustodyLifecycle::Enrolled {
                recovery_export_available: recovery.is_some(),
            },
        };
        Ok(Some(CustodySummary {
            lifecycle,
            enrollment: record
                .intent
                .as_ref()
                .map(|i| i.owner_summary())
                .transpose()?,
            pair_approval: pairing::owner_summary(&record),
        }))
    }
}
