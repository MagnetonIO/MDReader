//! Provider-neutral, native-only Markdown document capability.

pub(crate) mod materialized;
mod model;
pub(crate) mod service;
pub(crate) mod source;
pub(crate) mod store;
pub(crate) mod sync;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use model::{
    CloudCacheState, CloudDocumentError, CloudIoContext, CloudMarkdownDocument,
    KnownMarkdownMetadata, MarkdownChangeEvent, MarkdownChangePage, MarkdownDocumentRevision,
    MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage, MarkdownWriteReceipt,
    ProviderCursor, ProviderRevision, SourceRecordKey,
};
#[cfg(all(test, feature = "google-drive-connector"))]
pub(crate) use source::map_drive_document_error;
#[cfg(all(test, feature = "dropbox-connector"))]
pub(crate) use source::map_dropbox_document_error;
#[cfg(test)]
pub(crate) use source::map_lifecycle_error;
#[allow(unused_imports)]
pub(crate) use sync::{
    CloudSyncCoordinator, CloudSyncError, CloudSyncPhase, CloudWriteRepairExecutor,
};

/// Reads one published cloud Markdown record through the admitted native owner.
#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn read_cloud_markdown(
    app: tauri::AppHandle,
    source_id: String,
    record_key: String,
    state: tauri::State<'_, crate::cloud_accounts::CloudAccountsState>,
) -> Result<CloudMarkdownDocument, CloudDocumentError> {
    state.read_cloud_markdown(app, source_id, record_key).await
}
