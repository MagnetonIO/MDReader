use std::{collections::BTreeMap, time::SystemTime};

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use std::sync::Arc;

use catdb_connectors::{DataSource, ReadContext, SourceCursor, SourceError, SourceId, SourcePage};
use catdb_query::{EntityQuery, EntityTarget};

use super::model::{
    CloudDocumentError, CloudIoContext, KnownMarkdownMetadata, MarkdownWriteReceipt,
    ProviderCursor, ProviderRevision, SourceRecordKey,
};
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use super::model::{
    MarkdownChangePage, MarkdownDocumentRevision, MarkdownDocumentSource, MarkdownMetadata,
    MarkdownMetadataPage,
};
use crate::cloud_accounts::{CloudAuthorizationProfile, NativeMarkdownWriteAcceptance};
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use crate::cloud_accounts::{CloudCredentialResolver, CloudSourceError};

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
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
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
#[cfg(feature = "google-drive-connector")]
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
#[cfg(feature = "dropbox-connector")]
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

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
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
        #[cfg(any(
            not(feature = "google-drive-connector"),
            not(feature = "dropbox-connector")
        ))]
        CloudSourceError::ConnectorUnavailable => CloudDocumentError::ConnectorUnavailable,
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

#[cfg(feature = "google-drive-connector")]
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

#[cfg(feature = "dropbox-connector")]
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

#[cfg(feature = "google-drive-connector")]
fn drive_timestamp(milliseconds: i64) -> Result<String, CloudDocumentError> {
    use time::format_description::well_known::Rfc3339;

    let nanos = i128::from(milliseconds)
        .checked_mul(1_000_000)
        .ok_or(CloudDocumentError::ProtocolFailed)?;
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|_| CloudDocumentError::ProtocolFailed)?;
    timestamp
        .format(&Rfc3339)
        .map_err(|_| CloudDocumentError::ProtocolFailed)
}

#[cfg(feature = "dropbox-connector")]
fn decode_hash(value: &str) -> Result<[u8; 32], CloudDocumentError> {
    if value.len() != 64 {
        return Err(CloudDocumentError::ProtocolFailed);
    }
    let mut hash = [0_u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16)
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
    }
    Ok(hash)
}

#[cfg(feature = "dropbox-connector")]
fn encode_hash(hash: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(64);
    for byte in hash {
        value.push(HEX[usize::from(byte >> 4)] as char);
        value.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    value
}

/// Native Drive adapter. Its resolver retains the admitted credential and
/// rechecks source identity before and after every provider effect.
#[cfg(feature = "google-drive-connector")]
pub(crate) struct DriveMarkdownDocumentSource {
    source: Option<Arc<catdb_connector_google_drive::DriveSource>>,
    lifecycle: Arc<dyn DocumentLifecycle>,
    write_operations: Arc<dyn GuardedMarkdownDocumentOperations>,
    write_capability: MarkdownWriteCapability,
}

#[cfg(feature = "google-drive-connector")]
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

    fn known_revisions(
        known: &[KnownMarkdownMetadata],
    ) -> Result<
        BTreeMap<String, catdb_connector_google_drive::DriveKnownRevision>,
        CloudDocumentError,
    > {
        known
            .iter()
            .filter(|metadata| !metadata.deleted && metadata.modified_at_ms.is_some())
            .map(|metadata| {
                let modified_time = drive_timestamp(
                    metadata
                        .modified_at_ms
                        .ok_or(CloudDocumentError::ProtocolFailed)?,
                )?;
                let checksum = (metadata.revision.opaque != metadata.key.as_str())
                    .then(|| metadata.revision.opaque.clone());
                let revision =
                    catdb_connector_google_drive::DriveKnownRevision::new(modified_time, checksum)
                        .map_err(|_| CloudDocumentError::ProtocolFailed)?;
                Ok((metadata.key.as_str().to_string(), revision))
            })
            .collect()
    }

    fn deleted(known: &KnownMarkdownMetadata) -> MarkdownMetadata {
        MarkdownMetadata {
            key: known.key.clone(),
            name: known.name.clone(),
            display_path: known.display_path.clone(),
            media_type: known.media_type.clone(),
            modified_at_ms: known.modified_at_ms,
            size_bytes: known.size_bytes,
            revision: known.revision.clone(),
            deleted: true,
        }
    }
}

#[async_trait::async_trait]
#[cfg(feature = "google-drive-connector")]
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
        cursor: &ProviderCursor,
        known: &[KnownMarkdownMetadata],
        context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let cursor = catdb_connector_google_drive::DriveChangeCursor::from_bytes(cursor.0.clone())
            .map_err(map_source_error)?;
        let known_revisions = Self::known_revisions(known)?;
        let known_metadata = known
            .iter()
            .filter(|metadata| !metadata.deleted)
            .map(|metadata| (metadata.key.as_str().to_string(), metadata))
            .collect::<BTreeMap<_, _>>();
        let outcome = self
            .source()?
            .read_changes(&cursor, &known_revisions, &read_context(context)?)
            .await
            .map_err(map_source_error)?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        match outcome {
            catdb_connector_google_drive::DriveChangeOutcome::RebuildRequired => {
                Err(CloudDocumentError::ResetCursor)
            }
            catdb_connector_google_drive::DriveChangeOutcome::Page {
                events,
                next_cursor,
                checkpoint,
            } => {
                let has_more = next_cursor.is_some();
                let next_cursor = match (next_cursor, checkpoint) {
                    (Some(next), None) => ProviderCursor(next.as_bytes().to_vec()),
                    (None, Some(checkpoint)) => ProviderCursor(checkpoint.as_bytes().to_vec()),
                    _ => return Err(CloudDocumentError::ProtocolFailed),
                };
                let mut changes = Vec::new();
                for event in events {
                    match event {
                        catdb_connector_google_drive::DriveChangeEvent::Deleted { file_id } => {
                            if let Some(metadata) = known_metadata.get(&file_id) {
                                changes.push(Self::deleted(metadata));
                            }
                        }
                        catdb_connector_google_drive::DriveChangeEvent::Unchanged { .. } => {}
                        catdb_connector_google_drive::DriveChangeEvent::Upserted {
                            file_id,
                            ..
                        } => {
                            let row = self
                                .source()?
                                .metadata_for_sync(&file_id, &read_context(context)?)
                                .await
                                .map_err(map_source_error)?;
                            changes.push(Self::metadata(row)?);
                        }
                    }
                }
                Ok(MarkdownChangePage {
                    changes,
                    next_cursor,
                    has_more,
                    observed_at_ms: now_ms(),
                })
            }
        }
    }
}

