use std::{sync::Arc, time::SystemTime};

use catdb_connectors::{DataSource, ReadContext, SourceCursor, SourceError, SourceId, SourcePage};
use catdb_query::{EntityQuery, EntityTarget};

use super::model::{
    CloudDocumentError, CloudIoContext, MarkdownChangePage, MarkdownDocumentRevision,
    MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage, MarkdownWriteReceipt,
    ProviderCursor, ProviderRevision, SourceRecordKey,
};
use crate::cloud_accounts::{
    CloudAuthorizationProfile, CloudCredentialResolver, CloudSourceError,
    NativeMarkdownWriteAcceptance,
};

const CLOUD_DOCUMENT_ENTITY: &str = "mdreader.Document";

/// Publication state for the guarded provider-write primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkdownWriteCapability {
    ReconnectRequired,
    PendingAcceptance,
    Accepted,
}

impl MarkdownWriteCapability {
    pub(crate) fn from_profile(
        profile: CloudAuthorizationProfile,
        acceptance: NativeMarkdownWriteAcceptance,
    ) -> Self {
        match profile {
            CloudAuthorizationProfile::ReadOnlyV1 => Self::ReconnectRequired,
            CloudAuthorizationProfile::MarkdownReadWriteV2 => match acceptance {
                NativeMarkdownWriteAcceptance::Pending => Self::PendingAcceptance,
                NativeMarkdownWriteAcceptance::Accepted => Self::Accepted,
            },
        }
    }

    pub(crate) fn error(self) -> CloudDocumentError {
        match self {
            Self::ReconnectRequired => CloudDocumentError::ReconnectRequired,
            Self::PendingAcceptance => CloudDocumentError::WriteCapabilityPendingAcceptance,
            Self::Accepted => CloudDocumentError::ProtocolFailed,
        }
    }
}

/// Source admission authority that must hold before and after a provider
/// mutation. Test-only implementations model lifecycle loss at the effect
/// boundary without reimplementing a provider request.
#[async_trait::async_trait]
pub(super) trait DocumentLifecycle: Send + Sync {
    async fn validate_document_authority(&self) -> Result<(), CloudDocumentError>;
}

#[async_trait::async_trait]
impl DocumentLifecycle for CloudCredentialResolver {
    async fn validate_document_authority(&self) -> Result<(), CloudDocumentError> {
        CloudCredentialResolver::validate_document_authority(self)
            .await
            .map_err(map_lifecycle_error)
    }
}

/// One provider-owned guarded Markdown mutation. Production implementations
/// delegate to Task 2's source methods; this boundary does not own protocol or
/// provider validation.
#[async_trait::async_trait]
pub(super) trait GuardedMarkdownDocumentOperations: Send + Sync {
    fn source_id(&self) -> &SourceId;

    async fn update_markdown(
        &self,
        key: &str,
        expected_revision: &str,
        body: &[u8],
        context: &ReadContext,
    ) -> Result<GuardedMarkdownWriteReceipt, CloudDocumentError>;
}

/// Provider-confirmed identity returned from a guarded mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GuardedMarkdownWriteReceipt {
    record_key: String,
    revision: String,
}

impl GuardedMarkdownWriteReceipt {
    pub(super) fn new(record_key: impl Into<String>, revision: impl Into<String>) -> Self {
        Self {
            record_key: record_key.into(),
            revision: revision.into(),
        }
    }
}

#[async_trait::async_trait]
impl GuardedMarkdownDocumentOperations for catdb_connector_google_drive::DriveSource {
    fn source_id(&self) -> &SourceId {
        self.id()
    }

    async fn update_markdown(
        &self,
        key: &str,
        expected_revision: &str,
        body: &[u8],
        context: &ReadContext,
    ) -> Result<GuardedMarkdownWriteReceipt, CloudDocumentError> {
        let revision = catdb_connector_google_drive::DriveRevision::new(expected_revision)
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let receipt = self
            .update_markdown(key, &revision, body, context)
            .await
            .map_err(map_drive_document_error)?;
        Ok(GuardedMarkdownWriteReceipt::new(
            receipt.record_key(),
            receipt.revision().as_str(),
        ))
    }
}

#[async_trait::async_trait]
impl GuardedMarkdownDocumentOperations for catdb_connector_dropbox::DropboxSource {
    fn source_id(&self) -> &SourceId {
        self.id()
    }

