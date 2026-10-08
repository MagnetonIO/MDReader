//! Native-selected output and all-owner profile preservation. This exports a
//! copy; it never activates another profile or transfers OS-vault credentials.
use super::{BorrowedFreeze, NativeProfileTransition, persistence::commands::PersistenceState};
use mdreader_core::SourceRoot;
use mdreader_profile::{Error, Inventory, Limits, Result, bundle::VerifiedBundle};
use serde::Serialize;
use std::{path::Path, time::Duration};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupState {
    Exported,
    Cancelled,
}
#[derive(Serialize, specta::Type)]
pub(crate) struct BackupReply {
    pub state: BackupState,
}
#[derive(Serialize, specta::Type)]
pub(crate) struct BackupError {
    pub kind: &'static str,
}
fn failure(error: Error) -> BackupError {
    BackupError {
        kind: match error {
            Error::Unclassified => "offline_export_required",
            Error::Bounds => "too_large",
            Error::Unsupported => "unavailable",
            Error::NotQuiescent => "busy",
            _ => "backup_unconfirmed",
        },
    }
}
impl NativeProfileTransition {
    fn preserve_current_bundle(
        &self,
        destination: &Path,
        limits: &Limits,
    ) -> Result<VerifiedBundle> {
        self.registry
            .with_frozen(Duration::from_secs(30), |frozen| {
                frozen.revalidate()?;
                let root = frozen.binding().locator.restore()?;
                // Discover after real writers have drained, never before flush.
                let choices = super::backup_inventory::selections(&root, limits)?;
                self.registry.validate_choices(&choices)?;
                let inventory = Inventory::capture(&root, &choices, limits)?;
                let mut owner = BorrowedFreeze { frozen };
                mdreader_profile::bundle::stage_bundle(&mut owner, inventory, destination, limits)
            })
    }
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_backup(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, PersistenceState>,
) -> std::result::Result<BackupReply, BackupError> {
    let transition = state.transition.clone();
    // Do not ask for a destination when an actual owner was not composed.
    // Registration is only admission: real freeze/drain is still mandatory.
    if !transition.missing_participants().is_empty() {
        return Err(failure(Error::NotQuiescent));
    }
    let json = state.owner.clone();
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(destination) = app
            .dialog()
            .file()
            .set_title("Back up reader data — choose a folder")
            .blocking_pick_folder()
        else {
            return Ok(BackupReply {
                state: BackupState::Cancelled,
            });
        };
        let destination = destination
            .into_path()
            .map_err(|_| failure(Error::Invalid))?;
        let parent = SourceRoot::capture(destination).map_err(|_| failure(Error::Invalid))?;
        let profile =
            crate::reader_profile::directory(&app).map_err(|_| failure(Error::Conflict))?;
        if parent.path.starts_with(&profile) || profile.starts_with(&parent.path) {
            return Err(failure(Error::Invalid));
        }
        let (registered, _) = json.renderer().map_err(|_| failure(Error::NotQuiescent))?;
        if registered != label {
            return Err(failure(Error::NotQuiescent));
        }
        parent.verify().map_err(|_| failure(Error::Conflict))?;
        // tempfile creates an exclusive private directory. Keep it on failure:
        // uncertain staged output must not be deleted through a raced pathname.
        let directory = tempfile::Builder::new()
            .prefix("MDReader-backup-")
            .tempdir_in(&parent.path)
            .map_err(|_| failure(Error::Storage))?
            .keep();
        parent.verify().map_err(|_| failure(Error::Conflict))?;
        let limits = Limits::default();
        let bundle = transition
            .preserve_current_bundle(&directory.join("bundle"), &limits)
            .map_err(failure)?;
        bundle.revalidate(&limits).map_err(failure)?;
        parent.verify().map_err(|_| failure(Error::Conflict))?;
        Ok(BackupReply {
            state: BackupState::Exported,
        })
    })
    .await
    .map_err(|_| failure(Error::ReconciliationRequired))?
}
