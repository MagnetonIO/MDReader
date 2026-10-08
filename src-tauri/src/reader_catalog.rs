//! Native membership adapter. Catalog reads never admit or regrant a folder.
mod model;
use crate::{
    library::{LibraryStore, LocalFolderSource},
    reader::ReaderBackend,
};
use mdreader_service::{Request, catalog as service};
pub use model::*;
use model::{error_kind, failure};
use std::path::Path;
use tauri::{AppHandle, State};
fn members(app: &AppHandle) -> Result<Vec<LocalFolderSource>, CatalogError> {
    let (path, _) = crate::library::registry_path(app).map_err(|_| failure("unavailable"))?;
    let sources = LibraryStore::new(path)
        .read_query_snapshot()
        .map_err(|_| failure("unavailable"))?
        .local_sources;
    if sources.len() > 128 {
        return Err(failure("too_large"));
    }
    Ok(sources)
}
fn locate(
    backend: &ReaderBackend,
    member: &LocalFolderSource,
) -> Result<Option<mdreader_core::Source>, CatalogError> {
    let root = member.root().map_err(|_| failure("unauthorized"))?;
    root.verify().map_err(|_| failure("unauthorized"))?;
    backend
        .typed(Request::FindSource {
            path: root.path,
            identity: root.identity,
        })
        .map_err(|_| failure("unavailable"))
}
fn local_member(
    backend: &ReaderBackend,
    all: &[LocalFolderSource],
    source: &str,
) -> Result<LocalFolderSource, CatalogError> {
    for member in all {
        // One unavailable folder must not hide the remaining valid sources.
        if locate(backend, member)
            .ok()
            .flatten()
            .is_some_and(|s| s.id == source && s.enabled)
        {
            return Ok(member.clone());
        }
    }
    Err(failure("unauthorized"))
}
fn revalidate(
    app: &AppHandle,
    backend: &ReaderBackend,
    selected: Option<&LocalFolderSource>,
    source: Option<&str>,
) -> Result<(), CatalogError> {
    if let (Some(selected), Some(source)) = (selected, source) {
        if !members(app)?.contains(selected)
            || local_member(backend, std::slice::from_ref(selected), source).is_err()
        {
            return Err(failure("stale"));
        }
    }
    Ok(())
}
fn execute(
    app: &AppHandle,
    backend: &ReaderBackend,
    request: CatalogRequest,
) -> Result<CatalogReply, CatalogError> {
    let selected_source = match &request {
        CatalogRequest::Page { catalog_id, .. } => {
            catalog_id.strip_prefix("local:").map(str::to_owned)
        }
        CatalogRequest::Read { source_id, .. } if !source_id.starts_with("provider:") => {
            Some(source_id.clone())
        }
        _ => None,
    };
    let listing = matches!(&request, CatalogRequest::List);
    let (all, local_failure) = if listing || selected_source.is_some() {
        match members(app) {
            Ok(all) => (all, None),
            Err(error) if listing => (vec![], Some(error.kind)),
            Err(error) => return Err(error),
        }
    } else {
        (vec![], None)
    };
    let selected = selected_source
        .as_deref()
        .map(|id| local_member(backend, &all, id))
        .transpose()?;
    if let CatalogRequest::Read {
        source_id,
        path,
        revision,
    } = &request
    {
        if path.is_empty()
            || path.len() > 4096
            || path.chars().any(char::is_control)
            || Path::new(path)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || revision.len() != 64
            || !revision.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(failure("invalid"));
        }
        let document: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source_id.clone(),
                path: path.clone(),
                revision: Some(revision.clone()),
            })
            .map_err(|_| failure("stale"))?;
        if document.source_id != *source_id
            || document.path != *path
            || document.revision != *revision
        {
            return Err(failure("stale"));
        }
        revalidate(app, backend, selected.as_ref(), selected_source.as_deref())?;
        let line_end = u32::try_from(document.markdown.lines().count().max(1))
            .map_err(|_| failure("too_large"))?;
        return Ok(CatalogReply::ReadDocument {
            source_id: document.source_id,
            path: document.path,
            revision: document.revision,
            markdown: document.markdown,
            line_start: 1,
            line_end,
        });
    }
    let mut inactive = Vec::new();
    let mut mapped = Vec::new();
    let service_request = match request {
        CatalogRequest::List => {
            for member in &all {
                match locate(backend, member) {
                    Ok(Some(source)) => mapped.push((source.id, member.id.clone())),
                    value => inactive.push(CatalogSource {
                        catalog_id: format!("inactive:{}", member.id),
                        name: member
                            .path()
                            .ok()
                            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                            .unwrap_or_else(|| "Unavailable folder".into()),
                        kind: "local".into(),
                        enabled: false,
                        coverage: if matches!(&value, Ok(None)) {
                            "needs_activation"
                        } else {
                            "unavailable"
                        }
                        .into(),
                        observed_at_ms: None,
                        failure: value.err().map(|e| e.kind),
                        library_source_id: Some(member.id.clone()),
                    }),
                }
            }
            service::CatalogRequest::List {
                local_sources: mapped.iter().map(|(id, _)| id.clone()).collect(),
            }
        }
        CatalogRequest::Page {
            catalog_id,
            cursor,
            limit,
        } => service::CatalogRequest::Page {
            catalog_id,
            cursor,
            limit,
        },
        CatalogRequest::Close { cursor } => service::CatalogRequest::Close { cursor },
        CatalogRequest::Read { .. } => return Err(failure("invalid")),
    };
    #[derive(serde::Deserialize)]
    struct Envelope {
        result: Option<service::CatalogReply>,
        error: Option<service::CatalogError>,
    }
    let response: Envelope = backend
        .typed(Request::Catalog {
            request: service_request,
        })
        .map_err(|_| failure("unavailable"))?;
    if let Some(error) = response.error {
        return Err(failure(&error_kind(error)));
    }
    revalidate(app, backend, selected.as_ref(), selected_source.as_deref())?;
    if listing && local_failure.is_none() && members(app)? != all {
        return Err(failure("stale"));
    }
    Ok(
        match response.result.ok_or_else(|| failure("persistence"))? {
            service::CatalogReply::List {
                sources,
                provider_failure,
            } => {
                let mut sources: Vec<CatalogSource> = sources.into_iter().map(Into::into).collect();
                for source in &mut sources {
                    source.library_source_id = source
                        .catalog_id
                        .strip_prefix("local:")
                        .and_then(|id| mapped.iter().find(|(core, _)| core == id))
                        .map(|(_, library)| library.clone());
                }
                sources.extend(inactive);
                CatalogReply::List {
                    sources,
                    provider_failure: provider_failure.map(error_kind),
                    local_failure,
                }
            }
            service::CatalogReply::Page {
                source,
                documents,
                next_cursor,
            } => CatalogReply::Page {
                source: source.into(),
                documents: documents.into_iter().map(Into::into).collect(),
                next_cursor,
            },
            service::CatalogReply::Complete => CatalogReply::Complete,
        },
    )
}
#[tauri::command]
#[specta::specta]
pub async fn reader_catalog(
    app: AppHandle,
    reader: State<'_, ReaderBackend>,
    request: CatalogRequest,
) -> Result<CatalogReply, CatalogError> {
    let backend = reader.inner().clone();
    tauri::async_runtime::spawn_blocking(move || execute(&app, &backend, request))
        .await
        .map_err(|_| failure("unavailable"))?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_library_membership_does_not_admit_regrant_or_follow_a_replaced_folder() {
        let t = tempfile::tempdir().unwrap();
        let notes = t.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut registry = crate::library::LibraryRegistry::empty();
        let member = registry.add_root(&notes).unwrap();
        let backend = ReaderBackend::embedded(&t.path().join("profile")).unwrap();
        assert!(locate(&backend, &member).unwrap().is_none());
        let before: Vec<mdreader_core::Source> = backend.typed(Request::ListSources).unwrap();
        assert!(before.is_empty());
        let id = backend.admit(&member.root().unwrap()).unwrap();
        assert!(local_member(&backend, std::slice::from_ref(&member), &id).is_ok());
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id: id.clone(),
                enabled: false,
            })
            .unwrap();
        assert!(local_member(&backend, std::slice::from_ref(&member), &id).is_err());
        assert!(!locate(&backend, &member).unwrap().unwrap().enabled);
        std::fs::rename(&notes, t.path().join("original-notes")).unwrap();
        std::fs::create_dir(&notes).unwrap();
        assert!(locate(&backend, &member).is_err());
        let after: Vec<mdreader_core::Source> = backend.typed(Request::ListSources).unwrap();
        assert_eq!(after.len(), 1);
        assert!(!after[0].enabled);
    }
}