/// Native Dropbox adapter. See [`DriveMarkdownDocumentSource`] for lifecycle
/// and guarded-write invariants shared by both provider implementations.
#[cfg(feature = "dropbox-connector")]
pub(crate) struct DropboxMarkdownDocumentSource {
    source: Option<Arc<catdb_connector_dropbox::DropboxSource>>,
    lifecycle: Arc<dyn DocumentLifecycle>,
    write_operations: Arc<dyn GuardedMarkdownDocumentOperations>,
    write_capability: MarkdownWriteCapability,
}

#[cfg(feature = "dropbox-connector")]
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
                content_hash: row
                    .value
                    .get("contentHash")
                    .and_then(serde_json::Value::as_str)
                    .map(decode_hash)
                    .transpose()?,
            },
            deleted: false,
        })
    }

    fn known_revisions(
        known: &[KnownMarkdownMetadata],
    ) -> Result<BTreeMap<String, catdb_connector_dropbox::DropboxKnownRevision>, CloudDocumentError>
    {
        known
            .iter()
            .filter(|metadata| !metadata.deleted && metadata.content_hash.is_some())
            .map(|metadata| {
                let content_hash = metadata
                    .content_hash
                    .ok_or(CloudDocumentError::ProtocolFailed)?;
                let revision = catdb_connector_dropbox::DropboxKnownRevision::new(
                    metadata.revision.opaque.clone(),
                    encode_hash(content_hash),
                )
                .map_err(|_| CloudDocumentError::ProtocolFailed)?;
                Ok((metadata.key.as_str().to_string(), revision))
            })
            .collect()
    }

    fn deleted(known: &KnownMarkdownMetadata) -> MarkdownMetadata {
        MarkdownMetadata {
            key: known.key.clone(),
            name: known.name.clone(),
            display_path: known.display_path.clone(),
            media_type: known.media_type.clone(),
            modified_at_ms: known.modified_at_ms,
            size_bytes: known.size_bytes,
            revision: known.revision.clone(),
            deleted: true,
        }
    }
}

#[async_trait::async_trait]
#[cfg(feature = "dropbox-connector")]
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
        cursor: &ProviderCursor,
        known: &[KnownMarkdownMetadata],
        context: &CloudIoContext,
    ) -> Result<MarkdownChangePage, CloudDocumentError> {
        check_lifecycle(self.lifecycle.as_ref()).await?;
        let cursor = catdb_connector_dropbox::DropboxChangeCursor::from_bytes(cursor.0.clone())
            .map_err(map_source_error)?;
        let known_revisions = Self::known_revisions(known)?;
        let known_metadata = known
            .iter()
            .filter(|metadata| !metadata.deleted)
            .map(|metadata| (metadata.key.as_str().to_string(), metadata))
            .collect::<BTreeMap<_, _>>();
        let outcome = self
            .source()?
            .read_changes(&cursor, &known_revisions, &read_context(context)?)
            .await
            .map_err(map_source_error)?;
        check_lifecycle(self.lifecycle.as_ref()).await?;
        match outcome {
            catdb_connector_dropbox::DropboxChangeOutcome::RebuildRequired => {
                Err(CloudDocumentError::ResetCursor)
            }
            catdb_connector_dropbox::DropboxChangeOutcome::Page {
                events,
                next_cursor,
                checkpoint,
            } => {
                let has_more = next_cursor.is_some();
                let next_cursor = match (next_cursor, checkpoint) {
                    (Some(next), None) => ProviderCursor(next.as_bytes().to_vec()),
                    (None, Some(checkpoint)) => ProviderCursor(checkpoint.as_bytes().to_vec()),
                    _ => return Err(CloudDocumentError::ProtocolFailed),
                };
                let mut changes = Vec::new();
                for event in events {
                    match event {
                        catdb_connector_dropbox::DropboxChangeEvent::Deleted { path_lower } => {
                            if let Some(metadata) = known_metadata.get(&path_lower) {
                                changes.push(Self::deleted(metadata));
                            }
                        }
                        catdb_connector_dropbox::DropboxChangeEvent::Unchanged { .. } => {}
                        catdb_connector_dropbox::DropboxChangeEvent::Upserted {
                            path_lower,
                            ..
                        } => {
                            let row = self
                                .source()?
                                .metadata_for_sync(&path_lower, &read_context(context)?)
                                .await
                                .map_err(map_source_error)?;
                            changes.push(Self::metadata(row)?);
                        }
                    }
                }
                Ok(MarkdownChangePage {
                    changes,
                    next_cursor,
                    has_more,
                    observed_at_ms: now_ms(),
                })
            }
        }
    }
}
