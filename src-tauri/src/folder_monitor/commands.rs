use crate::{
    library::{LibraryError, LibraryState, LibraryStore, PickedLibrarySource},
    reader::ReaderBackend,
};
use mdreader_service::Request;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderIndexStatus {
    source_id: String,
    monitor_enabled: bool,
    phase: String,
    known_documents: f64,
    indexed_documents: f64,
    observed_at_ms: Option<f64>,
    complete: bool,
    error: Option<String>,
}
fn number(n: u64) -> Result<f64, String> {
    if n > 9_007_199_254_740_991 {
        return Err("Folder status is unavailable".into());
    }
    Ok(n as f64)
}
fn list(app: &AppHandle, backend: &ReaderBackend) -> Result<Vec<FolderIndexStatus>, String> {
    let (path, _) =
        crate::library::registry_path(app).map_err(|_| "Saved folders are unavailable")?;
    let members = LibraryStore::new(path)
        .read_query_snapshot()
        .map_err(|_| "Saved folders are unavailable")?
        .local_sources;
    if members.len() > 128 {
        return Err("Saved folders exceed supported count".into());
    }
    let mut states = Vec::new();
    for member in members {
        let status = member
            .persisted_binding()
            .ok()
            .and_then(|(path, identity)| {
                backend
                    .typed::<Option<mdreader_core::Source>>(Request::FindSource { path, identity })
                    .ok()
                    .flatten()
            })
            .and_then(|source| {
                backend
                    .typed::<mdreader_core::FolderIndexStatus>(Request::FolderIndexStatus {
                        source_id: source.id,
                    })
                    .ok()
            });
        if let Some(mut s) = status {
            let unavailable = app
                .try_state::<super::FolderMonitoring>()
                .is_some_and(|owner| {
                    owner
                        .unavailable
                        .lock()
                        .is_ok_and(|bad| bad.contains(&member.id))
                });
            if unavailable && s.monitor_enabled {
                s.phase = "unavailable".into();
                s.complete = false;
                s.error = Some("Folder watch is unavailable".into());
            }
            states.push(FolderIndexStatus {
                source_id: member.id,
                monitor_enabled: s.monitor_enabled,
                phase: s.phase,
                known_documents: number(s.known_documents)?,
                indexed_documents: number(s.indexed_documents)?,
                observed_at_ms: s
                    .observed_at_ms
                    .map(|v| {
                        u64::try_from(v)
                            .map_err(|_| "Folder status is unavailable".to_string())
                            .and_then(number)
                    })
                    .transpose()?,
                complete: s.complete,
                error: s
                    .error
                    .map(|_| "Folder indexing failed; retained results are kept".into()),
            });
        } else {
            states.push(FolderIndexStatus {
                source_id: member.id,
                monitor_enabled: true,
                phase: "unavailable".into(),
                known_documents: 0.,
                indexed_documents: 0.,
                observed_at_ms: None,
                complete: false,
                error: Some("Folder is unavailable".into()),
            });
        }
    }
    Ok(states)
}
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn folder_indexing_state(
    app: AppHandle,
    backend: State<'_, ReaderBackend>,
) -> Result<Vec<FolderIndexStatus>, String> {
    list(&app, &backend)
}
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn set_folder_monitoring(
    app: AppHandle,
    source_id: String,
    enabled: bool,
    backend: State<'_, ReaderBackend>,
) -> Result<Vec<FolderIndexStatus>, String> {
    let (path, _) =
        crate::library::registry_path(&app).map_err(|_| "Saved folders are unavailable")?;
    let member = LibraryStore::new(path)
        .read_query_snapshot()
        .map_err(|_| "Saved folders are unavailable")?
        .local_sources
        .into_iter()
        .find(|m| m.id == source_id)
        .ok_or("Saved folder is unavailable")?;
    let (path, identity) = member
        .persisted_binding()
        .map_err(|_| "Saved folder is unavailable")?;
    let source: mdreader_core::Source = backend
        .typed::<Option<mdreader_core::Source>>(Request::FindSource { path, identity })?
        .ok_or("Saved folder is unavailable")?;
    let source = source.id;
    let _: serde_json::Value = backend.typed(Request::SetFolderMonitoring {
        source_id: source,
        enabled,
    })?;
    list(&app, &backend)
}
#[derive(Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BroadFolderScope {
    Home,
    Disk,
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_broad_library_source(
    app: AppHandle,
    scope: BroadFolderScope,
    confirmed: bool,
    workspace: State<'_, crate::commands::WorkspaceState>,
    state: State<'_, LibraryState>,
) -> Result<PickedLibrarySource, LibraryError> {
    if !confirmed {
        return Err(LibraryError::InvalidRoot);
    }
    #[cfg(mobile)]
    {
        let _ = (app, scope, workspace, state);
        Err(LibraryError::NeedsNativeRepick)
    }
    #[cfg(not(mobile))]
    {
        use tauri_plugin_dialog::DialogExt;
        let home = app
            .path()
            .home_dir()
            .map_err(|_| LibraryError::InvalidRoot)?;
        let initial = match scope {
            BroadFolderScope::Home => home,
            BroadFolderScope::Disk => home
                .ancestors()
                .last()
                .ok_or(LibraryError::InvalidRoot)?
                .to_path_buf(),
        };
        let Some(selected) = app
            .dialog()
            .file()
            .set_directory(&initial)
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
        else {
            return Ok(PickedLibrarySource {
                source: None,
                added: false,
            });
        };
        let canonical = selected
            .canonicalize()
            .map_err(|_| LibraryError::InvalidRoot)?;
        if canonical
            != initial
                .canonicalize()
                .map_err(|_| LibraryError::InvalidRoot)?
        {
            return Err(LibraryError::InvalidRoot);
        }
        crate::library::publish_picked_source(&app, &workspace, &state, &selected, true)
    }
}