    async fn update_markdown(
        &self,
        key: &str,
        expected_revision: &str,
        body: &[u8],
        context: &ReadContext,
    ) -> Result<GuardedMarkdownWriteReceipt, CloudDocumentError> {
        let revision = catdb_connector_dropbox::DropboxRevision::new(expected_revision)
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let receipt = self
            .update_markdown(key, &revision, body, context)
            .await
            .map_err(map_dropbox_document_error)?;
        Ok(GuardedMarkdownWriteReceipt::new(
            receipt.record_key(),
            receipt.revision().as_str(),
        ))
    }
}

pub(crate) fn map_lifecycle_error(error: CloudSourceError) -> CloudDocumentError {
    match error {
        CloudSourceError::SourcePaused => CloudDocumentError::SourcePaused,
        CloudSourceError::SourceNotFound
        | CloudSourceError::QueryRetired
        | CloudSourceError::QueryCatalogInvalid => CloudDocumentError::SourceDisconnected,
        CloudSourceError::CredentialMissing | CloudSourceError::CredentialUnavailable => {
            CloudDocumentError::CredentialUnavailable
        }
        CloudSourceError::QueryCancelled | CloudSourceError::OauthCancelled => {
            CloudDocumentError::Cancelled
        }
        CloudSourceError::QueryDeadlineExceeded | CloudSourceError::OauthTimeout => {
            CloudDocumentError::Timeout
        }
        CloudSourceError::OauthRateLimited { retry_after_ms } => CloudDocumentError::RateLimited {
            retry_after_ms: retry_after_ms.map(u64::from),
        },
        _ => CloudDocumentError::ProtocolFailed,
    }
}

fn map_source_error(error: SourceError) -> CloudDocumentError {
    match error {
        SourceError::Cancelled | SourceError::SourceCancelled { .. } => {
            CloudDocumentError::Cancelled
        }
        SourceError::DeadlineExceeded | SourceError::StatementTimeout { .. } => {
            CloudDocumentError::Timeout
        }
        SourceError::Authentication { .. } => CloudDocumentError::Unauthorized,
        SourceError::RateLimited { retry_after_ms, .. } => {
            CloudDocumentError::RateLimited { retry_after_ms }
        }
        SourceError::ResourceLimit { .. }
        | SourceError::RowBudgetExceeded
        | SourceError::ByteBudgetExceeded => CloudDocumentError::TooLarge,
        SourceError::InvalidCursor | SourceError::CursorTooLarge { .. } => {
            CloudDocumentError::ResetCursor
        }
        _ => CloudDocumentError::ProtocolFailed,
    }
}

pub(crate) fn map_drive_document_error(
    error: catdb_connector_google_drive::DriveDocumentError,
) -> CloudDocumentError {
    use catdb_connector_google_drive::DriveDocumentError;

    match error {
        DriveDocumentError::NotFound { .. } => CloudDocumentError::NotFound,
        DriveDocumentError::Conflict { .. } => CloudDocumentError::Conflict,
        DriveDocumentError::TooLarge { .. } => CloudDocumentError::TooLarge,
        DriveDocumentError::InvalidUtf8 { .. } => CloudDocumentError::InvalidUtf8,
        DriveDocumentError::IdentityMismatch { .. } => CloudDocumentError::IdentityMismatch,
        DriveDocumentError::Protocol { .. } => CloudDocumentError::ProtocolFailed,
        DriveDocumentError::Authentication { .. } => CloudDocumentError::Unauthorized,
        DriveDocumentError::Timeout => CloudDocumentError::Timeout,
        DriveDocumentError::Cancelled => CloudDocumentError::Cancelled,
        DriveDocumentError::RateLimited { retry_after_ms, .. } => {
            CloudDocumentError::RateLimited { retry_after_ms }
        }
        DriveDocumentError::WriteOutcomeUnknown { .. } => CloudDocumentError::WriteOutcomeUnknown,
        DriveDocumentError::Source { error } => map_source_error(error),
    }
}

