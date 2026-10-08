//! Explicit native recovery export/import. No files, network or renderer DTOs.
mod activation;
mod bundle;
mod grant;
mod private_jwk;
mod transitions;
use super::*;
pub(super) use grant::{renew_intent, request_body};
use std::time::{Duration, Instant};
pub(super) use transitions::{activation_transition, validate_pending};
use wrapping::{JwePlaintext, RecoverySecret};
use zeroize::Zeroizing;

/// Encrypted local file plus independent secret, returned only after explicit export.
/// The native export adapter must keep the secret separate from the file/server.
pub struct RecoveryExport {
    file: Vec<u8>,
    secret: RecoverySecret,
}
impl RecoveryExport {
    pub fn file_bytes(&self) -> &[u8] {
        &self.file
    }
    pub fn export_secret(&self) -> Zeroizing<String> {
        self.secret.export()
    }
}
/// Decrypted import exists only in native memory until explicit activation.
/// No Debug/Serialize, no public key access, and no transport capability.
pub struct PendingRecovery {
    pub(super) material: bundle::Material,
    pub(super) reference: SyncCredentialReference,
    pub(super) registration: String,
    pub(super) generation: u64,
    pub(super) epoch: u64,
    expires: Instant,
}
impl PendingRecovery {
    pub(super) fn check(&self, record: &CustodyRecord, epoch: u64) -> AuthorityResult<()> {
        if Instant::now() >= self.expires
            || self.epoch != epoch
            || self.reference != record.reference
            || self.registration != record.registration
            || self.generation != record.generation
        {
            return Err(AuthorityError::Inactive);
        }
        self.material
            .check_scope(record.scope.account_digest(), record.scope.library_id())
    }
}
impl NativeAuthority {
    /// Explicit local export only. No file or server writes are performed here.
    pub fn export_recovery(&self, confirmed: bool) -> AuthorityResult<RecoveryExport> {
        if !confirmed {
            return Err(AuthorityError::LifecycleGated);
        }
        let permit = self.projection_permit()?;
        let plaintext = JwePlaintext::new(bundle::export(&self.record)?)?;
        let secret = RecoverySecret::generate()?;
        let jwe = wrapping::encrypt_recovery(&plaintext, &secret)?;
        // Revalidate the exact activation after local crypto work as well.
        permit.with_current(|_| ())?;
        let file =
            serde_json::to_vec(&serde_json::json!({"v":1,"kind":"mdreader-recovery","jwe":jwe}))
                .map_err(|_| AuthorityError::Custody)?;
        Ok(RecoveryExport { file, secret })
    }
}
impl NativeAuthorityFactory {
    /// Authenticate account before decrypting an explicitly selected recovery file.
    /// Candidate remains keyless; this result cannot activate it on its own.
    pub async fn prepare_recovery(
        &self,
        login: &VerifiedLogin,
        reference: &SyncCredentialReference,
        library: &str,
        file: &[u8],
        secret: Zeroizing<String>,
    ) -> AuthorityResult<PendingRecovery> {
        self.check_controller(&login.controller, login.epoch)?;
        let record = self
            .vault
            .get(reference, Instant::now() + Duration::from_secs(5))
            .await?
            .ok_or(AuthorityError::Custody)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        if !matches!(
            record.lifecycle,
            custody::Lifecycle::V2 {
                phase: custody::Phase::CandidatePending(CandidateKind::Recover),
                ..
            }
        ) {
            return Err(AuthorityError::LifecycleGated);
        }
        if file.len() > 33792 || secret.len() != 43 {
            return Err(AuthorityError::Bounds);
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            v: u8,
            kind: String,
            jwe: String,
        }
        let file: File = serde_json::from_slice(file).map_err(|_| AuthorityError::Custody)?;
        if file.v != 1 || file.kind != "mdreader-recovery" {
            return Err(AuthorityError::Custody);
        }
        let raw = Zeroizing::new(
            crate::crypto::decode_canonical(&secret).map_err(|_| AuthorityError::Custody)?,
        );
        let key = RecoverySecret::from_bytes(Zeroizing::new(
            raw.as_slice()
                .try_into()
                .map_err(|_| AuthorityError::Custody)?,
        ));
        let plaintext = wrapping::decrypt_recovery(&file.jwe, &key)?;
        let material = bundle::parse(plaintext.as_bytes(), record.scope.account_digest(), library)?;
        self.check_controller(&login.controller, login.epoch)?;
        record.check_identity(&login.identity, &self.client.registration, library)?;
        Ok(PendingRecovery {
            material,
            reference: reference.clone(),
            registration: record.registration,
            generation: record.generation,
            epoch: login.epoch,
            expires: Instant::now() + Duration::from_secs(300),
        })
    }
}
