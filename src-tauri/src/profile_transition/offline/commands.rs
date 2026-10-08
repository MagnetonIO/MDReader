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
    Close {
        operation_id: String,
    },
}
#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Document {
    document_id: String,
    name: String,
    revision: String,
    bytes: u32,
    pinned: bool,
    line: u32,
}
#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigrationReply {
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
    }
}
struct Pending {
    id: Uuid,
    #[cfg(unix)]
    original: Option<ClosedOwner>,
    #[cfg(unix)]
    workspace: Option<Workspace>,
}
enum Phase {
    Empty,
    Busy,
    Ready(Pending),
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
            Phase::Ready(pending) => Some(pending),
            _ => None,
        })
    }
    fn put(&self, pending: Option<Pending>) {
        if let Ok(mut state) = self.0.lock() {
            *state = pending.map_or(Phase::Empty, Phase::Ready);
        }
    }
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
    let folder = folder.into_path().map_err(|_| Error::Invalid)?;
    let parent = SourceRoot::capture(folder).map_err(|_| Error::Invalid)?;
    let current = crate::reader_profile::directory(app).map_err(|_| Error::Conflict)?;
    if parent.path.starts_with(source)
        || source.starts_with(&parent.path)
        || parent.path.starts_with(&current)
        || current.starts_with(&parent.path)
    {
        return Err(Error::Invalid);
    }
    parent.verify().map_err(|_| Error::Conflict)?;
    Ok(Some(parent))
}
#[cfg(unix)]
fn execute(
    app: &AppHandle,
    request: MigrationRequest,
    pending: &mut Option<Pending>,
) -> Result<MigrationReply> {
    if matches!(request, MigrationRequest::Resume) {
        if pending.is_some() {
            return Err(Error::Conflict);
        }
        let Some(folder) = app
            .dialog()
            .file()
            .set_title("Reopen a retained MDReader migration folder")
            .blocking_pick_folder()
        else {
            return Ok(reply("cancelled", None));
        };
        let path = folder.into_path().map_err(|_| Error::Invalid)?;
        let mut workspace = Workspace::resume(&path)?;
        let state = workspace.status()?;
        let imported = workspace.imported()?;
        let id = Uuid::new_v4();
        *pending = Some(Pending {
            id,
            original: None,
            workspace: Some(workspace),
        });
        let mut result = reply(state, Some(id));
        result.imported = imported;
        return Ok(result);
    }
    if matches!(request, MigrationRequest::Preview) {
        if pending.is_some() {
            return Err(Error::Conflict);
        }
        let Some(folder) = app
            .dialog()
            .file()
            .set_title("Choose a closed MDReader profile — the original is retained")
            .blocking_pick_folder()
        else {
            return Ok(reply("cancelled", None));
        };
        let path = folder.into_path().map_err(|_| Error::Invalid)?;
        let selected = SourceRoot::capture(path).map_err(|_| Error::Invalid)?;
        let current = crate::reader_profile::directory(app).map_err(|_| Error::Conflict)?;
        if selected.path.starts_with(&current) || current.starts_with(&selected.path) {
            return Err(Error::NotQuiescent);
        }
        let original = ClosedOwner::acquire(&selected.path)?;
        // Reject unknown stores BEFORE promising a usable conversion. Known
        // library SQLite is archived in full, including unconverted records.
        super::super::backup_inventory::selections(original.path(), &Default::default())?;
        let store = ReaderStore::open_readonly(&original.path().join("reader-v1"))
            .map_err(|_| Error::Unsupported)?;
        let documents = conversion::documents(&store)?
            .into_iter()
            .map(|document| Document {
                document_id: document.id,
                name: document.name,
                revision: document.revision,
                bytes: document.bytes,
                pinned: document.pinned,
                line: document.line,
            })
            .collect();
        let id = Uuid::new_v4();
        *pending = Some(Pending {
            id,
            original: Some(original),
            workspace: None,
        });
        let mut result = reply("preview", Some(id));
        result.documents = documents;
        return Ok(result);
    }
    let operation = match &request {
        MigrationRequest::Export { operation_id }
        | MigrationRequest::Prepare { operation_id, .. }
        | MigrationRequest::Activate { operation_id }
        | MigrationRequest::Close { operation_id } => operation_id,
        MigrationRequest::Preview | MigrationRequest::Resume => return Err(Error::Invalid),
    };
    let id = Uuid::parse_str(operation).map_err(|_| Error::Invalid)?;
    let active = pending.as_mut().ok_or(Error::Conflict)?;
    if id != active.id {
        return Err(Error::Conflict);
    }
    match request {
        MigrationRequest::Close { .. } => {
            *pending = None;
            Ok(reply("closed", Some(id)))
        }
        MigrationRequest::Export { .. } => {
            let original = active.original.as_mut().ok_or(Error::Conflict)?;
            let Some(parent) = selected_parent(app, original.path())? else {
                return Ok(reply("cancelled", Some(id)));
            };
            super::workspace::export(original, &parent.path)?.revalidate(&Default::default())?;
            parent.verify().map_err(|_| Error::Conflict)?;
            Ok(reply("exported", Some(id)))
        }
        MigrationRequest::Prepare { documents, .. } => {
            if documents.is_empty() || documents.len() > 1000 {
                return Err(Error::Bounds);
            }
            let mut seen = std::collections::HashSet::new();
            for selection in &documents {
                if Uuid::parse_str(&selection.document_id).is_err()
                    || selection.revision.len() != 64
                    || !selection
                        .revision
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                    || !seen.insert(&selection.document_id)
                {
                    return Err(Error::Invalid);
                }
            }
            let original = active.original.as_ref().ok_or(Error::Conflict)?;
            let Some(parent) = selected_parent(app, original.path())? else {
                return Ok(reply("cancelled", Some(id)));
            };
            let original = active.original.take().ok_or(Error::Conflict)?;
            // On partial failure the archive/candidate remain in the selected
            // destination; no reset, deletion, retry or source regrant occurs.
            active.workspace = Some(Workspace::prepare(original, &parent.path, &documents)?);
            parent.verify().map_err(|_| Error::Conflict)?;
            let mut result = reply("prepared", Some(id));
            result.imported = active
                .workspace
                .as_mut()
                .ok_or(Error::Conflict)?
                .imported()?;
            Ok(result)
        }
        MigrationRequest::Activate { .. } => {
            let path = active
                .workspace
                .as_mut()
                .ok_or(Error::Conflict)?
                .activate()?;
            // Selector CAS happened under both closed-owner capabilities. Drop
            // those locks before launching the independently owned new profile.
            // Keep the current GUI and any drafts untouched; this is a separate
            // profile launch, not an in-place replacement or auto-resume switch.
            active.workspace.take();
            let executable = std::env::current_exe().map_err(|_| Error::Storage)?;
            std::process::Command::new(executable)
                .arg("--reader-profile")
                .arg(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|_| Error::ReconciliationRequired)?;
            Ok(reply("launch_requested", Some(id)))
        }
        MigrationRequest::Preview | MigrationRequest::Resume => Err(Error::Invalid),
    }
}
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