pub(crate) fn map_dropbox_document_error(
    error: catdb_connector_dropbox::DropboxDocumentError,
) -> CloudDocumentError {
    use catdb_connector_dropbox::DropboxDocumentError;

    match error {
        DropboxDocumentError::NotFound { .. } => CloudDocumentError::NotFound,
        DropboxDocumentError::Conflict { .. } => CloudDocumentError::Conflict,
        DropboxDocumentError::TooLarge { .. } => CloudDocumentError::TooLarge,
        DropboxDocumentError::InvalidUtf8 { .. } => CloudDocumentError::InvalidUtf8,
        DropboxDocumentError::IdentityMismatch { .. } => CloudDocumentError::IdentityMismatch,
        DropboxDocumentError::Protocol { .. } => CloudDocumentError::ProtocolFailed,
        DropboxDocumentError::Authentication { .. } => CloudDocumentError::Unauthorized,
        DropboxDocumentError::Timeout => CloudDocumentError::Timeout,
        DropboxDocumentError::Cancelled => CloudDocumentError::Cancelled,
        DropboxDocumentError::RateLimited { retry_after_ms, .. } => {
            CloudDocumentError::RateLimited { retry_after_ms }
        }
        DropboxDocumentError::WriteOutcomeUnknown { .. } => CloudDocumentError::WriteOutcomeUnknown,
        DropboxDocumentError::Source { error } => map_source_error(error),
    }
}

fn read_context(context: &CloudIoContext) -> Result<ReadContext, CloudDocumentError> {
    ReadContext::new(
        uuid::Uuid::new_v4(),
        context.deadline,
        context.page_rows,
        context.byte_limit,
        context.page_rows,
        context.cancellation.clone(),
    )
    .map_err(map_source_error)
}

async fn check_lifecycle(lifecycle: &dyn DocumentLifecycle) -> Result<(), CloudDocumentError> {
    lifecycle.validate_document_authority().await
}

async fn guarded_write(
    lifecycle: &dyn DocumentLifecycle,
    operations: &dyn GuardedMarkdownDocumentOperations,
    capability: MarkdownWriteCapability,
    key: &SourceRecordKey,
    expected: &ProviderRevision,
    body: &[u8],
    context: &CloudIoContext,
) -> Result<MarkdownWriteReceipt, CloudDocumentError> {
    lifecycle.validate_document_authority().await?;
    if capability != MarkdownWriteCapability::Accepted {
        return Err(capability.error());
    }
    let receipt = operations
        .update_markdown(
            key.as_str(),
            expected.opaque.as_str(),
            body,
            &read_context(context)?,
        )
        .await?;
    if lifecycle.validate_document_authority().await.is_err() {
        return Err(CloudDocumentError::WriteOutcomeUnknown);
    }
    Ok(MarkdownWriteReceipt {
        key: SourceRecordKey::new(receipt.record_key),
        revision: ProviderRevision {
            opaque: receipt.revision,
            content_hash: None,
        },
        modified_at_ms: None,
        size_bytes: body
            .len()
            .try_into()
            .map_err(|_| CloudDocumentError::TooLarge)?,
    })
}

async fn physical_page<S: DataSource + ?Sized>(
    source: &S,
    fields: &[String],
    cursor: Option<&ProviderCursor>,
    context: &CloudIoContext,
) -> Result<SourcePage, CloudDocumentError> {
    let query = EntityQuery {
        target: EntityTarget::Entity(CLOUD_DOCUMENT_ENTITY.to_string()),
        filters: Vec::new(),
    };
    let plan = source.compile(&query, fields).map_err(map_source_error)?;
    let cursor = cursor
        .map(|cursor| SourceCursor::new(cursor.0.clone()).map_err(map_source_error))
        .transpose()?;
    source
        .execute_page(&plan, cursor.as_ref(), &read_context(context)?)
        .await
        .map_err(map_source_error)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn string(value: &serde_json::Value, field: &str) -> Result<String, CloudDocumentError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or(CloudDocumentError::ProtocolFailed)
}

fn optional_i64(value: &serde_json::Value, field: &str) -> Option<i64> {
    value.get(field).and_then(serde_json::Value::as_i64)
}

fn optional_u64(value: &serde_json::Value, field: &str) -> Option<u64> {
    value.get(field).and_then(serde_json::Value::as_u64)
}

/// Native Drive adapter. Its resolver retains the admitted credential and
/// rechecks source identity before and after every provider effect.
pub(crate) struct DriveMarkdownDocumentSource {
    source: Option<Arc<catdb_connector_google_drive::DriveSource>>,
    lifecycle: Arc<dyn DocumentLifecycle>,
    write_operations: Arc<dyn GuardedMarkdownDocumentOperations>,
    write_capability: MarkdownWriteCapability,
}

