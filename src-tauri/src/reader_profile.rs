//! Explicit native profile selection. A prepared profile never inherits the old
//! profile's source grants or credentials; it is a separate rollback boundary.
use mdreader_core::SourceRoot;
use std::path::{Path, PathBuf};
use tauri::Manager;
struct ProfileRoot(SourceRoot);

pub(crate) fn configure(app: &tauri::AppHandle, selected: Option<&Path>) -> Result<(), String> {
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
    std::fs::create_dir_all(&path).map_err(|_| "Reader profile unavailable")?;
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
pub(crate) fn directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(root) = app.try_state::<ProfileRoot>() {
        root.0.verify().map_err(|_| "Reader profile changed")?;
        Ok(root.0.path.clone())
    } else {
        Err("Reader profile is not initialized".into())
    }
}
