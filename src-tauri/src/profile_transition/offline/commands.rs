//! Native-selected offline profile operations. Renderer receives only bounded
//! document metadata and opaque operation identity, never paths or grants.
use super::*;
#[cfg(unix)]
use super::{conversion, owner::ClosedOwner, workspace::Workspace};
#[cfg(any(unix, windows))]
use mdreader_core::SourceRoot;
use std::sync::Mutex;
use tauri::{AppHandle, State};
#[cfg(any(unix, windows))]
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
    ResumeReturn,
    Retry {
        operation_id: String,
    },
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
    pub(super) source_references: u32,
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
    #[cfg(windows)]
    windows: windows::Retained,
    #[cfg(unix)]
    rollback_pending: bool,
    #[cfg(unix)]
    controller_issued: bool,
    #[cfg(unix)]
    original: Option<ClosedOwner>,
    #[cfg(unix)]
    opening: Option<opening::Opening>,
    #[cfg(unix)]
    legacy: Option<super::legacy_source::LegacySource>,
    #[cfg(unix)]
    archive: Option<super::archive_import::ArchiveImport>,
    #[cfg(unix)]
    workspace: Option<Workspace>,
}
#[cfg(unix)]
impl Pending {
    fn handoff_issued(&self) -> bool {
        self.opening.as_ref().is_some_and(|owner| owner.issued())
            || self
                .legacy
                .as_ref()
                .is_some_and(|owner| owner.handoff_issued())
            || self
                .archive
                .as_ref()
                .is_some_and(|owner| owner.handoff_issued())
    }
    fn handoff_failure(&self) -> Option<&'static str> {
        self.opening
            .as_ref()
            .and_then(|owner| owner.failure_kind())
            .or_else(|| {
                self.legacy
                    .as_ref()
                    .and_then(|owner| owner.handoff_failure())
            })
            .or_else(|| {
                self.archive
                    .as_ref()
                    .and_then(|owner| owner.handoff_failure())
            })
    }
    fn ensure_closeable(&self) -> Result<()> {
        if self.handoff_issued() || self.rollback_pending || self.controller_issued {
            return Err(Error::ReconciliationRequired);
        }
        Ok(())
    }
    fn waiting(&self) -> MigrationReply {
        let state = match self.handoff_failure() {
            _ if self.controller_issued => "return_controller_requested",
            _ if self.rollback_pending => "rollback_handoff_pending",
            Some("background_service_unsupported") => "background_service_unsupported",
            None if !self.handoff_issued() && self.opening.is_some() => "close_original_required",
            _ => "service_handoff_pending",
        };
        let mut result = reply(state, Some(self.id));
        result.origin = Some(if self.legacy.is_some() || self.archive.is_some() {
            "legacy_catdb"
        } else {
            "reader"
        });
        result
    }
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
#[cfg(any(unix, windows))]
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
#[cfg(any(unix, windows))]
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
mod controller;
#[cfg(unix)]
mod execute;
#[cfg(unix)]
mod opening;
#[cfg(unix)]
mod return_original;
#[cfg(unix)]
use execute::execute;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::execute;
#[cfg(not(any(unix, windows)))]
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
            let result = result.map_err(|error| {
                #[cfg(any(unix, windows))]
                if matches!(error, Error::ReconciliationRequired) {
                    if let Some(kind) = pending.as_ref().and_then(|pending| {
                        pending.handoff_failure().or_else(|| {
                            pending
                                .handoff_issued()
                                .then_some("service_handoff_unconfirmed")
                        })
                    }) {
                        return MigrationError { kind };
                    }
                }
                failure(error)
            });
            state.put(pending);
            result
        }
        Err(_) => Err(failure(Error::ReconciliationRequired)), // retain Busy after panic/unknown
    }
}
