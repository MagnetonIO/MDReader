//! Explicit profile preservation and selection. No SDK, source grants or vault.
//! Real native freeze participants must be registered before activation exists.
pub mod bundle;
pub mod candidate;
pub mod database_backup;
mod inventory;
pub mod lifecycle;
#[cfg(unix)]
mod live_database;
mod locator;
mod model;
#[cfg(unix)]
mod platform;
mod snapshot;
pub use inventory::{Inventory, InventorySelection};
pub use locator::{RelativeLocator, RootLocator};
pub use model::*;
pub use snapshot::{FreezeOwner, FrozenProfile, VerifiedSnapshot, stage, verify};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("profile transition is unsupported on this host")]
    Unsupported,
    #[error("profile transition input or schema is invalid")]
    Invalid,
    #[error("profile transition limit exceeded")]
    Bounds,
    #[error("profile or selection changed")]
    Conflict,
    #[error("profile writers are not quiescent")]
    NotQuiescent,
    #[error("profile inventory contains unclassified entries")]
    Unclassified,
    #[error("profile snapshot verification failed")]
    Integrity,
    #[error("profile transition storage is unavailable")]
    Storage,
    #[error("profile transition requires explicit reconciliation")]
    ReconciliationRequired,
}
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(all(test, unix))]
mod live_database_tests;
#[cfg(all(test, unix))]
mod tests;
