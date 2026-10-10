//! MDReader-owned provider boundary. Credentials, platform handles, generic query
//! engines and SQLite ownership never enter document/source DTOs.
mod context;
pub mod creation;
mod cursor;
pub mod journal;
mod model;
pub mod transport;
use async_trait::async_trait;
pub use context::*;
pub use cursor::*;
pub use model::*;

pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CURSOR_BYTES: usize = 64 * 1024;
pub const MAX_PAGE_ROWS: usize = 256;
pub const MAX_SELECTIONS: usize = 512;
pub fn validate_record_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}
pub fn body_hash(body: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(body).into()
}
pub type Result<T> = std::result::Result<T, ProviderError>;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderError {
    #[error("Invalid provider input")]
    Invalid,
    #[error("Provider capability is unsupported")]
    Unsupported,
    #[error("Provider authorization was revoked")]
    Unauthorized,
    #[error("Provider reconnect is required")]
    ReconnectRequired,
    #[error("Provider credential unavailable")]
    CredentialUnavailable,
    #[error("Provider scope is unavailable")]
    Unavailable,
    #[error("Provider document not found")]
    NotFound,
    #[error("Provider revision changed")]
    Conflict,
    #[error("Provider operation exceeded a bound")]
    TooLarge,
    #[error("Provider body is not UTF-8")]
    InvalidUtf8,
    #[error("Provider identity mismatch")]
    IdentityMismatch,
    #[error("Provider protocol failed")]
    Protocol,
    #[error("Provider operation timed out")]
    Timeout,
    #[error("Provider operation was cancelled")]
    Cancelled,
    #[error("Provider operation rate limited")]
    RateLimited { retry_after_ms: Option<u64> },
    #[error("Provider cursor requires rebuild")]
    ResetCursor,
    #[error("Provider write outcome is unknown")]
    UnknownWrite { operation_id: uuid::Uuid },
    #[error("Local publication is pending")]
    LocalPublicationPending,
    #[error("Provider persistence failed")]
    Persistence,
}
/// Read-only folder metadata; never a source grant or document body.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderFolder {
    pub id: RecordKey,
    pub name: DisplayText,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderPage {
    pub folders: Vec<ProviderFolder>,
    pub next: Option<ProviderCursor>,
}
/// I/O happens outside the service's writer lock. Each operation is scoped to
/// the immutable admitted binding; implementations revalidate it at effects.
#[async_trait]
pub trait DocumentProvider: Send + Sync {
    fn descriptor(&self) -> &SourceDescriptor;
    /// Create a new Markdown file at the saved folder root, never overwrite.
    async fn guarded_create(
        &self,
        _permit: creation::CreatePermit,
        _body: &MarkdownBytes,
        _context: &IoContext,
    ) -> Result<Metadata> {
        Err(ProviderError::Unsupported)
    }
    async fn list(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<MetadataPage>;
    /// Called only after this provider's list page was durably staged by its
    /// owner. Retained metadata aids change reconciliation; it grants no reads.
    fn retain_metadata_observation(&self, _rows: &[Metadata], _replaces: bool) -> Result<()> {
        Ok(())
    }
    /// Called after atomic metadata delta staging; no body or read authority.
    fn retain_metadata_changes(&self, _rows: &[Metadata], _deleted: &[RecordKey]) -> Result<()> {
        Ok(())
    }
    async fn folders(
        &self,
        _folder: &RecordKey,
        _cursor: Option<&ProviderCursor>,
        _context: &IoContext,
    ) -> Result<FolderPage> {
        Err(ProviderError::Unsupported)
    }
    async fn read(&self, key: &RecordKey, context: &IoContext) -> Result<DocumentBody>;
    async fn metadata(&self, key: &RecordKey, context: &IoContext) -> Result<Metadata>;
    async fn start_changes(&self, context: &IoContext) -> Result<ProviderCursor>;
    async fn changes(&self, cursor: &ProviderCursor, context: &IoContext) -> Result<ChangeOutcome>;
    async fn resolve_asset(
        &self,
        _document: &RecordKey,
        _reference: &str,
        _context: &IoContext,
    ) -> Result<AssetBody> {
        Err(ProviderError::Unsupported)
    }
    /// Only a durably committed, single-dispatch permit can authorize mutation.
    async fn guarded_write(
        &self,
        permit: journal::DispatchPermit,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<WriteReceipt>;
}
/// Service supplied admission/epoch authority, checked before and after effects.
#[async_trait]
pub trait ProviderAuthority: Send + Sync {
    async fn revalidate(&self, binding: &SourceBinding) -> Result<()>;
    /// Freeze a bounded overflow page in the existing owner; no second writer.
    /// The singleflight traversal retains one immutable slot per binding/kind.
    async fn store_page(
        &self,
        _binding: &SourceBinding,
        _kind: CursorKind,
        _slot: uuid::Uuid,
        _digest: [u8; 32],
        _bytes: &[u8],
    ) -> Result<()> {
        Err(ProviderError::Unsupported)
    }
    /// Missing or replaced exact page must reset traversal, never skip its tail.
    async fn load_page(
        &self,
        _binding: &SourceBinding,
        _kind: CursorKind,
        _slot: uuid::Uuid,
        _digest: [u8; 32],
    ) -> Result<Vec<u8>> {
        Err(ProviderError::Unsupported)
    }
}
pub const MAX_PAGE_SPOOL_BYTES: usize = 8 * 1024 * 1024;

/// Provider content hashes are algorithm-tagged integrity checks, never write authority.
pub fn verify_content_hash(body: &[u8], hash: &ContentHash) -> Result<()> {
    let actual: Vec<u8> = match hash.algorithm() {
        HashAlgorithm::Sha256 => body_hash(body).to_vec(),
        HashAlgorithm::Md5 => {
            use md5::{Digest, Md5};
            Md5::digest(body).to_vec()
        }
        HashAlgorithm::DropboxContentHash => {
            let blocks: Vec<u8> = body.chunks(4 * 1024 * 1024).flat_map(body_hash).collect();
            body_hash(&blocks).to_vec()
        }
    };
    if actual != hash.value() {
        return Err(ProviderError::Protocol);
    }
    Ok(())
}
/// Admission callbacks share the I/O deadline; a hung platform callback cannot
/// make a source operation unbounded. Post-dispatch failures become Unknown.
pub async fn revalidate_authority(
    authority: &dyn ProviderAuthority,
    binding: &SourceBinding,
    context: &IoContext,
    mutation: Option<uuid::Uuid>,
) -> Result<()> {
    context.check().map_err(|error| {
        mutation
            .map(|operation_id| ProviderError::UnknownWrite { operation_id })
            .unwrap_or(error)
    })?;
    let result = tokio::select! {biased;_=context.cancellation().cancelled()=>Err(ProviderError::Cancelled),result=tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),authority.revalidate(binding))=>result.map_err(|_|ProviderError::Timeout)?};
    result.map_err(|error| {
        mutation
            .map(|operation_id| ProviderError::UnknownWrite { operation_id })
            .unwrap_or(error)
    })
}
