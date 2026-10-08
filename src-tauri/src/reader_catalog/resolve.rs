//! Only the existing retained local membership can authorize this resolver.
use super::{CatalogError, CatalogReference, failure};
use crate::reader::ReaderBackend;
use mdreader_service::Request;

fn origin(
    backend: &ReaderBackend,
    source: &str,
    path: &str,
    revision: &str,
) -> Result<(), CatalogError> {
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
    Ok(())
}

pub(super) fn local(
    backend: &ReaderBackend,
    source: &str,
    path: &str,
    revision: &str,
    target: &str,
) -> Result<Vec<CatalogReference>, CatalogError> {
    if source.len() != 64
        || !source.bytes().all(|b| b.is_ascii_hexdigit())
        || revision.len() != 64
        || !revision.bytes().all(|b| b.is_ascii_hexdigit())
        || target.len() > 4096
        || target.chars().any(char::is_control)
        || target.starts_with('/')
        || target.contains(':')
        || target.contains('\\')
    {
        return Err(failure("invalid"));
    }
    origin(backend, source, path, revision)?;
    let references: Vec<mdreader_core::Reference> = backend
        .typed(Request::Resolve {
            source_id: source.into(),
            from: path.into(),
            target: target.into(),
        })
        .map_err(|_| failure("unavailable"))?;
    if references.len() > 16 {
        return Err(failure("too_large"));
    }
    if references
        .iter()
        .any(|reference| reference.source_id != source || reference.line == 0)
    {
        return Err(failure("stale"));
    }
    origin(backend, source, path, revision)?;
    Ok(references
        .into_iter()
        .map(|reference| CatalogReference {
            source_id: reference.source_id,
            path: reference.path,
            revision: reference.revision,
            line: reference.line,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_links_keep_exact_revisions_heading_lines_and_ambiguity_without_regrant() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("nested")).unwrap();
        std::fs::write(
            notes.join("origin.md"),
            "# Origin\n\n[Next](next.md#evidence)",
        )
        .unwrap();
        std::fs::write(notes.join("next.md"), "# Next\n\n## Evidence\n\nOriginal.").unwrap();
        std::fs::write(notes.join("nested/next.md"), "# Other\n").unwrap();
        let backend = ReaderBackend::embedded(&temp.path().join("profile")).unwrap();
        let root = mdreader_core::SourceRoot::capture(notes.canonicalize().unwrap()).unwrap();
        let source = backend.admit(&root).unwrap();
        let _: serde_json::Value = backend
            .typed(Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        let from: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source.clone(),
                path: "origin.md".into(),
                revision: None,
            })
            .unwrap();
        let exact = local(
            &backend,
            &source,
            "origin.md",
            &from.revision,
            "next.md#evidence",
        )
        .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].path, "next.md");
        assert_eq!(exact[0].line, 3);
        let doc: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source.clone(),
                path: exact[0].path.clone(),
                revision: Some(exact[0].revision.clone()),
            })
            .unwrap();
        assert!(doc.markdown.contains("Original."));
        assert!(
            local(&backend, &source, "origin.md", &from.revision, "missing.md")
                .unwrap()
                .is_empty()
        );
        // A flat stem with no exact relative path honestly returns both indexed matches.
        std::fs::write(notes.join("other.md"), "# Shared title").unwrap();
        std::fs::write(notes.join("nested/other.md"), "# Shared title").unwrap();
        let _: serde_json::Value = backend
            .typed(Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        assert_eq!(
            local(
                &backend,
                &source,
                "origin.md",
                &from.revision,
                "Shared title"
            )
            .unwrap()
            .len(),
            2
        );
        std::fs::write(notes.join("origin.md"), "# Changed").unwrap();
        assert!(local(&backend, &source, "origin.md", &from.revision, "next.md").is_err());
        assert!(
            local(
                &backend,
                &source,
                "origin.md",
                &from.revision,
                "../outside.md"
            )
            .is_err()
        );
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id: source.clone(),
                enabled: false,
            })
            .unwrap();
        assert!(local(&backend, &source, "origin.md", &from.revision, "next.md").is_err());
    }
}
