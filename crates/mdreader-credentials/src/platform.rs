//! Native platform storage retains the same verified grant codec and CAS rules.
use crate::{CredentialReference, ProviderGrant, Vault};
use mdreader_provider::{ProviderError, Result};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// Native-only custody. No renderer access to these bytes or references.
pub trait SecureGrantStore: Send + Sync {
    fn read(&self, reference: &str) -> Result<Option<Zeroizing<String>>>;
    fn write(&self, reference: &str, value: &str) -> Result<()>;
    fn delete(&self, reference: &str) -> Result<()>;
}
pub struct PlatformVault {
    store: Arc<dyn SecureGrantStore>,
    custody: Mutex<()>,
}
impl PlatformVault {
    pub fn new(store: Arc<dyn SecureGrantStore>) -> Self {
        Self {
            store,
            custody: Mutex::new(()),
        }
    }
    fn read(&self, r: &CredentialReference) -> Result<Option<ProviderGrant>> {
        self.store
            .read(r.as_str())?
            .map(crate::vault::decode)
            .transpose()
    }
}
impl Vault for PlatformVault {
    fn get(&self, r: &CredentialReference) -> Result<Option<ProviderGrant>> {
        let _guard = self
            .custody
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        self.read(r)
    }
    fn create(&self, r: &CredentialReference, g: &ProviderGrant) -> Result<()> {
        let _guard = self
            .custody
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        if self.read(r)?.is_some() {
            return Err(ProviderError::Conflict);
        }
        self.store.write(r.as_str(), &crate::vault::encode(g)?)
    }
    fn compare_exchange(&self, r: &CredentialReference, n: u64, g: &ProviderGrant) -> Result<()> {
        let _guard = self
            .custody
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        let prior = self.read(r)?.ok_or(ProviderError::CredentialUnavailable)?;
        if prior.secret().generation() != n
            || g.secret().generation() != n.checked_add(1).ok_or(ProviderError::Invalid)?
            || prior.configuration_digest() != g.configuration_digest()
            || prior.secret().cursor_key() != g.secret().cursor_key()
        {
            return Err(ProviderError::Conflict);
        }
        self.store.write(r.as_str(), &crate::vault::encode(g)?)
    }
    fn delete(&self, r: &CredentialReference) -> Result<()> {
        let _guard = self
            .custody
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        self.store.delete(r.as_str())
    }
}
