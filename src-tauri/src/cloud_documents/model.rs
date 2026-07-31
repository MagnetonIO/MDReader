use std::{fmt, time::Instant};

use serde::Serialize;
use specta::Type;

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

impl ProviderCursor {
    /// Creates an opaque cursor from source-owned bytes.
    #[cfg(test)]
    pub(crate) fn new(value: impl Into<Vec<u8>>) -> Self {
        Self(value.into())
    }
}

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

/// One provider-native change identity. Provider polling returns these opaque
/// bounded events before SQLite resolves only the matching published rows.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MarkdownChangeEvent {
    /// A record remains or became eligible for the Markdown materialization.
    Upsert { key: SourceRecordKey },
    /// A record was deleted or left the provider's configured source scope.
    Deleted { key: SourceRecordKey },
}

impl MarkdownChangeEvent {
    pub(crate) fn key(&self) -> &SourceRecordKey {
        match self {
            Self::Upsert { key } | Self::Deleted { key } => key,
        }
    }
}

/// A bounded provider change page before application-side metadata resolution.
pub(crate) struct MarkdownChangePage {
    pub(crate) changes: Vec<MarkdownChangeEvent>,
    pub(crate) next_cursor: ProviderCursor,
    pub(crate) has_more: bool,
    pub(crate) observed_at_ms: i64,
}

/// One published metadata revision supplied to a provider-native change
/// classifier. It contains no body or credential material.
#[derive(Clone)]
pub(crate) struct KnownMarkdownMetadata {
    pub(crate) key: SourceRecordKey,
    pub(crate) name: String,
    pub(crate) display_path: String,
    pub(crate) media_type: Option<String>,
    pub(crate) modified_at_ms: Option<i64>,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) revision: ProviderRevision,
    pub(crate) deleted: bool,
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
    pub(crate) query_id: uuid::Uuid,
    pub(crate) deadline: Instant,
    pub(crate) page_rows: usize,
    pub(crate) byte_limit: usize,
    pub(crate) cancellation: catdb_sdk::connectors::CancellationToken,
}

/// Closed application-level outcomes for cloud document operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "kebab-case")]
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
    RateLimited {
        #[specta(type = Option<f64>)]
        retry_after_ms: Option<u64>,
    },
    ResetCursor,
    NotFound,
    Conflict,
    TooLarge,
    InvalidUtf8,
    IdentityMismatch,
    ConnectorUnavailable,
    ProtocolFailed,
    WriteCapabilityPendingAcceptance,
    WriteOutcomeUnknown,
    LocalPublicationPending,
}

/// Provenance of the body returned by one cloud Markdown point read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudCacheState {
    /// The provider returned and the native cache published this exact revision.
    ProviderFresh,
    /// The presently published revision was already present in the native cache.
    Cached,
    /// The presently published revision was served from cache while offline.
    CachedOffline,
}

/// One validated Markdown body returned through the native desktop boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub(crate) struct CloudMarkdownDocument {
    /// Opaque renderer identity derived from Library, source, and record identity.
    pub(crate) document_key: String,
    /// Published display title.
    pub(crate) title: String,
    /// Validated UTF-8 Markdown content.
    pub(crate) content: String,
    /// Short-lived opaque authority for guarded editing.
    pub(crate) edit_token: Option<String>,
    /// Whether this exact revision came from the provider or native cache.
    pub(crate) cache_state: CloudCacheState,
    /// Provider modification timestamp retained from published metadata.
    #[specta(type = Option<f64>)]
    pub(crate) modified_at_ms: Option<i64>,
}

/// Closed renderer-visible result of one guarded cloud Markdown save.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloudMarkdownSaveOutcome {
    /// The provider accepted the guarded revision and local receipt durability
    /// was established.
    Saved {
        /// Fresh authority anchored to the accepted provider revision.
        edit_token: String,
        /// Provider modification time, when the provider supplies one.
        #[specta(type = Option<f64>)]
        modified_at_ms: Option<i64>,
    },
    /// The provider record no longer has the revision that was opened.
    Conflict {},
}

#[async_trait::async_trait]
pub(crate) trait MarkdownDocumentSource: Send + Sync {
    fn source_id(&self) -> &catdb_sdk::connectors::SourceId;
    async fn bootstrap_recent(
        &self,
        _context: &CloudIoContext,
    ) -> Result<Option<MarkdownMetadataPage>, CloudDocumentError> {
        Ok(None)
    }
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
    async fn materialize_changes(
        &self,
        changes: &[MarkdownChangeEvent],
        known: &[KnownMarkdownMetadata],
        context: &CloudIoContext,
    ) -> Result<Vec<MarkdownMetadata>, CloudDocumentError>;
}
