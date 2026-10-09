//! Retained local workspace authority and the native SQLite service adapter.
//! No legacy database, embedding model or proposal writer is opened here.
mod epoch;
mod index;
mod model;
mod navigation;
use crate::commands::{mtime_millis, resolve_dir, resolve_within, FileEntry, WorkspaceState};
pub use epoch::Epoch;
pub(crate) use epoch::StaleEpoch;
#[cfg(test)]
pub(crate) use index::queue_reader_index;
use mdreader_core::{scan_source_root, ScanOptions, SourceRoot};
pub use model::{ImportProgress, ListingError, WikiCandidate, WikiResolution, WorkspaceListing};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{ipc::Channel, AppHandle, Manager, State};
// Legacy managed paths are identifiers for explicit offline preservation/cleanup,
// never a database opener or a new source permission.
#[cfg(test)]
pub(crate) fn db_path_under(base: &Path, root: &SourceRoot) -> PathBuf {
    crate::library::legacy_layout::workspace_index_path(base, root)
}
fn claim_active(epoch: &Epoch) -> Result<bool, String> {
    let mut active = epoch
        .active
        .lock()
        .map_err(|_| "Workspace state unavailable")?;
    if *active {
        return Ok(false);
    }
    *active = true;
    Ok(true)
}
struct ActiveGuard {
    epoch: Arc<Epoch>,
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.epoch.active.lock() {
            *active = false;
        }
    }
}
fn record_status(epoch: &Epoch, progress: &ImportProgress) {
    if let Ok(mut status) = epoch.status.lock() {
        *status = progress.clone();
    }
}

#[tauri::command]
#[specta::specta]
pub async fn start_import(
    app: AppHandle,
    root: String,
    on_event: Channel<ImportProgress>,
    workspace: State<'_, WorkspaceState>,
) -> Result<ImportProgress, String> {
    index::start_import(app, root, on_event, workspace).await
}
#[tauri::command(async)]
#[specta::specta]
pub fn import_status(workspace: State<'_, WorkspaceState>) -> Result<ImportProgress, String> {
    index::import_status(workspace)
}
#[tauri::command(async)]
#[specta::specta]
pub fn list_workspace_files(
    root: String,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, crate::reader::ReaderBackend>,
) -> Result<WorkspaceListing, ListingError> {
    navigation::list_workspace_files(root, workspace, reader)
}
#[tauri::command(async)]
#[specta::specta]
pub fn resolve_wikilink(
    from_doc: String,
    target: String,
    heading: Option<String>,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, crate::reader::ReaderBackend>,
) -> Result<WikiResolution, String> {
    navigation::resolve_wikilink(from_doc, target, heading, workspace, reader)
}

#[cfg(test)]
mod tests;
