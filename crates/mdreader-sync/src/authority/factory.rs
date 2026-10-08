use super::*;
use super::{custody::EnrollmentState, signing::SigningKey};
use serde::Deserialize;
use std::{
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
pub struct NativeLogin {
    pub(super) pending: PendingLogin,
    pub(super) enrollment: Option<EnrollmentLogin>,
    pub(super) epoch: u64,
    pub(super) controller: Weak<SessionController>,
}
impl std::fmt::Debug for NativeLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("NativeLogin", f)
    }
}
impl NativeLogin {
    pub async fn open(&self, browser: &dyn NativeBrowser) -> AuthorityResult<()> {
        let controller = self.controller.upgrade().ok_or(AuthorityError::Inactive)?;
        controller.check_epoch(self.epoch)?;
        self.pending.open(browser).await
    }
}
pub struct VerifiedLogin {
    pub(super) identity: VerifiedIdentity,
    pub(super) enrollment: Option<EnrollmentLogin>,
    pub(super) epoch: u64,
    pub(super) controller: Weak<SessionController>,
}
impl std::fmt::Debug for VerifiedLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("VerifiedLogin", f)
    }
}
/// Configured/owned by native service only. There is no default registration or product RPC.
pub struct NativeAuthorityFactory {
    pub(super) client: OidcClient,
    pub(super) vault: SyncVaultClient,
    pub(super) controller: Arc<SessionController>,
}
impl std::fmt::Debug for NativeAuthorityFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("NativeAuthorityFactory", f)
    }
}
impl NativeAuthorityFactory {
    pub fn new(
        client: OidcClient,
        vault: SyncVaultClient,
        controller: Arc<SessionController>,
    ) -> Self {
        Self {
            client,
            vault,
            controller,
        }
    }
    pub fn begin_login(&self, native_loopback_redirect: &str) -> AuthorityResult<NativeLogin> {
        let epoch = self.controller.epoch()?;
        Ok(NativeLogin {
            pending: self.client.begin(native_loopback_redirect)?,
            enrollment: None,
            epoch,
            controller: Arc::downgrade(&self.controller),
        })
    }
    pub async fn verify_login(
        &self,
        pending: NativeLogin,
        callback_target: &str,
    ) -> AuthorityResult<VerifiedLogin> {
        self.check_controller(&pending.controller, pending.epoch)?;
        let identity = self
            .client
            .complete(pending.pending, callback_target)
            .await?;
        self.check_controller(&pending.controller, pending.epoch)?;
        if pending
            .enrollment
            .as_ref()
            .is_some_and(|binding| !binding.accepts_account(&identity.account))
        {
            return Err(AuthorityError::Scope);
        }
        Ok(VerifiedLogin {
            identity,
            enrollment: pending.enrollment,
            epoch: pending.epoch,
            controller: pending.controller,
        })
    }
    pub(super) fn check_controller(
        &self,
        controller: &Weak<SessionController>,
        epoch: u64,
    ) -> AuthorityResult<()> {
        let same = controller
            .upgrade()
            .is_some_and(|c| Arc::ptr_eq(&c, &self.controller));
        if !same {
            return Err(AuthorityError::Inactive);
        }
        self.controller.check_epoch(epoch)
    }
    pub fn prepare_pending(
        &self,
        login: &VerifiedLogin,
        local_library_id: &str,
    ) -> AuthorityResult<PendingCustody> {
        self.check_controller(&login.controller, login.epoch)?;
        PendingCustody::prepare(&login.identity, &self.client.registration, local_library_id)
    }
    pub async fn persist_pending(&self, pending: &PendingCustody) -> AuthorityResult<()> {
        self.vault
            .create(pending, Instant::now() + Duration::from_secs(5))
            .await
    }
    /// Candidate has no library keys and cannot become a transport authority.
    /// Network enrollment/verified transfer remain separately gated.
    pub fn prepare_candidate(
        &self,
        login: &VerifiedLogin,
        library: &str,
        kind: CandidateKind,
    ) -> AuthorityResult<PendingCustody> {
        self.check_controller(&login.controller, login.epoch)?;
        PendingCustody::candidate(&login.identity, &self.client.registration, library, kind)
    }
    /// No server bootstrap is attempted. A new/unpaired device remains inactive until the
    /// separately reviewed wrapping/recovery lifecycle can authorize it on the server.
    pub async fn resume(
        &self,
        login: VerifiedLogin,
        reference: &SyncCredentialReference,
        local_library_id: &str,
    ) -> AuthorityResult<NativeAuthority> {
        self.check_controller(&login.controller, login.epoch)?;
        let mut record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, local_library_id)?;
        record.can_resume()?;
        let signer = SigningKey::load(&record.pkcs8)?;
        let url = self
            .client
            .registration
            .sync
            .join("/v1/enrollment")
            .map_err(|_| AuthorityError::Configuration)?;
        let auth = signer.authorize(&login.identity, "GET", url.as_str(), &[])?;
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
        login.identity.check(&self.client.registration)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Enrollment {
            account: String,
            library_id: String,
            device: String,
            enrolled: bool,
            recovery_jwk: serde_json::Value,
        }
        let receipt: Enrollment =
            serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
        if !receipt.enrolled
            || receipt.account != record.scope.account_digest()
            || receipt.library_id != record.scope.library_id()
            || receipt.device != record.scope.device_id()
        {
            return Err(AuthorityError::Scope);
        }
        validate_public_anchor(&receipt.recovery_jwk)?;
        record.check_recovery_anchor(&receipt.recovery_jwk)?;
        // V2 compares this anchor to persisted private material. A legacy V1 server
        // anchor does not establish local recovery possession. Product export remains gated.
        let current = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        if current.generation != record.generation
            || current.state != record.state
            || !current.same_material(&record)
        {
            return Err(AuthorityError::Generation);
        }
        if record.state == EnrollmentState::Pending {
            let generation = record.generation;
            record.generation = generation
                .checked_add(1)
                .ok_or(AuthorityError::Generation)?;
            record.confirm_bootstrap()?;
            self.vault
                .exchange(generation, &record, Instant::now() + Duration::from_secs(5))
                .await?;
        }
        self.check_controller(&login.controller, login.epoch)?;
        login.identity.check(&self.client.registration)?;
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
fn validate_public_anchor(value: &serde_json::Value) -> AuthorityResult<()> {
    let object = value.as_object().ok_or(AuthorityError::Enrollment)?;
    if object.len() != 4 || value["kty"] != "EC" || value["crv"] != "P-256" {
        return Err(AuthorityError::Enrollment);
    }
    for field in ["x", "y"] {
        let coordinate = value[field].as_str().ok_or(AuthorityError::Enrollment)?;
        if crate::crypto::decode_canonical(coordinate)
            .map_err(|_| AuthorityError::Enrollment)?
            .len()
            != 32
        {
            return Err(AuthorityError::Enrollment);
        }
    }
    Ok(())
}
