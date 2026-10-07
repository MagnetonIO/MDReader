//! The sync client borrows the reader owner's connection. No sync path opens
//! another database, stores a raw library key, or changes the owner schema.
use mdreader_core::ReaderStore;
use mdreader_sync::{Result, SyncScope, SyncStore};

pub fn initialize(owner: &mut ReaderStore) -> Result<()> {
    owner.with_library_connection(mdreader_sync::migrate)
}
pub fn with_store<T>(
    owner: &mut ReaderStore,
    scope: SyncScope,
    operation: impl FnOnce(&mut SyncStore<'_>) -> Result<T>,
) -> Result<T> {
    owner.with_library_connection(|connection| {
        let mut sync = SyncStore::attach(connection, scope)?;
        operation(&mut sync)
    })
}
