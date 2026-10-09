//! Explicit native profile selection. A prepared profile never inherits the old
//! profile's source grants or credentials; it is a separate rollback boundary.
use mdreader_core::SourceRoot;
use std::path::{Path, PathBuf};
use tauri::Manager;
struct ProfileRoot(SourceRoot);
#[cfg(all(unix, not(mobile)))]
mod default_selection;
#[cfg(all(feature = "e2e-build", not(mobile)))]
mod e2e;
#[cfg(all(unix, not(mobile)))]
mod return_controller;
#[derive(serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileStartup {
    migration_required: bool,
    using_imported_library: bool,
    return_controller: bool,
}
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn native_profile_startup(app: tauri::AppHandle) -> Result<ProfileStartup, String> {
    let info = app
        .try_state::<ProfileStartup>()
        .ok_or("Reader startup unavailable")?;
    Ok(ProfileStartup {
        migration_required: info.migration_required,
        using_imported_library: info.using_imported_library,
        return_controller: info.return_controller,
    })
}
#[cfg(all(unix, not(mobile)))]
pub(crate) fn remember_imported(
    app: &tauri::AppHandle,
    path: &Path,
) -> mdreader_profile::Result<()> {
    app.try_state::<default_selection::DefaultSelection>()
        .ok_or(mdreader_profile::Error::Conflict)?
        .select(path)
}
#[cfg(all(unix, not(mobile)))]
pub(crate) fn is_imported_selected(
    app: &tauri::AppHandle,
    path: &Path,
) -> mdreader_profile::Result<bool> {
    let root =
        SourceRoot::capture(path.to_owned()).map_err(|_| mdreader_profile::Error::Conflict)?;
    let selected = app
        .try_state::<default_selection::DefaultSelection>()
        .ok_or(mdreader_profile::Error::Conflict)?
        .selected()?;
    Ok(selected.is_some_and(|selected| selected == root))
}
#[cfg(all(unix, not(mobile)))]
pub(crate) fn return_target(app: &tauri::AppHandle) -> mdreader_profile::Result<SourceRoot> {
    return_controller::target(app)
}

#[cfg(all(unix, not(mobile)))]
pub(crate) fn forget_imported(app: &tauri::AppHandle, path: &Path) -> mdreader_profile::Result<()> {
    app.try_state::<default_selection::DefaultSelection>()
        .ok_or(mdreader_profile::Error::Conflict)?
        .clear(path)
}

#[cfg(all(any(unix, windows), not(mobile)))]
pub(crate) struct SelectionLaunch {
    root: SourceRoot,
    journal: PathBuf,
}
#[cfg(all(any(unix, windows), not(mobile)))]
pub(crate) fn selection_launch(journal: PathBuf) -> Result<SelectionLaunch, String> {
    let root = selected_for_launch(&journal).map_err(|_| "Selected profile integrity failed")?;
    Ok(SelectionLaunch { root, journal })
}
#[cfg(not(all(any(unix, windows), not(mobile))))]
pub(crate) struct SelectionLaunch;
#[cfg(all(any(unix, windows), not(mobile)))]
fn selected_for_launch(journal: &Path) -> mdreader_profile::Result<SourceRoot> {
    #[cfg(unix)]
    return crate::profile_transition::offline::workspace::selected_for_launch(journal);
    #[cfg(windows)]
    return crate::profile_transition::offline::windows_workspace::selected_for_launch(journal);
}

