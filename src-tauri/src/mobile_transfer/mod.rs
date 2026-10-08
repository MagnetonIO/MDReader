//! Mobile-only platform handoff over the existing embedded document/asset owner.
mod package;
mod snapshot;
mod staging;
mod types;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::Manager;
pub use types::*;

#[derive(Default)]
pub(crate) struct TransferRuntime(Arc<AtomicBool>);
struct Permit(Arc<AtomicBool>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_transfer_capabilities(app: tauri::AppHandle) -> Capabilities {
    let available = mdreader_mobile_transfer::available(&app);
    Capabilities {
        export: available,
        share: available,
        reading_links: mdreader_mobile_transfer::links_available(&app),
    }
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_take_reading_links(
    app: tauri::AppHandle,
) -> Result<PendingLinks, String> {
    let pending = mdreader_mobile_transfer::take_links(&app).map_err(str::to_string)?;
    Ok(PendingLinks {
        links: pending.links,
        truncated: pending.truncated,
    })
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_transfer(
    app: tauri::AppHandle,
    backend: tauri::State<'_, crate::reader::ReaderBackend>,
    runtime: tauri::State<'_, crate::mobile_assets::AssetRuntime>,
    transfers: tauri::State<'_, TransferRuntime>,
    reference: Reference,
    action: Action,
) -> Result<Reply, String> {
    if !mdreader_mobile_transfer::available(&app) {
        return Err("unsupported".into());
    }
    transfers
        .0
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "busy")?;
    let permit = Permit(transfers.0.clone());
    let backend = backend.inner().clone();
    let runtime = runtime.inner().clone();
    let profile = crate::reader_profile::directory(&app).map_err(|_| "unavailable")?;
    let cache = app.path().app_cache_dir().map_err(|_| "unavailable")?;
    // The worker owns admission through the OS sheet, including lost IPC replies.
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        let copy = package::prepare(&backend, &runtime, &profile, &cache, &reference)?;
        if crate::reader_profile::directory(&app).map_err(|_| "stale")? != profile {
            return Err("stale".into());
        }
        let reply = mdreader_mobile_transfer::present(
            &app,
            mdreader_mobile_transfer::CopyRequest {
                path: copy.path,
                name: copy.name,
                mime: copy.mime,
                sha256: copy.sha256,
                bytes: copy.bytes,
                action: match action {
                    Action::Export => mdreader_mobile_transfer::Action::Export,
                    Action::Share => mdreader_mobile_transfer::Action::Share,
                },
            },
        )
        .map_err(str::to_string)?;
        Ok(Reply {
            state: match reply.state {
                mdreader_mobile_transfer::State::Exported => TransferState::Exported,
                mdreader_mobile_transfer::State::Presented => TransferState::Presented,
                mdreader_mobile_transfer::State::Cancelled => TransferState::Cancelled,
            },
        })
    })
    .await
    .map_err(|_| "transfer_unconfirmed".to_string())?
}

#[cfg(test)]
mod tests;
