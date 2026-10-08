//! Brief writer contention should not leave the initial catalog visibly failed.
use super::{CatalogError, CatalogSource, Result, provider_error};
use crate::provider;
use mdreader_core::ReaderStore;
use std::{
    sync::{Mutex, TryLockError},
    time::{Duration, Instant},
};

/// Called on the request's blocking worker after releasing the query owner.
/// Keep the observation bounded without holding any owner while waiting.
pub(super) fn providers(owner: &Mutex<ReaderStore>) -> Result<Vec<CatalogSource>> {
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match owner.try_lock() {
            Ok(mut owner) => {
                return owner
                    .with_library_connection(provider::catalog::catalog_sources)
                    .map_err(provider_error);
            }
            Err(TryLockError::Poisoned(_)) => return Err(CatalogError::Persistence),
            Err(TryLockError::WouldBlock) => {}
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(CatalogError::Busy)?;
        std::thread::sleep(remaining.min(Duration::from_millis(5)));
    }
}
