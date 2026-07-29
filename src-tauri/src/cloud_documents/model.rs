use std::{fmt, time::Instant};

/// Opaque source-owned document key. It is never suitable for renderer output
/// or diagnostic text.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct SourceRecordKey(pub(super) String);

impl SourceRecordKey {
    pub(super) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(super) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SourceRecordKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceRecordKey")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// Opaque provider cursor.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderCursor(pub(super) Vec<u8>);

impl fmt::Debug for ProviderCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCursor")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// Provider revision captured from a source operation.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderRevision {
    pub(crate) opaque: String,
    pub(crate) content_hash: Option<[u8; 32]>,
}

impl fmt::Debug for ProviderRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRevision")
            .field("opaque_bytes", &self.opaque.len())
            .field("has_content_hash", &self.content_hash.is_some())
            .finish()
    }
}

pub(crate) struct MarkdownMetadata {
    pub(crate) key: SourceRecordKey,
    pub(crate) name: String,
    pub(crate) display_path: String,
    pub(crate) media_type: Option<String>,
    pub(crate) modified_at_ms: Option<i64>,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) revision: ProviderRevision,
    pub(crate) deleted: bool,
}

pub(crate) struct MarkdownMetadataPage {
    pub(crate) rows: Vec<MarkdownMetadata>,
    pub(crate) next_cursor: Option<ProviderCursor>,
    pub(crate) observed_at_ms: i64,
}

pub(crate) struct MarkdownChangePage {
    pub(crate) changes: Vec<MarkdownMetadata>,
    pub(crate) next_cursor: ProviderCursor,
    pub(crate) has_more: bool,
    pub(crate) observed_at_ms: i64,
}

pub(crate) struct MarkdownDocumentRevision {
    pub(crate) key: SourceRecordKey,
    pub(crate) revision: ProviderRevision,
    pub(crate) body: Vec<u8>,
    pub(crate) observed_at_ms: i64,
}

pub(crate) struct MarkdownWriteReceipt {
    pub(crate) key: SourceRecordKey,
    pub(crate) revision: ProviderRevision,
    pub(crate) modified_at_ms: Option<i64>,
    pub(crate) size_bytes: u64,
}

/// The bounded, cancellation-aware source I/O context.
pub(crate) struct CloudIoContext {
    pub(crate) deadline: Instant,
    pub(crate) page_rows: usize,
    pub(crate) byte_limit: usize,
    pub(crate) cancellation: catdb_connectors::CancellationToken,
}

/// Closed application-level outcomes for cloud document operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CloudDocumentError {
    Unauthorized,
    ReconnectRequired,
    CredentialUnavailable,
    SourcePaused,
    SourceDisconnected,
    SyncInProgress,
    OfflineUncached,
    Timeout,
    Cancelled,
    RateLimited { retry_after_ms: Option<u64> },
    ResetCursor,
    NotFound,
    Conflict,
    TooLarge,
    InvalidUtf8,
    IdentityMismatch,
    ProtocolFailed,
    WriteCapabilityPendingAcceptance,
    WriteOutcomeUnknown,
    LocalPublicationPending,
}

#[async_trait::async_trait]
pub(crate) trait MarkdownDocumentSource: Send + Sync {
    fn source_id(&self) -> &catdb_connectors::SourceId;
    async fn start_change_tracking(
        &self,
        context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError>;
    async fn list_page(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError>;
    async fn read_document(
        &self,
        key: &SourceRecordKey,
        context: &CloudIoContext,
    ) -> Result<MarkdownDocumentRevision, CloudDocumentError>;
    async fn write_document(
        &self,
        key: &SourceRecordKey,
        expected: &ProviderRevision,
        body: &[u8],
        context: &CloudIoContext,
    ) -> Result<MarkdownWriteReceipt, CloudDocumentError>;
    async fn read_changes(
        &self,
        cursor: &ProviderCursor,
        context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError>;
}
