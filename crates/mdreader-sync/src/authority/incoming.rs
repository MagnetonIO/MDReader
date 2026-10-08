//! Native-held key use only. No key getter, grant DTO or raw renderer crypto API.
use super::*;
use crate::{
    incoming::{self, EncryptedResource, PreparedIncoming},
    transport::{TransportError, TransportResult},
    LibraryKey, RemoteRevision, SyncScope,
};
impl NativeAuthority {
    fn with_library_keys<T>(
        &self,
        action: impl FnOnce(&SyncScope, &[&LibraryKey]) -> crate::Result<T>,
    ) -> TransportResult<T> {
        self.check().map_err(|_| TransportError::Authority)?;
        let keys = self
            .record
            .keys
            .iter()
            .map(|(id, raw)| LibraryKey::from_bytes(id.clone(), raw.clone()))
            .collect::<crate::Result<Vec<_>>>()?;
        let references = keys.iter().collect::<Vec<_>>();
        let result = action(&self.record.scope, &references)?;
        self.check().map_err(|_| TransportError::Authority)?;
        Ok(result)
    }
    pub fn incoming_dependencies(&self, revision: &RemoteRevision) -> TransportResult<Vec<String>> {
        self.with_library_keys(|scope, keys| incoming::dependencies(scope, revision, keys))
    }
    pub fn prepare_incoming(
        &self,
        revision: &RemoteRevision,
        resources: &[EncryptedResource<'_>],
    ) -> TransportResult<PreparedIncoming> {
        self.with_library_keys(|scope, keys| incoming::prepare(scope, revision, keys, resources))
    }
}
