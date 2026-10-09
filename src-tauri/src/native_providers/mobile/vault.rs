use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_credentials::SecureGrantStore;
use mdreader_provider::{ProviderError, Result};
use zeroize::Zeroizing;
pub(super) struct Store {
    pub app: tauri::AppHandle,
    pub namespace: String,
}
impl SecureGrantStore for Store {
    fn read(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
        mdreader_mobile_transfer::vault_read(&self.app, &self.namespace, reference)
            .map_err(|_| ProviderError::CredentialUnavailable)?
            .map(|encoded| {
                let bytes = Zeroizing::new(
                    STANDARD
                        .decode(encoded.as_bytes())
                        .map_err(|_| ProviderError::CredentialUnavailable)?,
                );
                if bytes.len() > 128 * 1024 {
                    return Err(ProviderError::TooLarge);
                }
                Ok(Zeroizing::new(
                    std::str::from_utf8(&bytes)
                        .map_err(|_| ProviderError::CredentialUnavailable)?
                        .to_owned(),
                ))
            })
            .transpose()
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        if value.len() > 128 * 1024 {
            return Err(ProviderError::TooLarge);
        }
        let bytes = Zeroizing::new(STANDARD.encode(value));
        mdreader_mobile_transfer::vault_write(&self.app, &self.namespace, reference, &bytes)
            .map_err(|_| ProviderError::CredentialUnavailable)
    }
    fn delete(&self, reference: &str) -> Result<()> {
        mdreader_mobile_transfer::vault_delete(&self.app, &self.namespace, reference)
            .map_err(|_| ProviderError::CredentialUnavailable)
    }
}
