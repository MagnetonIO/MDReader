//! The sync client borrows the reader owner's connection. No sync path opens
//! another database, stores a raw library key, or changes the owner schema.
use mdreader_core::ReaderStore;
use mdreader_sync::{Result, SyncScope, SyncStore};

#[cfg(feature = "sync-native")]
pub mod incoming;

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

/// Native-only adapter over the existing reader owner, with a binding barrier held only
/// during synchronous DB work. Authentication/custody factories remain unimplemented.
#[cfg(feature = "sync-transport")]
pub struct TransportOwner<'a> {
    pub owner: &'a std::sync::Mutex<ReaderStore>,
    pub active_scope: &'a std::sync::RwLock<Option<SyncScope>>,
}
#[cfg(feature = "sync-transport")]
impl mdreader_sync::transport::Owner for TransportOwner<'_> {
    fn with_store<T>(
        &self,
        expected: &SyncScope,
        action: impl FnOnce(&mut SyncStore<'_>) -> Result<T>,
    ) -> mdreader_sync::transport::TransportResult<T> {
        use mdreader_sync::transport::TransportError;
        let binding = self
            .active_scope
            .read()
            .map_err(|_| TransportError::Authority)?;
        if binding.as_ref() != Some(expected) {
            return Err(TransportError::Authority);
        }
        let mut owner = self.owner.lock().map_err(|_| TransportError::Authority)?;
        Ok(with_store(&mut owner, expected.clone(), action)?)
    }
}
