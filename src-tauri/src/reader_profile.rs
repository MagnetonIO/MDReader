//! Explicit native profile selection. A prepared profile never inherits the old
//! profile's source grants or credentials; it is a separate rollback boundary.
use mdreader_core::SourceRoot;
use std::path::{Path, PathBuf};
use tauri::Manager;
struct ProfileRoot(SourceRoot);

#[cfg(all(unix, not(mobile)))]
pub(crate) struct SelectionLaunch {
    root: SourceRoot,
    journal: PathBuf,
}
#[cfg(all(unix, not(mobile)))]
pub(crate) fn selection_launch(journal: PathBuf) -> Result<SelectionLaunch, String> {
    let root = crate::profile_transition::offline::workspace::selected_for_launch(&journal)
        .map_err(|_| "Selected profile integrity failed")?;
    Ok(SelectionLaunch { root, journal })
}
#[cfg(not(all(unix, not(mobile))))]
pub(crate) struct SelectionLaunch;

pub(crate) fn configure(
    app: &tauri::AppHandle,
    selected: Option<&Path>,
    transition: Option<&SelectionLaunch>,
) -> Result<(), String> {
    #[cfg(all(unix, not(mobile)))]
    if let Some(transition) = transition {
        if selected.is_some() {
            return Err("Conflicting profile selection".into());
        }
        let current =
            crate::profile_transition::offline::workspace::selected_for_launch(&transition.journal)
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
        return Ok(());
    }
    #[cfg(not(all(unix, not(mobile))))]
    if transition.is_some() {
        return Err("Selected profiles unavailable on this platform".into());
    }

    let path = match selected {
        Some(path) => {
            #[cfg(all(unix, not(mobile)))]
            let native = path
                .join("native-reader-profile.json")
                .try_exists()
                .map_err(|_| "Prepared profile integrity failed")?;
            #[cfg(not(all(unix, not(mobile))))]
            let native = false;
            if native {
                #[cfg(all(unix, not(mobile)))]
                crate::profile_transition::offline::workspace::verify_prepared(path)
                    .map_err(|_| "Prepared profile integrity failed")?;
            } else {
                mdreader_legacy_export::migration::verify(path, &Default::default())
                    .map_err(|_| "Prepared profile integrity failed")?;
            }
            path.to_owned()
        }
        None => app
            .path()
            .app_data_dir()
            .map_err(|_| "Reader profile unavailable")?,
    };
    create_profile_directory(&path).map_err(|_| "Reader profile unavailable")?;
    let root = SourceRoot::capture(
        path.canonicalize()
            .map_err(|_| "Reader profile unavailable")?,
    )
    .map_err(|_| "Reader profile unavailable")?;
    if !app.manage(ProfileRoot(root)) {
        return Err("Reader profile already selected".into());
    }
    Ok(())
}

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
