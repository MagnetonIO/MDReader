//! Explicit desktop archive selection. Renderer never supplies a stage path;
//! source references come from existing grants and exact service revisions.
use crate::{commands::WorkspaceState, reader::ReaderBackend};
use mdreader_service::{Request, reviews as service};
use serde::Deserialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

mod model;
pub use model::*;
use model::{failure, reply};

fn local_source(
    reader: &ReaderBackend,
    epoch: &crate::engine::Epoch,
) -> Result<String, ArchivedReviewsError> {
    epoch.validate_root().map_err(|_| failure("conflict"))?;
    let root = epoch.source_root();
    let source: Option<mdreader_core::Source> = reader
        .typed(Request::FindSource {
            path: root.path,
            identity: root.identity,
        })
        .map_err(|_| failure("unavailable"))?;
    source
        .filter(|value| value.enabled)
        .map(|value| value.id)
        .ok_or_else(|| failure("unavailable"))
}
fn current(
    reader: &ReaderBackend,
    epoch: &crate::engine::Epoch,
    path: &str,
) -> Result<ReviewCurrent, ArchivedReviewsError> {
    let source_id = local_source(reader, epoch)?;
    let relative = epoch
        .source_root()
        .relative_path(std::path::Path::new(path))
        .map_err(|_| failure("invalid"))?;
    let relative = relative
        .to_str()
        .ok_or_else(|| failure("unsupported"))?
        .replace(std::path::MAIN_SEPARATOR, "/");
    let document: mdreader_core::Document = reader
        .typed(Request::Read {
            source_id,
            path: relative,
            revision: None,
        })
        .map_err(|_| failure("unavailable"))?;
    epoch.validate_root().map_err(|_| failure("conflict"))?;
    Ok(ReviewCurrent {
        source_id: document.source_id,
        path: document.path,
        revision: document.revision,
    })
}
fn context(
    workspace: &WorkspaceState,
    generation: Option<u32>,
) -> Result<std::sync::Arc<crate::engine::Epoch>, ArchivedReviewsError> {
    let epoch = workspace.current().ok_or_else(|| failure("unavailable"))?;
    if generation.map(u64::from) != Some(epoch.generation) {
        return Err(failure("conflict"));
    }
    Ok(epoch)
}
#[tauri::command]
#[specta::specta]
pub async fn reader_archived_reviews(
    app: AppHandle,
    request: ArchivedReviewsRequest,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<ArchivedReviewsReply, ArchivedReviewsError> {
    if !cfg!(unix) {
        return Err(failure("unsupported"));
    }
    // Capture the current local era once, never discover a source from a legacy locator.
    let needs_local = matches!(&request, ArchivedReviewsRequest::Current { .. })
        || matches!(&request,ArchivedReviewsRequest::Compare{current,generation,..} if generation.is_some() && !current.source_id.starts_with("provider:"));
    let generation = match &request {
        ArchivedReviewsRequest::Current { generation, .. } => Some(*generation),
        ArchivedReviewsRequest::Compare { generation, .. } => *generation,
        _ => None,
    };
    let epoch = if needs_local {
        Some(context(&workspace, generation)?)
    } else {
        None
    };
    let worker_epoch = epoch.clone();
    let backend = reader.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let local_membership = match &request {
            ArchivedReviewsRequest::Compare {
                current,
                generation: None,
                ..
            } if !current.source_id.starts_with("provider:") => Some(
                crate::reader_catalog::LocalMembership::capture(&app, &backend, &current.source_id)
                    .map_err(|_| failure("conflict"))?,
            ),
            _ => None,
        };
        let operation = match request {
            ArchivedReviewsRequest::Select {
                export_id,
                manifest_sha256,
            } => {
                let export_id =
                    uuid::Uuid::parse_str(&export_id).map_err(|_| failure("invalid"))?;
                if export_id.get_version_num() != 4
                    || manifest_sha256.len() != 64
                    || !manifest_sha256.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(failure("invalid"));
                }
                let Some(stage) = app
                    .dialog()
                    .file()
                    .set_title("Select verified archival review stage")
                    .blocking_pick_folder()
                    .and_then(|selected| selected.into_path().ok())
                else {
                    return Ok(ArchivedReviewsReply::Cancelled);
                };
                service::ReviewRequest::Select {
                    stage,
                    identity: mdreader_legacy_export::reviews::ArchiveIdentity {
                        export_id,
                        manifest_sha256: manifest_sha256.to_ascii_lowercase(),
                    },
                }
            }
            ArchivedReviewsRequest::Current { path, .. } => {
                return Ok(ArchivedReviewsReply::Current {
                    reference: current(
                        &backend,
                        worker_epoch
                            .as_deref()
                            .ok_or_else(|| failure("unavailable"))?,
                        &path,
                    )?,
                });
            }
            ArchivedReviewsRequest::List {
                handle,
                offset,
                limit,
            } => service::ReviewRequest::List {
                handle,
                offset: offset.into(),
                limit,
            },
            ArchivedReviewsRequest::Get { handle, record_id } => {
                service::ReviewRequest::Get { handle, record_id }
            }
            ArchivedReviewsRequest::Close { handle } => service::ReviewRequest::Close { handle },
            ArchivedReviewsRequest::Compare {
                handle,
                record_id,
                current,
                ..
            } => {
                if let Some(epoch) = worker_epoch.as_deref() {
                    if local_source(&backend, epoch)? != current.source_id {
                        return Err(failure("conflict"));
                    }
                }
                service::ReviewRequest::Compare {
                    handle,
                    record_id,
                    source_id: current.source_id,
                    path: current.path,
                    revision: current.revision,
                }
            }
        };
        #[derive(Deserialize)]
        struct Envelope {
            result: Option<service::ReviewReply>,
            error: Option<service::ReviewError>,
        }
        let response: Envelope = backend
            .typed(Request::Reviews { request: operation })
            .map_err(|_| failure("unavailable"))?;
        let result = match (response.result, response.error) {
            (Some(value), None) => reply(value),
            (None, Some(error)) => Err(error.into()),
            _ => Err(failure("unavailable")),
        };
        if let Some(membership) = local_membership {
            membership
                .revalidate(&app, &backend)
                .map_err(|_| failure("conflict"))?;
        }
        result
    })
    .await
    .map_err(|_| failure("storage"))?;
    if let Some(epoch) = epoch {
        epoch
            .finish(&workspace, ())
            .map_err(|_| failure("conflict"))?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_local_selection_never_regrants_and_old_same_root_context_fails() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        let root = notes.path().canonicalize().unwrap();
        let path = root.join("note.md");
        std::fs::write(&path, "# Exact").unwrap();
        let backend = ReaderBackend::embedded(data.path()).unwrap();
        let source = backend
            .admit(&mdreader_core::SourceRoot::capture(root.clone()).unwrap())
            .unwrap();
        let workspace = WorkspaceState::granted(root.clone());
        let epoch = context(&workspace, Some(1)).unwrap();
        let reference = current(&backend, &epoch, path.to_str().unwrap()).unwrap();
        assert_eq!(reference.source_id, source);
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id: source.clone(),
                enabled: false,
            })
            .unwrap();
        assert!(current(&backend, &epoch, path.to_str().unwrap()).is_err());
        let rows: Vec<mdreader_core::Source> = backend.typed(Request::ListSources).unwrap();
        assert!(!rows[0].enabled);
        workspace.switch(root).unwrap();
        assert!(context(&workspace, Some(1)).is_err());
        assert!(context(&workspace, Some(2)).is_ok());
    }
}
