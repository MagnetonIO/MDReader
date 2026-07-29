//! Provider-neutral, native-only Markdown document capability.

use std::path::Path;

use tauri_plugin_dialog::DialogExt;
use url::Url;

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
    CloudMarkdownSaveOutcome, KnownMarkdownMetadata, MarkdownChangeEvent, MarkdownChangePage,
    MarkdownDocumentRevision, MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage,
    MarkdownWriteReceipt, ProviderCursor, ProviderRevision, SourceRecordKey,
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

/// Saves one cloud Markdown body through its signed native edit authority.
#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn save_cloud_markdown(
    app: tauri::AppHandle,
    edit_token: String,
    content: String,
    state: tauri::State<'_, crate::cloud_accounts::CloudAccountsState>,
) -> Result<CloudMarkdownSaveOutcome, CloudDocumentError> {
    state.save_cloud_markdown(app, edit_token, content).await
}

/// Open one allowlisted provider URL with the operating system's browser.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn open_cloud_original(url: String) -> Result<(), CloudDocumentError> {
    let parsed = Url::parse(&url).map_err(|_| CloudDocumentError::ProtocolFailed)?;
    let allowed = parsed.scheme() == "https"
        && matches!(
            parsed.host_str(),
            Some("drive.google.com" | "www.dropbox.com")
        );
    if !allowed || parsed.username() != "" || parsed.password().is_some() {
        return Err(CloudDocumentError::ProtocolFailed);
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|_| CloudDocumentError::ProtocolFailed)
}

/// Ask for a destination and save one bounded Markdown copy outside the cache.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_markdown_copy(
    app: tauri::AppHandle,
    suggested_name: String,
    content: String,
) -> Result<bool, CloudDocumentError> {
    if content.len() > store::CloudDocumentStore::MAX_BODY_BYTES
        || suggested_name.is_empty()
        || suggested_name.len() > 240
        || Path::new(&suggested_name).components().count() != 1
        || suggested_name.chars().any(char::is_control)
    {
        return Err(CloudDocumentError::ProtocolFailed);
    }
    let Some(destination) = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown"])
        .set_file_name(&suggested_name)
        .blocking_save_file()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(false);
    };
    tauri::async_runtime::spawn_blocking(move || std::fs::write(destination, content))
        .await
        .map_err(|_| CloudDocumentError::ProtocolFailed)?
        .map_err(|_| CloudDocumentError::ProtocolFailed)?;
    Ok(true)
}
