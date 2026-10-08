use super::*;
use super::{
    custody::EnrollmentState, request::validate_sync_request, session::Lease, signing::SigningKey,
};
use crate::{
    transport::{
        Authority, Authorization, Budget, Owner, RunReport, Transport, TransportError,
        TransportResult,
    },
    EncryptionContext, PreparedRevision, RemoteChange, SyncScope,
};
use std::sync::Arc;
pub struct NativeAuthority {
    pub(super) record: CustodyRecord,
    pub(super) signer: SigningKey,
    pub(super) identity: VerifiedIdentity,
    pub(super) registration: Arc<ReviewedOidcRegistration>,
    pub(super) controller: Arc<SessionController>,
    pub(super) lease: Arc<Lease>,
}
impl std::fmt::Debug for NativeAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("NativeAuthority", f)
    }
}
impl NativeAuthority {
    fn check(&self) -> AuthorityResult<()> {
        let result = (|| {
            self.controller.check(&self.lease)?;
            self.identity.check(&self.registration)?;
            if self.record.scope != self.lease.scope
                || self.record.state != EnrollmentState::Confirmed
            {
                return Err(AuthorityError::Inactive);
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.controller.reject(&self.lease);
        }
        result
    }
    /// Convenience foreground run. Transport itself delivers rejection to the lease hook.
    pub async fn run<O: Owner>(
        &self,
        transport: &Transport,
        owner: &O,
        budget: Budget,
    ) -> TransportResult<RunReport> {
        transport.run(owner, self, budget).await
    }
}
impl Drop for NativeAuthority {
    fn drop(&mut self) {
        let _ = self.controller.reject(&self.lease);
    }
}
impl Authority for NativeAuthority {
    fn rejected(&self, _reason: crate::transport::AuthorityRejection) {
        let _ = self.controller.reject(&self.lease);
    }
    fn scope(&self) -> TransportResult<SyncScope> {
        self.check().map_err(|_| TransportError::Authority)?;
        Ok(self.record.scope.clone())
    }
    fn generation(&self) -> TransportResult<u64> {
        self.check().map_err(|_| TransportError::Authority)?;
        Ok(self.lease.epoch)
    }
    fn authorize(
        &self,
        method: &str,
        exact_url: &str,
        body: &[u8],
    ) -> TransportResult<Authorization> {
        self.check().map_err(|_| TransportError::Authority)?;
        validate_sync_request(
            &self.registration,
            &self.record.scope,
            method,
            exact_url,
            body,
        )
        .map_err(|_| TransportError::Authority)?;
        let auth = self
            .signer
            .authorize(&self.identity, method, exact_url, body)
            .map_err(|_| TransportError::Authority)?;
        self.check().map_err(|_| TransportError::Authority)?;
        Ok(auth)
    }
    fn authenticate_root(
        &self,
        scope: &SyncScope,
        change: &RemoteChange,
        body: &[u8],
    ) -> TransportResult<PreparedRevision> {
        self.check().map_err(|_| TransportError::Authority)?;
        if scope != &self.record.scope {
            return Err(TransportError::Authority);
        }
        let key_id = crate::crypto::envelope_key_id(body)?;
        let raw = self
            .record
            .keys
            .get(&key_id)
            .ok_or(TransportError::Authority)?;
        let key = crate::LibraryKey::from_bytes(key_id.clone(), raw.clone())?;
        let context = EncryptionContext {
            library_id: scope.library_id().into(),
            item_id: change.item_id.clone(),
            revision_id: change.revision_id.clone(),
            key_id,
        };
        let prepared = PreparedRevision::from_wire(
            &key,
            &context,
            change.blob.as_deref().ok_or(TransportError::Protocol)?,
            body,
        )?;
        self.check().map_err(|_| TransportError::Authority)?;
        Ok(prepared)
    }
}
