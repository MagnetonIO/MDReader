//! Android AES-GCM ciphertext is private/no-backup; the wrapping key belongs to
//! Android Keystore. Core owns exact record codec and generation/material CAS.
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_sync::authority::{AuthorityError, AuthorityResult, SecureByteStore};
use zeroize::Zeroizing;
pub(super) struct Vault {
    pub app: tauri::AppHandle,
    pub namespace: String,
}
impl SecureByteStore for Vault {
    fn read(&self, reference: &str) -> AuthorityResult<Option<Zeroizing<Vec<u8>>>> {
        mdreader_mobile_transfer::vault_read(&self.app, &self.namespace, reference)
            .map_err(|_| AuthorityError::Custody)?
            .map(|value| {
                STANDARD
                    .decode(value.as_bytes())
                    .map(Zeroizing::new)
                    .map_err(|_| AuthorityError::Custody)
            })
            .transpose()
    }
    fn write(&self, reference: &str, bytes: &[u8]) -> AuthorityResult<()> {
        if bytes.len() > 16384 {
            return Err(AuthorityError::Bounds);
        }
        let encoded = Zeroizing::new(STANDARD.encode(bytes));
        mdreader_mobile_transfer::vault_write(&self.app, &self.namespace, reference, &encoded)
            .map_err(|_| AuthorityError::Custody)
    }
}