pub(crate) fn configure(
    app: &tauri::AppHandle,
    selected: Option<&Path>,
    transition: Option<&SelectionLaunch>,
    controller: Option<&Path>,
) -> Result<(), String> {
    let mut info = ProfileStartup {
        migration_required: false,
        using_imported_library: false,
        return_controller: false,
    };
    // A test-owned root is selected before any database, registry or JSON owner.
    // This branch does not exist in shipping builds and never writes a selector.
    #[cfg(all(feature = "e2e-build", not(mobile)))]
    if let Some(path) = std::env::var_os("MDREADER_E2E_PROFILE_ROOT") {
        let root = e2e::capture(
            &app.config().identifier,
            Path::new(&path),
            selected.is_some() || transition.is_some() || controller.is_some(),
        )?;
        if !app.manage(ProfileRoot(root)) || !app.manage(info) {
            return Err("E2E reader profile already selected".into());
        }
        return Ok(());
    }
    #[cfg(all(unix, not(mobile)))]
    let default = {
        let standard = app
            .path()
            .app_data_dir()
            .map_err(|_| "Reader profile unavailable")?;
        let slot = default_selection::DefaultSelection::open(&standard)
            .map_err(|_| "Reader profile selection unavailable; libraries were kept")?;
        let mut path = None;
        if let Some(candidate) = controller {
            if selected.is_some() || transition.is_some() {
                return Err("Conflicting profile selection".into());
            }
            let target = return_controller::validate(&slot, candidate)
                .map_err(|_| "Return selection unavailable; libraries retained")?;
            path = Some(
                slot.return_controller()
                    .map_err(|_| "Separate controller unavailable")?,
            );
            app.manage(return_controller::Target(target));
            info.return_controller = true;
        } else if selected.is_none() && transition.is_none() {
            if let Some(root) = slot
                .selected()
                .map_err(|_| "Selected library unavailable; libraries were kept")?
            {
                info.using_imported_library = true;
                path = Some(root);
            } else if standard
                .try_exists()
                .map_err(|_| "Reader profile unavailable")?
            {
                let original =
                    SourceRoot::capture(standard).map_err(|_| "Original library unavailable")?;
                if crate::profile_transition::offline::legacy_profile(&original)
                    .map_err(|_| "Original library unavailable")?
                {
                    info.migration_required = true;
                    path = Some(
                        slot.bootstrap()
                            .map_err(|_| "Separate reader unavailable")?,
                    );
                }
            }
        }
        if !app.manage(slot) {
            return Err("Reader selection already initialized".into());
        }
        path
    };
    #[cfg(not(all(unix, not(mobile))))]
    let default: Option<SourceRoot> = None;
    #[cfg(windows)]
    if controller.is_some() {
        return Err("Return controllers unavailable on this platform".into());
    }
    #[cfg(all(any(unix, windows), not(mobile)))]
    if let Some(transition) = transition {
        if selected.is_some() {
            return Err("Conflicting profile selection".into());
        }
        let current = selected_for_launch(&transition.journal)
            .map_err(|_| "Selected profile integrity failed")?;
        transition
            .root
            .verify()
            .map_err(|_| "Selected profile changed")?;
        if current.path != transition.root.path || current.identity != transition.root.identity {
            return Err("Selected profile changed".into());
        }
        if !app.manage(ProfileRoot(current)) {
            return Err("Reader profile already selected".into());
        }
        info.using_imported_library = true;
        app.manage(info);
        return Ok(());
    }
    #[cfg(not(all(any(unix, windows), not(mobile))))]
    if transition.is_some() || controller.is_some() {
        return Err("Selected profiles unavailable on this platform".into());
    }

    let mut retained = default;
    let path = match selected {
        Some(path) => {
            let original =
                SourceRoot::capture(path.to_owned()).map_err(|_| "Prepared profile unavailable")?;
            #[cfg(all(unix, not(mobile)))]
            let native = path
                .join("native-reader-profile.json")
                .try_exists()
                .map_err(|_| "Prepared profile integrity failed")?;
            #[cfg(all(windows, not(mobile)))]
            let native = path.file_name() == Some(std::ffi::OsStr::new("candidate"))
                && path
                    .parent()
                    .is_some_and(|parent| parent.join("control/library.sqlite").is_file());
            #[cfg(not(all(any(unix, windows), not(mobile))))]
            let native = false;
            if native {
                #[cfg(all(unix, not(mobile)))]
                crate::profile_transition::offline::workspace::verify_prepared(path)
                    .map_err(|_| "Prepared profile integrity failed")?;
                #[cfg(all(windows, not(mobile)))]
                crate::profile_transition::offline::windows_workspace::verify_profile(path)
                    .map_err(|_| "Prepared profile integrity failed")?;
                info.using_imported_library = true;
            } else {
                let profile = mdreader_legacy_export::migration::verify(path, &Default::default())
                    .map_err(|_| "Prepared profile integrity failed")?;
                info.using_imported_library = profile.version == 2;
            }
            original.verify().map_err(|_| "Prepared profile changed")?;
            retained = Some(original);
            path.to_owned()
        }
        None => match &retained {
            Some(root) => root.path.clone(),
            None => app
                .path()
                .app_data_dir()
                .map_err(|_| "Reader profile unavailable")?,
        },
    };
    if let Some(root) = &retained {
        root.verify().map_err(|_| "Selected profile changed")?;
    } else {
        create_profile_directory(&path).map_err(|_| "Reader profile unavailable")?;
    }
    let root = SourceRoot::capture(
        path.canonicalize()
            .map_err(|_| "Reader profile unavailable")?,
    )
    .map_err(|_| "Reader profile unavailable")?;
    if let Some(retained) = &retained {
        retained.verify().map_err(|_| "Selected profile changed")?;
        if retained.identity != root.identity {
            return Err("Selected profile changed".into());
        }
    }
    if !app.manage(ProfileRoot(root)) {
        return Err("Reader profile already selected".into());
    }
    app.manage(info);
    Ok(())
}

#[cfg(not(windows))]
fn create_profile_directory(path: &Path) -> std::io::Result<()> {
    let mut directory = std::fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(path)
}
#[cfg(windows)]
fn create_profile_directory(path: &Path) -> std::io::Result<()> {
    // Create the current-user/SYSTEM-only DACL on the initial object. Existing
    // broad/inherited profiles are refused, not silently repaired or adopted.
    mdreader_capture::PrivateDirectory::create(path)
        .map(|_| ())
        .map_err(|_| std::io::Error::other("Private reader profile unavailable"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn new_profile_is_private_without_changing_existing_directory_permissions() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("new-profile");
        create_profile_directory(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        create_profile_directory(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }
}
pub(crate) fn directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(root) = app.try_state::<ProfileRoot>() {
        root.0.verify().map_err(|_| "Reader profile changed")?;
        Ok(root.0.path.clone())
    } else {
        Err("Reader profile is not initialized".into())
    }
}
