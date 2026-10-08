//! Subprocess-only in-memory keystore; production SyncKeyringVault performs
//! the real custody codec/admission/CAS. Never changes the host OS keychain.
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi, CredentialPersistence};
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;
type Rows = Arc<Mutex<BTreeMap<(String, String), Zeroizing<Vec<u8>>>>>;
#[derive(Default)]
struct Builder(Rows);
struct Entry {
    rows: Rows,
    key: (String, String),
}
impl CredentialApi for Entry {
    fn set_secret(&self, value: &[u8]) -> keyring::Result<()> {
        self.rows
            .lock()
            .unwrap()
            .insert(self.key.clone(), Zeroizing::new(value.to_vec()));
        Ok(())
    }
    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.rows
            .lock()
            .unwrap()
            .get(&self.key)
            .map(|v| v.to_vec())
            .ok_or(keyring::Error::NoEntry)
    }
    fn delete_credential(&self) -> keyring::Result<()> {
        self.rows
            .lock()
            .unwrap()
            .remove(&self.key)
            .map(|_| ())
            .ok_or(keyring::Error::NoEntry)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl CredentialBuilderApi for Builder {
    fn build(
        &self,
        target: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<Credential>> {
        assert!(target.is_none());
        Ok(Box::new(Entry {
            rows: self.0.clone(),
            key: (service.into(), user.into()),
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn persistence(&self) -> CredentialPersistence {
        CredentialPersistence::ProcessOnly
    }
}
pub(super) fn install() {
    keyring::set_default_credential_builder(Box::<Builder>::default());
}
