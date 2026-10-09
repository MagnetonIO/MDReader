//! Membership and exact-origin composition; shared assets own bounded IO/AST proof.
use super::{failure, CatalogError, LocalMembership};
use crate::{
    reader::ReaderBackend,
    reader_assets::{AssetObservation, AssetPreview, AssetReferenceKind, BoundedFile, Permit},
};
use mdreader_core::Document;
use mdreader_service::Request;
use tauri::AppHandle;

pub(super) fn exact(
    backend: &ReaderBackend,
    source: &str,
    path: &str,
    revision: &str,
) -> Result<Document, CatalogError> {
    let document: Document = backend
        .typed(Request::Read {
            source_id: source.into(),
            path: path.into(),
            revision: Some(revision.into()),
        })
        .map_err(|_| failure("stale"))?;
    if document.source_id != source || document.path != path || document.revision != revision {
        return Err(failure("stale"));
    }
    Ok(document)
}
fn origin(
    backend: &ReaderBackend,
    root: &mdreader_core::SourceRoot,
    source: &str,
    path: &str,
    revision: &str,
) -> Result<(Document, BoundedFile), CatalogError> {
    let document = exact(backend, source, path, revision)?;
    let observation = BoundedFile::read(root, path, mdreader_core::MAX_DOCUMENT_BYTES)
        .map_err(|error| failure(error.kind.as_str()))?;
    if observation.revision() != revision
        || observation.bytes.as_slice() != document.markdown.as_bytes()
    {
        return Err(failure("stale"));
    }
    Ok((document, observation))
}
pub(super) fn local(
    app: &AppHandle,
    backend: &ReaderBackend,
    source: &str,
    path: &str,
    revision: &str,
    target: &str,
    kind: AssetReferenceKind,
) -> Result<AssetPreview, CatalogError> {
    let map = |error: crate::reader_assets::AssetReadError| failure(error.kind.as_str());
    if source.len() != 64
        || revision.len() != 64
        || !source
            .bytes()
            .chain(revision.bytes())
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(failure("invalid"));
    }
    let _permit = Permit::acquire().map_err(map)?;
    if crate::reader_owned::capture_selected(backend, source).map_err(|_| failure("unavailable"))? {
        return Err(failure("unsupported"));
    }
    let membership = LocalMembership::capture(app, backend, source)?;
    membership.revalidate(app, backend)?;
    let root = membership
        .member
        .root()
        .map_err(|_| failure("unauthorized"))?;
    let (document, origin) = origin(backend, &root, source, path, revision)?;
    let asset =
        AssetObservation::read(&root, path, &document.markdown, target, kind).map_err(map)?;
    exact(backend, source, path, revision)?;
    membership.revalidate(app, backend)?;
    origin.revalidate(&root).map_err(map)?;
    asset.revalidate(&root).map_err(map)?;
    Ok(asset.preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_saved_ast_assets_keep_actual_unindexed_origin_and_owner_revocation() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        std::fs::write(notes.join("origin.md"), "[Compose](compose.platform.yml)").unwrap();
        std::fs::write(notes.join("compose.platform.yml"), "services:\n  api: {}\n").unwrap();
        let root = mdreader_core::SourceRoot::capture(notes.canonicalize().unwrap()).unwrap();
        let backend = ReaderBackend::embedded(&temp.path().join("profile")).unwrap();
        let source = backend.admit(&root).unwrap();
        let read: Document = backend
            .typed(Request::Read {
                source_id: source.clone(),
                path: "origin.md".into(),
                revision: None,
            })
            .unwrap();
        let (document, saved) =
            origin(&backend, &root, &source, &read.path, &read.revision).unwrap();
        let asset = AssetObservation::read(
            &root,
            &document.path,
            &document.markdown,
            "compose.platform.yml",
            AssetReferenceKind::Attachment,
        )
        .unwrap();
        saved.revalidate(&root).unwrap();
        asset.revalidate(&root).unwrap();
        let crate::reader_assets::AssetContent::Text { text, .. } = asset.preview.content else {
            panic!("expected inert text")
        };
        assert_eq!(text, "services:\n  api: {}\n");
        let status: mdreader_core::IndexStatus = backend
            .typed(Request::Status {
                sources: vec![source.clone()],
            })
            .unwrap();
        assert_eq!(status.documents, 0);
        std::fs::write(notes.join("origin.md"), "[Other](other.yml)").unwrap();
        assert!(origin(&backend, &root, &source, &read.path, &read.revision).is_err());
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id: source.clone(),
                enabled: false,
            })
            .unwrap();
        assert!(exact(&backend, &source, &read.path, &read.revision).is_err());
        assert_eq!(
            std::fs::read_to_string(notes.join("compose.platform.yml")).unwrap(),
            "services:\n  api: {}\n"
        );
    }
}
