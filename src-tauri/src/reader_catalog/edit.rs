//! Resolve an exact existing local grant for an explicit editor handoff.
//! This never activates a workspace, admits/regrants a folder or writes a file.
use super::*;

pub(super) fn local(
    app: &AppHandle,
    backend: &ReaderBackend,
    member: &LocalFolderSource,
    source: &str,
    path: &str,
    revision: &str,
) -> Result<CatalogReply, CatalogError> {
    if source.len() != 64
        || revision.len() != 64
        || !source
            .bytes()
            .chain(revision.bytes())
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(failure("invalid"));
    }
    revalidate(app, backend, Some(member), Some(source))?;
    let root = backend.protect_local_root(member.root().map_err(|_| failure("unauthorized"))?).map_err(|_|failure("unauthorized"))?;
    root.verify().map_err(|_| failure("unauthorized"))?;
    let document: mdreader_core::Document = backend
        .typed(Request::Read {
            source_id: source.into(),
            path: path.into(),
            revision: Some(revision.into()),
        })
        .map_err(|_| failure("stale"))?;
    if document.source_id != source || document.path != path || document.revision != revision {
        return Err(failure("stale"));
    }
    // Normal editor reads/saves retain their existing workspace generation/hash
    // checks. Frontend must settle its unsaved-buffer guard before activation,
    // then repeat this resolution and verify the ordinary read's exact hash.
    let absolute = root.path.join(path);
    let native_path = absolute.to_str().ok_or_else(|| failure("unsupported"))?;
    let native_root = root.path.to_str().ok_or_else(|| failure("unsupported"))?;
    if native_path.len() > 8192 || native_root.len() > 8192 {
        return Err(failure("too_large"));
    }
    root.verify().map_err(|_| failure("stale"))?;
    revalidate(app, backend, Some(member), Some(source))?;
    Ok(CatalogReply::EditLocal {
        source_id: source.into(),
        library_source_id: member.id.clone(),
        root: native_root.into(),
        path: native_path.into(),
        relative_path: path.into(),
        revision: revision.into(),
    })
}
