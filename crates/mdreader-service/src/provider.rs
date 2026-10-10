//! Service-owned remote source registry and orchestration.
//!
//! All persistence borrows the existing library connection. Async I/O occurs
//! between short owner callbacks. Private remote caches are never local source
//! grants. Projection admission defaults to Unsupported until core can enforce
//! provider binding/generation authority in the same publication transaction.
mod capabilities;
pub(crate) mod catalog;
mod catalog_epoch;
mod coordinator;
mod creation;
mod discovery_metadata;
mod document_page;
mod draft_recovery;
pub use draft_recovery::{RecoveredDraft, RecoveredDraftBody, RecoveredDraftPage};
mod links;
mod listing;
mod metadata_observation;
mod page_spool;
pub use listing::{ListingCompleteness, MetadataListing};
pub(crate) mod search_coverage;
pub use links::{ProviderLinkCandidate, ProviderLinkKind, ProviderLinkResolution};
mod factory;
mod orchestration;
pub use coordinator::*;
pub use factory::*;
mod registry;
use mdreader_provider::*;
pub use orchestration::*;
pub use registry::*;
use rusqlite::{Connection, Transaction};
use std::sync::Mutex;
use uuid::Uuid;

/// The service owner serializes these closures; implementations must not open a
/// second SQLite writer. No closure is awaited by the orchestration layer.
pub trait Owner: Send + Sync {
    fn with_connection<T>(&self, operation: impl FnOnce(&mut Connection) -> Result<T>)
    -> Result<T>;
}
impl Owner for Mutex<mdreader_core::ReaderStore> {
    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        self.lock()
            .map_err(|_| ProviderError::Persistence)?
            .with_library_connection(operation)
    }
}
/// Useful for an already-owned connection, including synthetic acceptance tests.
impl Owner for Mutex<Connection> {
    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.lock().map_err(|_| ProviderError::Persistence)?;
        operation(&mut guard)
    }
}
/// In-memory authority callback for provider kernels. The owner remains the
/// only connection/writer; no credential or renderer handle enters this type.
pub struct RegistryAuthority<O: Owner>(pub std::sync::Arc<O>);
#[async_trait::async_trait]
impl<O: Owner> ProviderAuthority for RegistryAuthority<O> {
    async fn revalidate(&self, binding: &SourceBinding) -> Result<()> {
        self.0
            .with_connection(|connection| revalidate(connection, binding))
    }
    async fn store_page(
        &self,
        binding: &SourceBinding,
        kind: CursorKind,
        slot: Uuid,
        digest: [u8; 32],
        bytes: &[u8],
    ) -> Result<()> {
        self.0.with_connection(|connection| {
            page_spool::store(connection, binding, kind, slot, digest, bytes)
        })
    }
    async fn load_page(
        &self,
        binding: &SourceBinding,
        kind: CursorKind,
        slot: Uuid,
        digest: [u8; 32],
    ) -> Result<Vec<u8>> {
        self.0
            .with_connection(|connection| page_spool::load(connection, binding, kind, slot, digest))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionReceipt {
    pub job_id: Uuid,
    pub reader_source_id: SourceId,
    pub path: RecordKey,
    pub revision: OpaqueValue,
}
/// Trusted core integration, never a renderer implementation. Admission must
/// validate exact binding, stable ID, revision/hash and durable Markdown bytes,
/// and enqueue a generation/revision-bound index job in this SAME transaction.
/// Replays of an admitted revision must return the same receipt. Removal/revoke
/// must fence every read, search and pending index job, not just delete a cache.
pub trait ProjectionAdmission: Send + Sync {
    fn admit(
        &self,
        _transaction: &Transaction<'_>,
        _binding: &SourceBinding,
        _metadata: &Metadata,
        _document: &DocumentBody,
    ) -> Result<ProjectionReceipt> {
        Err(ProviderError::Unsupported)
    }
    fn remove(
        &self,
        _transaction: &Transaction<'_>,
        _binding: &SourceBinding,
        _key: &RecordKey,
    ) -> Result<()> {
        Err(ProviderError::Unsupported)
    }
    fn revoke(&self, _transaction: &Transaction<'_>, _binding: &SourceBinding) -> Result<()> {
        Err(ProviderError::Unsupported)
    }
}
#[derive(Default)]
pub struct UnsupportedProjection;
impl ProjectionAdmission for UnsupportedProjection {}

/// Concrete core bridge. Windows uses the protected native SQLite body cache;
/// unsupported hosts fail before creating cache paths.
pub struct CoreProjection(mdreader_core::ProviderProjection);
impl CoreProjection {
    #[cfg(any(unix, windows))]
    pub fn open(cache_root: &std::path::Path, expected_library: LibraryId) -> Result<Self> {
        if !cache_root.is_absolute() {
            return Err(ProviderError::Invalid);
        }
        Ok(Self(
            mdreader_core::ProviderProjection::open(cache_root, expected_library)
                .map_err(core_error)?,
        ))
    }
    #[cfg(not(any(unix, windows)))]
    pub fn open(_cache_root: &std::path::Path, _expected_library: LibraryId) -> Result<Self> {
        Err(ProviderError::Unsupported)
    }
}
fn core_error(error: mdreader_core::ReaderError) -> ProviderError {
    use mdreader_core::ReaderError;
    match error {
        ReaderError::Conflict => ProviderError::Conflict,
        ReaderError::Unavailable | ReaderError::Source(_) => ProviderError::Unavailable,
        ReaderError::Cancelled => ProviderError::Cancelled,
        ReaderError::Invalid(_) => ProviderError::Invalid,
        ReaderError::ModelUnavailable => ProviderError::Unsupported,
        ReaderError::Io(_) | ReaderError::Sqlite(_) | ReaderError::NewerDatabase(_) => {
            ProviderError::Persistence
        }
    }
}
impl ProjectionAdmission for CoreProjection {
    fn admit(
        &self,
        transaction: &Transaction<'_>,
        binding: &SourceBinding,
        metadata: &Metadata,
        document: &DocumentBody,
    ) -> Result<ProjectionReceipt> {
        let receipt = self
            .0
            .admit(transaction, binding, metadata, document)
            .map_err(core_error)?;
        Ok(ProjectionReceipt {
            job_id: receipt.job_id,
            reader_source_id: SourceId::new(receipt.reader_source_id)?,
            path: RecordKey::new(receipt.path)?,
            revision: OpaqueValue::new(receipt.revision)?,
        })
    }
    fn remove(
        &self,
        transaction: &Transaction<'_>,
        binding: &SourceBinding,
        key: &RecordKey,
    ) -> Result<()> {
        self.0.remove(transaction, binding, key).map_err(core_error)
    }
    fn revoke(&self, transaction: &Transaction<'_>, binding: &SourceBinding) -> Result<()> {
        self.0.revoke(transaction, binding).map_err(core_error)
    }
}

#[cfg(test)]
#[path = "provider/capability_tests.rs"]
mod capability_tests;
