//! Only the existing retained local membership can authorize this resolver.
use super::{failure, CatalogError, CatalogLinkKind, CatalogReference};
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
    kind: CatalogLinkKind,
    target: &str,
    root: Option<&mdreader_core::SourceRoot>,
) -> Result<Vec<CatalogReference>, CatalogError> {
    if source.len() != 64
        || !source.bytes().all(|b| b.is_ascii_hexdigit())
        || revision.len() != 64
        || !revision.bytes().all(|b| b.is_ascii_hexdigit())
        || target.len() > 4096
        || target.chars().any(char::is_control)
        || target.starts_with('/')
        || target
            .split('#')
            .next()
            .is_some_and(|path| path.contains([':', '\\']))
    {
        return Err(failure("invalid"));
    }
    origin(backend, source, path, revision)?;
    if let Some(root) = root.filter(|_| matches!(kind, CatalogLinkKind::Relative)) {
        let parsed = mdreader_core::RelativeMarkdownTarget::parse(path, target)
            .map_err(|_| failure("unsupported"))?;
        root.verify().map_err(|_| failure("unauthorized"))?;
        let mut references = Vec::new();
        for candidate in parsed.paths {
            if root
                .open_optional_file_relative(std::path::Path::new(&candidate))
                .map_err(|_| failure("unauthorized"))?
                .is_none()
            {
                continue;
            }
            // The optional handle only distinguishes absent from refused. It is
            // never a receipt: the owner reads actual bytes through its existing
            // enabled root and returns the exact observed revision.
            let document: mdreader_core::Document = backend
                .typed(Request::Read {
                    source_id: source.into(),
                    path: candidate.clone(),
                    revision: None,
                })
                .map_err(|_| failure("unavailable"))?;
            if document.source_id != source || document.path != candidate {
                return Err(failure("stale"));
            }
            references.push(CatalogReference {
                source_id: document.source_id,
                path: document.path,
                revision: document.revision,
                // A line is only a hint. Missing/duplicate/raw/footnote fragments
                // do not erase the document; the renderer owns its final IDs.
                line: heading_hint(&document.markdown, parsed.heading.as_deref()),
            });
        }
        origin(backend, source, path, revision)?;
        root.verify().map_err(|_| failure("unauthorized"))?;
        return Ok(references);
    }
    let (name, heading) = target
        .split_once('#')
        .map(|(n, h)| (n, Some(h)))
        .unwrap_or((target, None));
    let heading = heading
        .map(mdreader_core::decode_markdown_fragment)
        .transpose()
        .map_err(|_| failure("invalid"))?
        .filter(|h| !h.is_empty());
    let mut references: Vec<mdreader_core::Reference> = backend
        .typed(Request::Resolve {
            source_id: source.into(),
            from: path.into(),
            // The index selects exact/alias candidates only. Approximate SQL
            // slugs cannot decide whether a renderer footnote/raw ID exists.
            target: name.into(),
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
    for reference in &mut references {
        let document: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source.into(),
                path: reference.path.clone(),
                revision: Some(reference.revision.clone()),
            })
            .map_err(|_| failure("stale"))?;
        if document.source_id != source
            || document.path != reference.path
            || document.revision != reference.revision
        {
            return Err(failure("stale"));
        }
        reference.line = heading_hint(&document.markdown, heading.as_deref());
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

fn heading_hint(markdown: &str, fragment: Option<&str>) -> u32 {
    let Some(fragment) = fragment else {
        return 1;
    };
    mdreader_core::parse_markdown(markdown, "")
        .sections
        .into_iter()
        .find(|section| {
            section.depth > 0
                && section.heading_path.last().is_some_and(|text| {
                    let slug: String = text
                        .to_lowercase()
                        .chars()
                        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' '))
                        .map(|c| if c == ' ' { '-' } else { c })
                        .collect();
                    text.eq_ignore_ascii_case(fragment) || slug == fragment
                })
        })
        .map(|section| section.start_line)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_wikilink_documents_survive_renderer_only_and_missing_fragments() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("nested")).unwrap();
        std::fs::write(notes.join("origin.md"), "# Origin").unwrap();
        std::fs::write(notes.join("note.md"), "# Notebook\n\n<div id=\"Raw:ID\">Raw anchor</div>\n\nFootnote[^1].\n\n[^1]: Original body.").unwrap();
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
        let resolve = |target: &str| {
            super::local(
                &backend,
                &source,
                &from.path,
                &from.revision,
                CatalogLinkKind::Wikilink,
                target,
                Some(&root),
            )
        };
        for target in [
            "Notebook#fn%3A1",
            "Notebook#Raw%3AID",
            "note.md#missing-fragment",
            "Notebook#not-a-heading",
        ] {
            let matched = resolve(target).unwrap();
            assert_eq!(matched.len(), 1, "document erased for {target}");
            assert_eq!(matched[0].path, "note.md");
            assert_eq!(matched[0].line, 1);
            let exact: mdreader_core::Document = backend
                .typed(Request::Read {
                    source_id: source.clone(),
                    path: matched[0].path.clone(),
                    revision: Some(matched[0].revision.clone()),
                })
                .unwrap();
            assert!(exact.markdown.contains("Original body."));
        }
        assert!(resolve("Notebook#invalid%0a").is_err());
        assert!(resolve("Notebook#invalid%ZZ").is_err());
        std::fs::write(
            notes.join("nested/another.md"),
            "# Notebook\n\nSecond exact alias.",
        )
        .unwrap();
        let _: serde_json::Value = backend
            .typed(Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        assert_eq!(resolve("Notebook#fn%3A1").unwrap().len(), 2);
        let matched = resolve("note.md#missing").unwrap();
        assert_eq!(matched.len(), 1); // Authored exact filename still wins.
        std::fs::write(notes.join("note.md"), "# Changed after index").unwrap();
        assert!(resolve("note.md#fn%3A1").is_err());
    }
    #[test]
    fn unindexed_relative_markdown_keeps_exact_bytes_grant_and_fragment_target() {
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("docs/nested")).unwrap();
        std::fs::write(
            notes.join("docs/nested/origin.md"),
            "[Guide](../Guide%20%CF%80.markdown#fn%3A1)",
        )
        .unwrap();
        std::fs::write(
            notes.join("docs/Guide π.markdown"),
            "# Guide\n\nFootnote[^1].\n\n[^1]: Exact original.",
        )
        .unwrap();
        std::fs::write(notes.join("docs/image.png"), [0, 1, 2]).unwrap();
        let backend = ReaderBackend::embedded(&temp.path().join("profile")).unwrap();
        let root = mdreader_core::SourceRoot::capture(notes.canonicalize().unwrap()).unwrap();
        let source = backend.admit(&root).unwrap();
        let from: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source.clone(),
                path: "docs/nested/origin.md".into(),
                revision: None,
            })
            .unwrap();
        let status: mdreader_core::IndexStatus = backend
            .typed(Request::Status {
                sources: vec![source.clone()],
            })
            .unwrap();
        assert_eq!(status.documents, 0);
        let resolve = |target: &str| {
            super::local(
                &backend,
                &source,
                &from.path,
                &from.revision,
                CatalogLinkKind::Relative,
                target,
                Some(&root),
            )
        };
        let references = resolve("../Guide%20%CF%80.markdown#fn%3A1").unwrap();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].path, "docs/Guide π.markdown");
        assert_eq!(references[0].line, 1); // Renderer resolves footnote ID after read.
        let exact: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source.clone(),
                path: references[0].path.clone(),
                revision: Some(references[0].revision.clone()),
            })
            .unwrap();
        assert!(exact.markdown.contains("Exact original."));
        assert_eq!(
            resolve("../Guide%20%CF%80.markdown#not-a-heading")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            resolve("#not-a-heading").unwrap()[0].revision,
            from.revision
        );
        assert_eq!(resolve("../Guide%20%CF%80").unwrap().len(), 1);
        assert!(resolve("missing.md").unwrap().is_empty());
        for target in [
            "../../../outside.md",
            "%2foutside.md",
            "../image.png",
            "../missing.pdf",
            "../Guide%20%CF%80.markdown?q=1",
            "%43%3a/Guide.md",
            "bad%00.md",
        ] {
            assert!(resolve(target).is_err(), "accepted {target}");
        }
        // Observed receipts cannot silently read a later file revision.
        std::fs::write(notes.join("docs/Guide π.markdown"), "# Replaced Markdown").unwrap();
        assert!(backend
            .typed::<mdreader_core::Document>(Request::Read {
                source_id: source.clone(),
                path: references[0].path.clone(),
                revision: Some(references[0].revision.clone()),
            })
            .is_err());
        assert_ne!(
            resolve("../Guide%20%CF%80.markdown").unwrap()[0].revision,
            references[0].revision
        );
        let status: mdreader_core::IndexStatus = backend
            .typed(Request::Status {
                sources: vec![source.clone()],
            })
            .unwrap();
        assert_eq!(status.documents, 0); // Navigation never starts an index/write cycle.
        let sources: Vec<mdreader_core::Source> = backend.typed(Request::ListSources).unwrap();
        assert_eq!(sources.len(), 1);
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id: source.clone(),
                enabled: false,
            })
            .unwrap();
        assert!(resolve("../Guide%20%CF%80.markdown").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn relative_markdown_refuses_symlink_components_and_replaced_retained_root() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let notes = temp.path().join("notes");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&notes).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(notes.join("origin.md"), "# Origin").unwrap();
        std::fs::write(outside.join("target.md"), "# Outside grant").unwrap();
        symlink(&outside, notes.join("escape")).unwrap();
        symlink(outside.join("target.md"), notes.join("target.md")).unwrap();
        let backend = ReaderBackend::embedded(&temp.path().join("profile")).unwrap();
        let root = mdreader_core::SourceRoot::capture(notes.canonicalize().unwrap()).unwrap();
        let source = backend.admit(&root).unwrap();
        let from: mdreader_core::Document = backend
            .typed(Request::Read {
                source_id: source.clone(),
                path: "origin.md".into(),
                revision: None,
            })
            .unwrap();
        let resolve = |target: &str| {
            super::local(
                &backend,
                &source,
                &from.path,
                &from.revision,
                CatalogLinkKind::Relative,
                target,
                Some(&root),
            )
        };
        assert!(resolve("escape/target.md").is_err());
        assert!(resolve("target.md").is_err());
        std::fs::rename(&notes, temp.path().join("retained-original")).unwrap();
        std::fs::create_dir(&notes).unwrap();
        std::fs::write(notes.join("origin.md"), "# Origin").unwrap();
        std::fs::write(notes.join("target.md"), "# Replacement").unwrap();
        assert!(resolve("target.md").is_err());
    }
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
        let local =
            |backend: &ReaderBackend, source: &str, path: &str, revision: &str, target: &str| {
                super::local(
                    backend,
                    source,
                    path,
                    revision,
                    CatalogLinkKind::Relative,
                    target,
                    Some(&root),
                )
            };
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
            super::local(
                &backend,
                &source,
                "origin.md",
                &from.revision,
                CatalogLinkKind::Wikilink,
                "Shared title",
                Some(&root),
            )
            .unwrap()
            .len(),
            2
        );
        std::fs::write(notes.join("origin.md"), "# Changed").unwrap();
        assert!(local(&backend, &source, "origin.md", &from.revision, "next.md").is_err());
        assert!(local(
            &backend,
            &source,
            "origin.md",
            &from.revision,
            "../outside.md"
        )
        .is_err());
        let _: serde_json::Value = backend
            .typed(Request::SetSourceEnabled {
                source_id: source.clone(),
                enabled: false,
            })
            .unwrap();
        assert!(local(&backend, &source, "origin.md", &from.revision, "next.md").is_err());
    }
}
