//! Client-only encrypted sync primitives. The reader service owns the SQLite connection.
use thiserror::Error;
#[derive(Debug, Error)]
pub enum SyncError {
    #[error("invalid encrypted sync input")]
    Invalid,
    #[error("encrypted sync authentication failed")]
    Authentication,
    #[error("encrypted sync bound exceeded")]
    Bounds,
    #[error("encrypted sync scope mismatch")]
    Scope,
    #[error("encrypted sync operation or revision reused")]
    Reused,
    #[error("encrypted sync state is not ready")]
    NotReady,
    #[error("encrypted sync migration version is unsupported")]
    Schema,
    #[error("encrypted sync persistence failed")]
    Database(#[from] rusqlite::Error),
    #[error("secure entropy unavailable")]
    Entropy,
}
pub type Result<T> = std::result::Result<T, SyncError>;
pub mod codec;
pub mod crypto;
pub mod store;
pub use crypto::*;
pub use store::*;
