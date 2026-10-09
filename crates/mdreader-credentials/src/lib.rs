//! Native-only verified grant and credential custody. No token/history is a
//! renderer DTO, SQLite row, source body, log field or synchronized document.
mod grant;
mod platform;
mod vault;
pub use grant::*;
pub use platform::{PlatformVault, SecureGrantStore};
pub use vault::*;
pub fn random_key() -> mdreader_provider::Result<[u8; 32]> {
    let mut key = [0; 32];
    getrandom::fill(&mut key)
        .map_err(|_| mdreader_provider::ProviderError::CredentialUnavailable)?;
    Ok(key)
}
