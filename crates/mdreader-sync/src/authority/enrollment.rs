//! Native enrollment intent and nonce-bound login. Never renderer/token persistence.
pub(super) mod bootstrap;
mod reconcile;
pub use reconcile::EnrollmentProgress;
pub(super) mod challenge;
use super::*;
use serde::{Deserialize, Serialize};

/// Native-only link between a persisted exact request and its fresh PKCE login.
/// Public fields/serialization are deliberately unavailable.
#[derive(Clone)]
pub struct EnrollmentLogin {
    reference: SyncCredentialReference,
    registration: String,
    account: String,
    generation: u64,
    digest: String,
    nonce: String,
    expires: u64,
    epoch: u64,
}
impl std::fmt::Debug for EnrollmentLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("EnrollmentLogin", f)
    }
}
impl EnrollmentLogin {
    pub(super) fn matches_intent(&self, intent: &Intent) -> bool {
        self.digest == intent.digest() && self.nonce == intent.nonce
    }
    pub(super) fn accepts_account(&self, account: &str) -> bool {
        self.account == account
    }
    pub(super) fn check(&self, record: &CustodyRecord, epoch: u64) -> AuthorityResult<()> {
        if self.reference != record.reference
            || self.registration != record.registration
            || self.account != record.scope.account_digest()
            || self.generation != record.generation
            || self.epoch != epoch
            || now()? >= self.expires
            || !record
                .intent
                .as_ref()
                .is_some_and(|intent| intent.digest() == self.digest && intent.nonce == self.nonce)
                && !record.pair_renewal.as_ref().is_some_and(|renewal| {
                    renewal.next.digest() == self.digest && renewal.next.nonce == self.nonce
                })
        {
            return Err(AuthorityError::Scope);
        }
        Ok(())
    }
}
mod intent;
pub(super) use intent::{Intent, Kind};
impl NativeAuthorityFactory {
    /// Start a fresh PKCE login using only a previously persisted HTTPS challenge.
    /// The verified response remains bound to this exact account and generation.
    pub fn begin_enrollment_login(
        &self,
        enrollment: &EnrollmentLogin,
        redirect: &str,
    ) -> AuthorityResult<NativeLogin> {
        self.controller.check_epoch(enrollment.epoch)?;
        if enrollment.registration != self.client.registration.digest
            || now()? >= enrollment.expires
        {
            return Err(AuthorityError::Identity);
        }
        Ok(NativeLogin {
            pending: self
                .client
                .begin_nonce(redirect, zeroize::Zeroizing::new(enrollment.nonce.clone()))?,
            epoch: enrollment.epoch,
            controller: std::sync::Arc::downgrade(&self.controller),
            enrollment: Some(enrollment.clone()),
        })
    }
}