impl DriveMarkdownDocumentSource {
    pub(crate) fn new(
        source: Arc<catdb_connector_google_drive::DriveSource>,
        resolver: Arc<CloudCredentialResolver>,
        profile: CloudAuthorizationProfile,
        acceptance: NativeMarkdownWriteAcceptance,
    ) -> Self {
        Self {
            source: Some(source.clone()),
            lifecycle: resolver,
            write_operations: source,
            write_capability: MarkdownWriteCapability::from_profile(profile, acceptance),
        }
    }

    #[cfg(test)]
    pub(super) fn with_test_operations(
        write_operations: Arc<dyn GuardedMarkdownDocumentOperations>,
        lifecycle: Arc<dyn DocumentLifecycle>,
        profile: CloudAuthorizationProfile,
        acceptance: NativeMarkdownWriteAcceptance,
    ) -> Self {
        Self {
            source: None,
            lifecycle,
            write_operations,
            write_capability: MarkdownWriteCapability::from_profile(profile, acceptance),
        }
    }

    fn source(&self) -> Result<&catdb_connector_google_drive::DriveSource, CloudDocumentError> {
        self.source
            .as_deref()
            .ok_or(CloudDocumentError::ProtocolFailed)
    }

    fn metadata(row: catdb_connectors::SourceRow) -> Result<MarkdownMetadata, CloudDocumentError> {
        let name = string(&row.value, "name")?;
        Ok(MarkdownMetadata {
            key: SourceRecordKey::new(row.key.clone()),
            display_path: name.clone(),
            name,
            media_type: row
                .value
                .get("mimeType")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            modified_at_ms: optional_i64(&row.value, "modifiedAt"),
            size_bytes: optional_u64(&row.value, "size"),
            revision: ProviderRevision {
                opaque: row
                    .value
                    .get("checksum")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&row.key)
                    .to_string(),
                content_hash: None,
            },
            deleted: false,
        })
    }
}

#[async_trait::async_trait]
impl MarkdownDocumentSource for DriveMarkdownDocumentSource {
    fn source_id(&self) -> &SourceId {
        self.write_operations.source_id()
    }

    async fn start_change_tracking(
        &self,
        context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let cursor = self
            .source()?
            .start_change_tracking(&read_context(context)?)
            .await
            .map_err(map_source_error)?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        Ok(ProviderCursor(cursor.as_bytes().to_vec()))
    }

    async fn list_page(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let fields = ["id", "name", "mimeType", "modifiedAt", "size", "checksum"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let page = physical_page(self.source()?, &fields, cursor, context).await?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        Ok(MarkdownMetadataPage {
            rows: page
                .rows
                .into_iter()
                .map(Self::metadata)
                .collect::<Result<_, _>>()?,
            next_cursor: page
                .next_cursor
                .map(|cursor| ProviderCursor(cursor.as_bytes().to_vec())),
            observed_at_ms: page.observed_at_ms,
        })
    }

    async fn read_document(
        &self,
        key: &SourceRecordKey,
        context: &CloudIoContext,
    ) -> Result<MarkdownDocumentRevision, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let document = self
            .source()?
            .download_markdown(key.as_str(), &read_context(context)?)
            .await
            .map_err(map_drive_document_error)?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        Ok(MarkdownDocumentRevision {
            key: SourceRecordKey::new(document.record_key()),
            revision: ProviderRevision {
                opaque: document.revision().as_str().to_string(),
                content_hash: None,
            },
            body: document.body().to_vec(),
            observed_at_ms: now_ms(),
        })
    }

    async fn write_document(
        &self,
        key: &SourceRecordKey,
        expected: &ProviderRevision,
        body: &[u8],
        context: &CloudIoContext,
    ) -> Result<MarkdownWriteReceipt, CloudDocumentError> {
        guarded_write(
            self.lifecycle.as_ref(),
            self.write_operations.as_ref(),
            self.write_capability,
            key,
            expected,
            body,
            context,
        )
        .await
    }

    async fn read_changes(
        &self,
        _cursor: &ProviderCursor,
        _context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        // Task 4 supplies the durable known-revision map required to classify
        // Drive deletions without guessing. Until then a change read must not
        // silently report an empty page.
        Err(CloudDocumentError::SyncInProgress)
    }
}

