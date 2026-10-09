//! Native-selected output and all-owner profile preservation. This exports a
//! copy; it never activates another profile or transfers OS-vault credentials.
use super::{persistence::commands::PersistenceState, BorrowedFreeze, NativeProfileTransition};
use mdreader_core::SourceRoot;
use mdreader_profile::{bundle::VerifiedBundle, Error, Inventory, Limits, Result};
use serde::Serialize;
use std::{path::Path, time::Duration};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

fn private_destination(parent: &Path) -> Result<std::path::PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // TempDir otherwise inherits the process umask (commonly creating 0755).
        // Set permissions on creation, before any private snapshot is written.
        tempfile::Builder::new()
            .prefix("MDReader-backup-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .map(tempfile::TempDir::keep)
            .map_err(|_| Error::Storage)
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Err(Error::Unsupported)
    }
}

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
        diagnostics: &crate::diagnostics::DiagnosticsState,
    ) -> Result<VerifiedBundle> {
        self.registry
            .with_frozen(Duration::from_secs(30), |frozen| {
                frozen.revalidate()?;
                let root = frozen.binding().locator.restore()?;
                // Acquire diagnostics only after renderer/owner flush, so a
                // renderer awaiting its diagnostic event cannot block that flush.
                diagnostics.with_preservation(&root, || {
                    let choices = super::backup_inventory::selections(&root, limits)?;
                    self.registry.validate_choices(&choices)?;
                    let inventory = Inventory::capture(&root, &choices, limits)?;
                    let mut owner = BorrowedFreeze { frozen };
                    mdreader_profile::bundle::stage_bundle(
                        &mut owner,
                        inventory,
                        destination,
                        limits,
                    )
                })
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
    let transition = state
        .transition
        .clone()
        .ok_or_else(|| failure(Error::Unsupported))?;
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
        // Create an exclusive private directory. Keep it on failure:
        // uncertain staged output must not be deleted through a raced pathname.
        let directory = private_destination(&parent.path).map_err(failure)?;
        parent.verify().map_err(|_| failure(Error::Conflict))?;
        let limits = Limits::default();
        let bundle = transition
            .preserve_current_bundle(
                &directory.join("bundle"),
                &limits,
                &app.state::<crate::diagnostics::DiagnosticsState>(),
            )
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn backup_destination_is_private_before_snapshot_publication() {
        let parent = tempfile::tempdir().unwrap();
        let destination = private_destination(parent.path()).unwrap();
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
        assert!(destination.starts_with(parent.path()));
    }
}
