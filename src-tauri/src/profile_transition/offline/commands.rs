//! Native-selected offline profile operations. Renderer receives only bounded
//! document metadata and opaque operation identity, never paths or grants.
use super::*;
#[cfg(unix)]
use super::{conversion, owner::ClosedOwner, workspace::Workspace};
#[cfg(unix)]
use mdreader_core::{ReaderStore, SourceRoot};
use std::sync::Mutex;
use tauri::{AppHandle, State};
#[cfg(unix)]
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

#[derive(Deserialize, specta::Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MigrationRequest {
    Preview,
    Resume,
    Export {
        operation_id: String,
    },
    Prepare {
        operation_id: String,
        documents: Vec<Selection>,
    },
    Activate {
        operation_id: String,
    },
    Rollback {
        operation_id: String,
    },
    Close {
        operation_id: String,
    },
}
#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Document {
    pub(super) document_id: String,
    pub(super) name: String,
    pub(super) revision: String,
    pub(super) bytes: u32,
    pub(super) pinned: bool,
    pub(super) line: u32,
}
#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(super) struct Coverage {
    pub(super) documents: u32,
    pub(super) relationships: u32,
    pub(super) reviews: u32,
    pub(super) history_entries: u32,
    pub(super) unavailable_history_entries: u32,
    pub(super) settings: bool,
    pub(super) unknown_records: u32,
}
#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigrationReply {
    origin: Option<&'static str>,
    coverage: Option<Coverage>,
    state: &'static str,
    operation_id: Option<String>,
    documents: Vec<Document>,
    imported: u32,
}
#[derive(Serialize, specta::Type)]
pub(crate) struct MigrationError {
    kind: &'static str,
}
fn failure(error: Error) -> MigrationError {
    MigrationError {
        kind: match error {
            Error::NotQuiescent => "close_source_app_required",
            Error::Unclassified | Error::Unsupported => "offline_export_required",
            Error::Bounds => "too_large",
            Error::Conflict => "profile_changed",
            Error::InvalidDestination => "invalid_destination",
            Error::Invalid | Error::Integrity => "invalid_profile",
            _ => "migration_unconfirmed",
        },
    }
}
fn reply(state: &'static str, operation: Option<Uuid>) -> MigrationReply {
    MigrationReply {
        state,
        operation_id: operation.map(|id| id.to_string()),
        documents: vec![],
        imported: 0,
        origin: None,
        coverage: None,
    }
}
struct Pending {
    id: Uuid,
    #[cfg(unix)]
    original: Option<ClosedOwner>,
    #[cfg(unix)]
    archive: Option<super::archive_import::ArchiveImport>,
    #[cfg(unix)]
    workspace: Option<Workspace>,
}
enum Phase {
    Empty,
    Busy,
    Ready(Box<Pending>),
}
pub(crate) struct MigrationState(Mutex<Phase>);
impl Default for MigrationState {
    fn default() -> Self {
        Self(Mutex::new(Phase::Empty))
    }
}
impl MigrationState {
    fn take(&self) -> Result<Option<Pending>> {
        let mut state = self.0.lock().map_err(|_| Error::ReconciliationRequired)?;
        if matches!(*state, Phase::Busy) {
            return Err(Error::NotQuiescent);
        }
        Ok(match std::mem::replace(&mut *state, Phase::Busy) {
            Phase::Ready(pending) => Some(*pending),
            _ => None,
        })
    }
    fn put(&self, pending: Option<Pending>) {
        if let Ok(mut state) = self.0.lock() {
            *state = pending.map_or(Phase::Empty, |pending| Phase::Ready(Box::new(pending)));
        }
    }
}
#[cfg(unix)]
pub(super) fn picked_root(path: PathBuf) -> Result<SourceRoot> {
    // Reject a leaf symlink first, then normalize OS aliases such as /var on
    // macOS without changing the admitted object or relaxing no-follow checks.
    let original = SourceRoot::capture(path).map_err(|_| Error::Invalid)?;
    let canonical = original.path.canonicalize().map_err(|_| Error::Invalid)?;
    let root =
        SourceRoot::restore(canonical, original.identity.clone()).map_err(|_| Error::Conflict)?;
    original.verify().map_err(|_| Error::Conflict)?;
    Ok(root)
}
#[cfg(unix)]
fn selected_parent(app: &AppHandle, source: &Path) -> Result<Option<SourceRoot>> {
    let Some(folder) = app
        .dialog()
        .file()
        .set_title("Choose a folder for the separate reader profile and original archive")
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let parent = picked_root(folder.into_path().map_err(|_| Error::InvalidDestination)?).map_err(
        |error| match error {
            Error::Invalid => Error::InvalidDestination,
            other => other,
        },
    )?;
    let current = crate::reader_profile::directory(app)
        .map_err(|_| Error::Conflict)?
        .canonicalize()
        .map_err(|_| Error::Conflict)?;
    if parent.path.starts_with(source)
        || source.starts_with(&parent.path)
        || parent.path.starts_with(&current)
        || current.starts_with(&parent.path)
    {
        return Err(Error::InvalidDestination);
    }
    parent.verify().map_err(|_| Error::Conflict)?;
    Ok(Some(parent))
}
#[cfg(unix)]
mod execute;
#[cfg(unix)]
use execute::execute;
#[cfg(not(unix))]
fn execute(_: &AppHandle, _: MigrationRequest, _: &mut Option<Pending>) -> Result<MigrationReply> {
    Err(Error::Unsupported)
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_migration(
    app: AppHandle,
    state: State<'_, MigrationState>,
    request: MigrationRequest,
) -> std::result::Result<MigrationReply, MigrationError> {
    let mut pending = state.take().map_err(failure)?;
    // The state stores only native capabilities/paths; it is never serialized.
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = execute(&handle, request, &mut pending);
        (result, pending)
    })
    .await;
    match result {
        Ok((result, pending)) => {
            state.put(pending);
            result.map_err(failure)
        }
        Err(_) => Err(failure(Error::ReconciliationRequired)), // retain Busy after panic/unknown
    }
}