/// Native Dropbox adapter. See [`DriveMarkdownDocumentSource`] for lifecycle
/// and guarded-write invariants shared by both provider implementations.
pub(crate) struct DropboxMarkdownDocumentSource {
    source: Option<Arc<catdb_connector_dropbox::DropboxSource>>,
    lifecycle: Arc<dyn DocumentLifecycle>,
    write_operations: Arc<dyn GuardedMarkdownDocumentOperations>,
    write_capability: MarkdownWriteCapability,
}

impl DropboxMarkdownDocumentSource {
    pub(crate) fn new(
        source: Arc<catdb_connector_dropbox::DropboxSource>,
        resolver: Arc<CloudCredentialResolver>,
        profile: CloudAuthorizationProfile,
        acceptance: NativeMarkdownWriteAcceptance,
    ) -> Self {
        Self {
            source: Some(source.clone()),
            lifecycle: resolver,
            write_operations: source,
            write_capability: MarkdownWriteCapability::from_profile(profile, acceptance),
        }
    }

    #[cfg(test)]
    pub(super) fn with_test_operations(
        write_operations: Arc<dyn GuardedMarkdownDocumentOperations>,
        lifecycle: Arc<dyn DocumentLifecycle>,
        profile: CloudAuthorizationProfile,
        acceptance: NativeMarkdownWriteAcceptance,
    ) -> Self {
        Self {
            source: None,
            lifecycle,
            write_operations,
            write_capability: MarkdownWriteCapability::from_profile(profile, acceptance),
        }
    }

    fn source(&self) -> Result<&catdb_connector_dropbox::DropboxSource, CloudDocumentError> {
        self.source
            .as_deref()
            .ok_or(CloudDocumentError::ProtocolFailed)
    }

    fn metadata(row: catdb_connectors::SourceRow) -> Result<MarkdownMetadata, CloudDocumentError> {
        let name = string(&row.value, "name")?;
        let display_path = string(&row.value, "path")?;
        Ok(MarkdownMetadata {
            key: SourceRecordKey::new(row.key),
            name,
            display_path,
            media_type: None,
            modified_at_ms: optional_i64(&row.value, "modifiedAt"),
            size_bytes: optional_u64(&row.value, "size"),
            revision: ProviderRevision {
                opaque: string(&row.value, "rev")?,
                content_hash: None,
            },
            deleted: false,
        })
    }
}

#[async_trait::async_trait]
impl MarkdownDocumentSource for DropboxMarkdownDocumentSource {
    fn source_id(&self) -> &SourceId {
        self.write_operations.source_id()
    }

    async fn start_change_tracking(
        &self,
        context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let cursor = self
            .source()?
            .start_change_tracking(&read_context(context)?)
            .await
            .map_err(map_source_error)?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        Ok(ProviderCursor(cursor.as_bytes().to_vec()))
    }

    async fn list_page(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let fields = [
            "id",
            "name",
            "path",
            "modifiedAt",
            "size",
            "rev",
            "contentHash",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let page = physical_page(self.source()?, &fields, cursor, context).await?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        Ok(MarkdownMetadataPage {
            rows: page
                .rows
                .into_iter()
                .map(Self::metadata)
                .collect::<Result<_, _>>()?,
            next_cursor: page
                .next_cursor
                .map(|cursor| ProviderCursor(cursor.as_bytes().to_vec())),
            observed_at_ms: page.observed_at_ms,
        })
    }

    async fn read_document(
        &self,
        key: &SourceRecordKey,
        context: &CloudIoContext,
    ) -> Result<MarkdownDocumentRevision, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let document = self
            .source()?
            .download_markdown(key.as_str(), &read_context(context)?)
            .await
            .map_err(map_dropbox_document_error)?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        Ok(MarkdownDocumentRevision {
            key: SourceRecordKey::new(document.record_key()),
            revision: ProviderRevision {
                opaque: document.revision().as_str().to_string(),
                content_hash: None,
            },
            body: document.body().to_vec(),
            observed_at_ms: now_ms(),
        })
    }

    async fn write_document(
        &self,
        key: &SourceRecordKey,
        expected: &ProviderRevision,
        body: &[u8],
        context: &CloudIoContext,
    ) -> Result<MarkdownWriteReceipt, CloudDocumentError> {
        guarded_write(
            self.lifecycle.as_ref(),
            self.write_operations.as_ref(),
            self.write_capability,
            key,
            expected,
            body,
            context,
        )
        .await
    }

    async fn read_changes(
        &self,
        _cursor: &ProviderCursor,
        _context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        Err(CloudDocumentError::SyncInProgress)
    }
}
