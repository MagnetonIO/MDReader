//! One account actor over the already opened embedded ReaderStore and the exact
//! AssetRuntime used by mobile reading. No additional SQLite writer or service.
mod browser;
mod configuration;
#[cfg(target_os = "android")]
mod vault;
use mdreader_service::accounts::{AccountError, CacheRoots, Coordinator};
use std::sync::Arc;
use tauri::Manager;
pub(crate) fn configure(
    app: &tauri::AppHandle,
    backend: &crate::reader::ReaderBackend,
) -> Result<(), String> {
    let owner = backend.mobile_owner()?;
    let roots = crate::reader_profile::directory(app)?;
    let setup = owner
        .lock()
        .map_err(|_| "Account owner is busy")?
        .profile_uuid()
        .map_err(|_| AccountError::Storage)
        .and_then(|profile| configuration::setup(app, profile));
    let coordinator = setup
        .and_then(|setup| {
            Coordinator::with_asset_runtime(
                owner,
                setup,
                CacheRoots {
                    documents: roots.join("device-markdown-v1"),
                    assets: roots.join("device-assets-v1"),
                },
                app.state::<crate::mobile_assets::AssetRuntime>()
                    .inner()
                    .clone(),
            )
        })
        .map(Arc::new);
    backend.bind_mobile_accounts(coordinator)
}
