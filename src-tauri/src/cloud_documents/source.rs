use std::{sync::Arc, time::SystemTime};

use catdb_connectors::{DataSource, ReadContext, SourceCursor, SourceError, SourceId, SourcePage};
use catdb_query::{EntityQuery, EntityTarget};

use super::model::{
    CloudDocumentError, CloudIoContext, MarkdownChangePage, MarkdownDocumentRevision,
    MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage, MarkdownWriteReceipt,
    ProviderCursor, ProviderRevision, SourceRecordKey,
};
use crate::cloud_accounts::{CloudAuthorizationProfile, CloudCredentialResolver, CloudSourceError};

const CLOUD_DOCUMENT_ENTITY: &str = "mdreader.Document";

/// Publication state for the guarded provider-write primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkdownWriteCapability {
    ReconnectRequired,
    PendingAcceptance,
    Accepted,
}

impl MarkdownWriteCapability {
    pub(crate) fn from_profile(profile: CloudAuthorizationProfile) -> Self {
        match profile {
            CloudAuthorizationProfile::ReadOnlyV1 => Self::ReconnectRequired,
            CloudAuthorizationProfile::MarkdownReadWriteV2 => Self::PendingAcceptance,
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

async fn check_lifecycle(resolver: &CloudCredentialResolver) -> Result<(), CloudDocumentError> {
    resolver
        .validate_document_authority()
        .await
        .map_err(map_lifecycle_error)
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
    source: Arc<catdb_connector_google_drive::DriveSource>,
    resolver: Arc<CloudCredentialResolver>,
    write_capability: MarkdownWriteCapability,
}

impl DriveMarkdownDocumentSource {
    pub(crate) fn new(
        source: Arc<catdb_connector_google_drive::DriveSource>,
        resolver: Arc<CloudCredentialResolver>,
        profile: CloudAuthorizationProfile,
    ) -> Self {
        Self {
            source,
            resolver,
            write_capability: MarkdownWriteCapability::from_profile(profile),
        }
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
        self.source.id()
    }

    async fn start_change_tracking(
        &self,
        context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError> {
        check_lifecycle(&self.resolver).await?;
        let cursor = self
            .source
            .start_change_tracking(&read_context(context)?)
            .await
            .map_err(map_source_error)?;
        check_lifecycle(&self.resolver).await?;
        Ok(ProviderCursor(cursor.as_bytes().to_vec()))
    }

    async fn list_page(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
        check_lifecycle(&self.resolver).await?;
        let fields = ["id", "name", "mimeType", "modifiedAt", "size", "checksum"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let page = physical_page(self.source.as_ref(), &fields, cursor, context).await?;
        check_lifecycle(&self.resolver).await?;
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
        check_lifecycle(&self.resolver).await?;
        let document = self
            .source
            .download_markdown(key.as_str(), &read_context(context)?)
            .await
            .map_err(map_drive_document_error)?;
        check_lifecycle(&self.resolver).await?;
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
        check_lifecycle(&self.resolver).await?;
        if self.write_capability != MarkdownWriteCapability::Accepted {
            return Err(self.write_capability.error());
        }
        let revision = catdb_connector_google_drive::DriveRevision::new(expected.opaque.clone())
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let receipt = self
            .source
            .update_markdown(key.as_str(), &revision, body, &read_context(context)?)
            .await
            .map_err(map_drive_document_error)?;
        if check_lifecycle(&self.resolver).await.is_err() {
            return Err(CloudDocumentError::WriteOutcomeUnknown);
        }
        Ok(MarkdownWriteReceipt {
            key: SourceRecordKey::new(receipt.record_key()),
            revision: ProviderRevision {
                opaque: receipt.revision().as_str().to_string(),
                content_hash: None,
            },
            modified_at_ms: None,
            size_bytes: body
                .len()
                .try_into()
                .map_err(|_| CloudDocumentError::TooLarge)?,
        })
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
    source: Arc<catdb_connector_dropbox::DropboxSource>,
    resolver: Arc<CloudCredentialResolver>,
    write_capability: MarkdownWriteCapability,
}

impl DropboxMarkdownDocumentSource {
    pub(crate) fn new(
        source: Arc<catdb_connector_dropbox::DropboxSource>,
        resolver: Arc<CloudCredentialResolver>,
        profile: CloudAuthorizationProfile,
    ) -> Self {
        Self {
            source,
            resolver,
            write_capability: MarkdownWriteCapability::from_profile(profile),
        }
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
        self.source.id()
    }

    async fn start_change_tracking(
        &self,
        context: &CloudIoContext,
    ) -> Result<ProviderCursor, CloudDocumentError> {
        check_lifecycle(&self.resolver).await?;
        let cursor = self
            .source
            .start_change_tracking(&read_context(context)?)
            .await
            .map_err(map_source_error)?;
        check_lifecycle(&self.resolver).await?;
        Ok(ProviderCursor(cursor.as_bytes().to_vec()))
    }

    async fn list_page(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &CloudIoContext,
    ) -> Result<MarkdownMetadataPage, CloudDocumentError> {
        check_lifecycle(&self.resolver).await?;
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
        let page = physical_page(self.source.as_ref(), &fields, cursor, context).await?;
        check_lifecycle(&self.resolver).await?;
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
        check_lifecycle(&self.resolver).await?;
        let document = self
            .source
            .download_markdown(key.as_str(), &read_context(context)?)
            .await
            .map_err(map_dropbox_document_error)?;
        check_lifecycle(&self.resolver).await?;
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
        check_lifecycle(&self.resolver).await?;
        if self.write_capability != MarkdownWriteCapability::Accepted {
            return Err(self.write_capability.error());
        }
        let revision = catdb_connector_dropbox::DropboxRevision::new(expected.opaque.clone())
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let receipt = self
            .source
            .update_markdown(key.as_str(), &revision, body, &read_context(context)?)
            .await
            .map_err(map_dropbox_document_error)?;
        if check_lifecycle(&self.resolver).await.is_err() {
            return Err(CloudDocumentError::WriteOutcomeUnknown);
        }
        Ok(MarkdownWriteReceipt {
            key: SourceRecordKey::new(receipt.record_key()),
            revision: ProviderRevision {
                opaque: receipt.revision().as_str().to_string(),
                content_hash: None,
            },
            modified_at_ms: None,
            size_bytes: body
                .len()
                .try_into()
                .map_err(|_| CloudDocumentError::TooLarge)?,
        })
    }

    async fn read_changes(
        &self,
        _cursor: &ProviderCursor,
        _context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        Err(CloudDocumentError::SyncInProgress)
    }
}
