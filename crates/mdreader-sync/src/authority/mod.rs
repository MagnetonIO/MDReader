//! Native-only, default-off OIDC and signing custody. No renderer DTO, provider grant,
//! production bootstrap, pairing, recovery or mobile custody is supplied here.
mod access;
mod capability;
mod custody;
mod factory;
mod http;
mod identity;
mod jwt;
mod keyring;
mod oidc;
mod projection;
mod request;
mod session;
mod signing;
mod vault;
pub use capability::NativeAuthority;
pub use custody::{CustodyRecord, PendingCustody, SyncCredentialReference};
pub use factory::{NativeAuthorityFactory, NativeLogin, VerifiedLogin};
pub use identity::VerifiedIdentity;
pub use keyring::SyncKeyringVault;
pub use oidc::{NativeBrowser, OidcClient, OidcEndpoints, PendingLogin, ReviewedOidcRegistration};
pub use projection::{ProjectionGuard, ProjectionPermit};
pub use session::SessionController;
use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};
pub use vault::{SyncVault, SyncVaultClient};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    #[error("native sync configuration rejected")]
    Configuration,
    #[error("native sync identity verification rejected")]
    Identity,
    #[error("native sync scope or custody binding rejected")]
    Scope,
    #[error("native sync authority is inactive or replaced")]
    Inactive,
    #[error("native sync authorization or custody is busy")]
    Busy,
    #[error("native sync operation timed out or was interrupted")]
    Interrupted,
    #[error("native sync response or custody bound exceeded")]
    Bounds,
    #[error("native sync custody unavailable or corrupt")]
    Custody,
    #[error("native sync custody generation conflict")]
    Generation,
    #[error("native sync device enrollment denied")]
    Enrollment,
    #[error("native sync bootstrap pairing or recovery remains unavailable")]
    LifecycleGated,
    #[error("native sync custody unsupported on this host")]
    Unsupported,
}
pub type AuthorityResult<T> = std::result::Result<T, AuthorityError>;
fn now() -> AuthorityResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|n| n.as_secs())
        .map_err(|_| AuthorityError::Identity)
}
fn redacted(name: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{name}([redacted])")
}
fn secret_equal(a: &[u8], b: &[u8]) -> bool {
    let mut different = a.len() ^ b.len();
    for (x, y) in a.iter().zip(b) {
        different |= usize::from(x ^ y);
    }
    different == 0
}
#[cfg(test)]
mod tests;
