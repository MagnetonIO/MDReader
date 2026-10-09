//! Plain Markdown from the exact admitted provider cache. No fetching, attachment
//! inference or conversion to a device copy; the renderer never supplies bytes.
use super::{package::PreparedCopy, staging};
use crate::{mobile_provider_state::ProviderReadingReference, reader::ReaderBackend};
use std::{io::Write, path::Path};
fn read(backend: &ReaderBackend, reference: &ProviderReadingReference) -> Result<String, String> {
    let exact = reference.core()?;
    backend.with_mobile_store(|store| {
        let body = store
            .read_document(&exact.source_id, &exact.path, Some(&exact.revision))
            .map_err(|_| "stale")?;
        if body.markdown.len() > 2 * 1024 * 1024 {
            return Err("too_large".into());
        }
        Ok(body.markdown)
    })
}
pub(super) fn prepare(
    backend: &ReaderBackend,
    cache: &Path,
    reference: &ProviderReadingReference,
) -> Result<PreparedCopy, String> {
    let markdown = read(backend, reference)?;
    // This is a generated display filename, never a provider path or remote key.
    let name = "provider-document.md";
    let mut copy = staging::create(cache, name)?;
    copy.file
        .write_all(markdown.as_bytes())
        .map_err(|_| "transfer_unconfirmed")?;
    if read(backend, reference)? != markdown {
        return Err("stale".into());
    }
    let (path, bytes, sha256) = copy.finish()?;
    Ok(PreparedCopy {
        path: path.to_str().ok_or("unavailable")?.into(),
        name: name.into(),
        mime: "text/markdown".into(),
        bytes,
        sha256,
    })
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_provider_transfer(
    app: tauri::AppHandle,
    backend: tauri::State<'_, ReaderBackend>,
    transfers: tauri::State<'_, super::TransferRuntime>,
    reference: ProviderReadingReference,
    action: super::Action,
) -> Result<super::Reply, String> {
    if !cfg!(mobile) || !mdreader_mobile_transfer::available(&app) {
        return Err("unsupported".into());
    }
    reference.core()?;
    transfers
        .0
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .map_err(|_| "busy")?;
    let permit = super::Permit(transfers.0.clone());
    let backend = backend.inner().clone();
    let profile = crate::reader_profile::directory(&app).map_err(|_| "unavailable")?;
    use tauri::Manager;
    let cache = app.path().app_cache_dir().map_err(|_| "unavailable")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        let copy = prepare(&backend, &cache, &reference)?;
        if crate::reader_profile::directory(&app).map_err(|_| "stale")? != profile {
            return Err("stale".into());
        }
        let reply = mdreader_mobile_transfer::present(
            &app,
            mdreader_mobile_transfer::CopyRequest {
                path: copy.path.clone(),
                name: copy.name,
                mime: copy.mime,
                sha256: copy.sha256,
                bytes: copy.bytes,
                action: match action {
                    super::Action::Export => mdreader_mobile_transfer::Action::Export,
                    super::Action::Share => mdreader_mobile_transfer::Action::Share,
                },
            },
        )
        .map_err(str::to_string)?;
        if matches!(
            reply.state,
            mdreader_mobile_transfer::State::Exported | mdreader_mobile_transfer::State::Cancelled
        ) {
            let _ = staging::release(&cache, Path::new(&copy.path));
        }
        Ok(super::Reply {
            state: match reply.state {
                mdreader_mobile_transfer::State::Exported => super::TransferState::Exported,
                mdreader_mobile_transfer::State::Presented => super::TransferState::Presented,
                mdreader_mobile_transfer::State::Cancelled => super::TransferState::Cancelled,
            },
        })
    })
    .await
    .map_err(|_| "transfer_unconfirmed".to_string())?
}
