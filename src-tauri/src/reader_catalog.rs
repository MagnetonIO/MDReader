//! Native membership adapter. Catalog reads never admit or regrant a folder.
mod asset;
mod edit;
mod image;
mod model;
mod resolve;
use crate::{
    library::{LibraryStore, LocalFolderSource},
    reader::ReaderBackend,
};
use mdreader_service::{catalog as service, Request};
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
/// Retain the existing native Library grant across another cached-only reader operation.
/// A device source has no membership here and cannot acquire comparison authority.
pub(crate) struct LocalMembership {
    source_id: String,
    member: LocalFolderSource,
}
impl LocalMembership {
    pub(crate) fn capture(
        app: &AppHandle,
        backend: &ReaderBackend,
        source_id: &str,
    ) -> Result<Self, CatalogError> {
        Self::from_members(backend, &members(app)?, source_id)
    }
    fn from_members(
        backend: &ReaderBackend,
        all: &[LocalFolderSource],
        source_id: &str,
    ) -> Result<Self, CatalogError> {
        Ok(Self {
            source_id: source_id.to_owned(),
            member: local_member(backend, all, source_id)?,
        })
    }
    pub(crate) fn revalidate(
        &self,
        app: &AppHandle,
        backend: &ReaderBackend,
    ) -> Result<(), CatalogError> {
        self.verify(backend, &members(app)?)
    }
    fn verify(
        &self,
        backend: &ReaderBackend,
        all: &[LocalFolderSource],
    ) -> Result<(), CatalogError> {
        if !all.contains(&self.member)
            || local_member(backend, std::slice::from_ref(&self.member), &self.source_id).is_err()
        {
            return Err(failure("stale"));
        }
        Ok(())
    }
}
pub(crate) fn execute(
    app: &AppHandle,
    backend: &ReaderBackend,
    request: CatalogRequest,
) -> Result<CatalogReply, CatalogError> {
    if let CatalogRequest::Attachment {
        source_id,
        path,
        revision,
        target,
    } = &request
    {
        let preview = asset::local(
            app,
            backend,
            source_id,
            path,
            revision,
            target,
            crate::reader_assets::AssetReferenceKind::Attachment,
        )?;
        return Ok(CatalogReply::Attachment {
            source_id: source_id.clone(),
            path: path.clone(),
            revision: revision.clone(),
            target: target.clone(),
            preview,
        });
    }
    if let CatalogRequest::Image {
        source_id,
        path,
        revision,
        target,
    } = &request
    {
        return image::local(app, backend, source_id, path, revision, target);
    }
    let capture = match &request {
        CatalogRequest::Read { source_id, .. }
        | CatalogRequest::Resolve { source_id, .. }
        | CatalogRequest::EditLocal { source_id, .. }
            if !source_id.starts_with("provider:") =>
        {
            crate::reader_owned::capture_selected(backend, source_id)
                .unwrap_or(false)
                .then(|| source_id.clone())
        }
        _ => None,
    };
    if capture.is_some() && matches!(&request, CatalogRequest::EditLocal { .. }) {
        return Err(failure("unsupported"));
    }
    let selected_source = match &request {
        CatalogRequest::Page { catalog_id, .. } => {
            catalog_id.strip_prefix("local:").map(str::to_owned)
        }
        CatalogRequest::Read { source_id, .. }
        | CatalogRequest::ReadCurrentLocal { source_id, .. }
        | CatalogRequest::EditLocal { source_id, .. }
        | CatalogRequest::Resolve { source_id, .. }
            if !source_id.starts_with("provider:") && capture.is_none() =>
        {
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
    if let CatalogRequest::ReadCurrentLocal { source_id, path } = &request {
        if path.is_empty()
            || path.len() > 4096
            || path.chars().any(char::is_control)
            || Path::new(path)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || !mdreader_core::scan::is_markdown(Path::new(path), false)
        {
            return Err(failure("invalid"));
        }
        let member = selected.as_ref().ok_or_else(|| failure("unauthorized"))?;
        revalidate(app, backend, Some(member), Some(source_id))?;
        let document: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source_id.clone(),
                path: path.clone(),
                revision: None,
            })
            .map_err(|_| failure("stale"))?;
        if document.source_id != *source_id || document.path != *path {
            return Err(failure("stale"));
        }
        revalidate(app, backend, Some(member), Some(source_id))?;
        return Ok(CatalogReply::ReadDocument {
            source_kind: "local".into(),
            source_id: document.source_id,
            path: document.path,
            revision: document.revision,
            line_start: 1,
            line_end: document
                .markdown
                .lines()
                .count()
                .max(1)
                .try_into()
                .map_err(|_| failure("too_large"))?,
            markdown: document.markdown,
        });
    }
    if let CatalogRequest::EditLocal {
        source_id,
        path,
        revision,
    } = &request
    {
        return edit::local(
            app,
            backend,
            selected.as_ref().ok_or_else(|| failure("unauthorized"))?,
            source_id,
            path,
            revision,
        );
    }
    if let CatalogRequest::Resolve {
        source_id,
        path,
        revision,
        kind,
        target,
    } = &request
    {
        let root = selected
            .as_ref()
            .map(|member| {
                member
                    .root()
                    .map_err(|_| failure("unauthorized"))
                    .and_then(|root| {
                        backend
                            .protect_local_root(root)
                            .map_err(|_| failure("unauthorized"))
                    })
            })
            .transpose()?;
        let references = resolve::local(
            backend,
            source_id,
            path,
            revision,
            *kind,
            target,
            root.as_ref(),
        )?;
        revalidate(app, backend, selected.as_ref(), selected_source.as_deref())?;
        if let Some(id) = &capture {
            if !crate::reader_owned::capture_selected(backend, id).map_err(|_| failure("stale"))? {
                return Err(failure("stale"));
            }
        }
        return Ok(CatalogReply::Resolved { references });
    }
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
        if let Some(id) = &capture {
            if !crate::reader_owned::capture_selected(backend, id).map_err(|_| failure("stale"))? {
                return Err(failure("stale"));
            }
        }
        let line_end = u32::try_from(document.markdown.lines().count().max(1))
            .map_err(|_| failure("too_large"))?;
        return Ok(CatalogReply::ReadDocument {
            source_kind: if capture.is_some() {
                "capture"
            } else if selected.is_some() {
                "local"
            } else {
                "provider"
            }
            .into(),
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
        CatalogRequest::Read { .. }
        | CatalogRequest::ReadCurrentLocal { .. }
        | CatalogRequest::Resolve { .. }
        | CatalogRequest::Image { .. }
        | CatalogRequest::Attachment { .. }
        | CatalogRequest::EditLocal { .. } => {
            return Err(failure("invalid"));
        }
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
    fn retained_library_membership_does_not_admit_or_regrant_and_matches_a_replaced_folder_by_path()
    {
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
        // Path-based: the folder now at this path is the same, still revoked source.
        let replaced = locate(&backend, &member).unwrap().unwrap();
        assert_eq!(replaced.id, id);
        assert!(!replaced.enabled);
        let after: Vec<mdreader_core::Source> = backend.typed(Request::ListSources).unwrap();
        assert_eq!(after.len(), 1);
        assert!(!after[0].enabled);
    }
    #[test]
    fn canonical_review_membership_checks_exact_read_and_final_retained_grant_without_workspace() {
        let t = tempfile::tempdir().unwrap();
        let notes = t.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        std::fs::write(notes.join("note.md"), "# Exact canonical revision").unwrap();
        let mut registry = crate::library::LibraryRegistry::empty();
        let member = registry.add_root(&notes).unwrap();
        let backend = ReaderBackend::embedded(&t.path().join("profile")).unwrap();
        let members = std::slice::from_ref(&member);
        assert!(LocalMembership::from_members(&backend, members, "unknown").is_err());
        let source_id = backend.admit(&member.root().unwrap()).unwrap();
        let guard = LocalMembership::from_members(&backend, members, &source_id).unwrap();
        let observed: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source_id.clone(),
                path: "note.md".into(),
                revision: None,
            })
            .unwrap();
        let exact: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source_id.clone(),
                path: observed.path.clone(),
                revision: Some(observed.revision.clone()),
            })
            .unwrap();
        assert_eq!(exact.revision, observed.revision);
        guard.verify(&backend, members).unwrap();
        // Missing Library membership, including service-only device sources, is no grant.
        assert!(guard.verify(&backend, &[]).is_err());
        assert!(LocalMembership::from_members(&backend, &[], &source_id).is_err());
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id: source_id.clone(),
                enabled: false,
            })
            .unwrap();
        assert!(guard.verify(&backend, members).is_err());
        assert!(LocalMembership::from_members(&backend, members, &source_id).is_err());
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id,
                enabled: true,
            })
            .unwrap();
        std::fs::rename(&notes, t.path().join("original-notes")).unwrap();
        std::fs::create_dir(&notes).unwrap();
        // Path-based membership: the folder at the member's path still matches.
        guard.verify(&backend, members).unwrap();
    }
}
