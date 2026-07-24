//! Turning a written link target into a document in the corpus.
//!
//! Resolution is deliberately **conservative and total-or-nothing**: a target
//! that matches zero documents, or more than one, resolves to `None`. An
//! ambiguous link is not evidence of anything, and guessing which of two
//! candidates the author meant is exactly the kind of intent-invention this
//! module refuses to do.

use super::SourceDocument;

/// A link target split into its document part and its optional `#fragment`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget<'a> {
    /// The part before `#` (may be empty for a same-document `#anchor`).
    pub document: &'a str,
    /// The part after the first `#`, if any.
    pub fragment: Option<&'a str>,
}

/// Split `raw_target` at its first `#`.
pub fn split_fragment(raw_target: &str) -> ResolvedTarget<'_> {
    match raw_target.split_once('#') {
        Some((document, fragment)) => ResolvedTarget {
            document: document.trim(),
            fragment: Some(fragment.trim()).filter(|f| !f.is_empty()),
        },
        None => ResolvedTarget {
            document: raw_target.trim(),
            fragment: None,
        },
    }
}

/// Normalize a heading or name for comparison: lowercased, with runs of
/// whitespace, `-` and `_` collapsed to single spaces.
///
/// This is what makes `## Search API`, `[[…#search-api]]` and `search_api`
/// the same interface without any fuzzy matching.
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.extend(ch.to_lowercase());
        }
    }
    out
}

/// Resolve a link's document part against the corpus, returning the index of
/// the matching document.
///
/// Three forms are understood, tried in order, and each must match exactly
/// one document:
///
/// 1. a path relative to `from`'s directory (`../notes/x.md`, `./x`),
/// 2. a repo-relative path (`docs/notes/x.md`),
/// 3. a wikilink page name matched against file stems, then titles.
///
/// External targets (anything with a `:` scheme, e.g. `https://…`) never
/// resolve.
pub fn resolve_document(corpus: &[SourceDocument], from: &str, target: &str) -> Option<usize> {
    let target = target.trim();
    if target.is_empty() || target.contains("://") {
        return None;
    }
    let with_md = |path: String| {
        if path.ends_with(".md") {
            path
        } else {
            format!("{path}.md")
        }
    };

    // (1) and (2): path-shaped targets. A base that `..` walks off the top of
    // simply yields no candidate — an escaping target is not resolvable, but
    // it must not abort the wikilink attempt for well-formed siblings.
    let dir = from.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut candidates: Vec<String> = Vec::new();
    for base in [dir, ""] {
        let Some(path) = normalize_path(base, target).map(with_md) else {
            continue;
        };
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }
    for candidate in &candidates {
        if let Some(index) = unique(corpus, |doc| doc.path == *candidate) {
            return Some(index);
        }
    }

    // (3): wikilink page name — file stem first, then title. A target with a
    // slash is a path and is never re-read as a page name.
    if target.contains('/') {
        return None;
    }
    let wanted = slug(target.strip_suffix(".md").unwrap_or(target));
    unique(corpus, |doc| slug(doc.stem()) == wanted)
        .or_else(|| unique(corpus, |doc| slug(&doc.title) == wanted))
}

/// The index of the single document satisfying `predicate`, or `None` when
/// zero or several do.
fn unique(corpus: &[SourceDocument], predicate: impl Fn(&SourceDocument) -> bool) -> Option<usize> {
    let mut found = None;
    for (index, doc) in corpus.iter().enumerate() {
        if predicate(doc) {
            if found.is_some() {
                return None; // ambiguous
            }
            found = Some(index);
        }
    }
    found
}

/// Join `target` onto `base` and resolve `.`/`..`, rejecting any traversal
/// that escapes the repo root.
fn normalize_path(base: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    if target.starts_with('/') {
        parts.clear();
    }
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::SourceSection;

    fn doc(path: &str, title: &str) -> SourceDocument {
        SourceDocument {
            path: path.into(),
            title: title.into(),
            content_hash: "blake3:x".into(),
            line_count: 5,
            frontmatter: Vec::new(),
            sections: vec![SourceSection {
                heading_path: vec![title.into()],
                depth: 1,
                start_line: 1,
                end_line: 5,
                own_end_line: 5,
                parent: None,
                content_hash: "blake3:s".into(),
                links: Vec::new(),
            }],
        }
    }

    fn corpus() -> Vec<SourceDocument> {
        vec![
            doc("docs/adr/0007-storage.md", "Storage engine"),
            doc("docs/adr/0009-search.md", "Search"),
            doc("docs/arch/catdb-core.md", "catdb-core"),
        ]
    }

    #[test]
    fn splits_the_fragment_off_a_target() {
        assert_eq!(
            split_fragment("catdb-core#Search API"),
            ResolvedTarget {
                document: "catdb-core",
                fragment: Some("Search API")
            }
        );
        assert_eq!(
            split_fragment("catdb-core"),
            ResolvedTarget {
                document: "catdb-core",
                fragment: None
            }
        );
        // A trailing `#` carries no fragment.
        assert_eq!(split_fragment("x#").fragment, None);
    }

    #[test]
    fn slug_collapses_case_dashes_and_underscores() {
        assert_eq!(slug("Search  API"), "search api");
        assert_eq!(slug("search-api"), "search api");
        assert_eq!(slug("Search_API"), "search api");
    }

    #[test]
    fn resolves_relative_and_repo_relative_paths() {
        let corpus = corpus();
        let from = "docs/adr/0007-storage.md";
        assert_eq!(resolve_document(&corpus, from, "0009-search.md"), Some(1));
        assert_eq!(
            resolve_document(&corpus, from, "../arch/catdb-core"),
            Some(2)
        );
        assert_eq!(
            resolve_document(&corpus, from, "docs/arch/catdb-core.md"),
            Some(2)
        );
    }

    #[test]
    fn resolves_wikilinks_by_stem_then_title() {
        let corpus = corpus();
        let from = "docs/adr/0007-storage.md";
        assert_eq!(resolve_document(&corpus, from, "0009-search"), Some(1));
        assert_eq!(resolve_document(&corpus, from, "Storage engine"), Some(0));
    }

    #[test]
    fn refuses_external_unknown_and_escaping_targets() {
        let corpus = corpus();
        let from = "docs/adr/0007-storage.md";
        assert_eq!(resolve_document(&corpus, from, "https://example.com"), None);
        assert_eq!(resolve_document(&corpus, from, "no-such-doc"), None);
        assert_eq!(resolve_document(&corpus, from, ""), None);
        assert_eq!(resolve_document(&corpus, from, "../../../etc/passwd"), None);
    }

    #[test]
    fn a_relative_path_match_beats_the_page_name_search() {
        // `0009-search` sits next to the source document, so it resolves as a
        // relative path before any stem search happens — a same-stem file
        // elsewhere in the corpus is not ambiguity.
        let mut corpus = corpus();
        corpus.push(doc("docs/other/0009-search.md", "Search again"));
        let from = "docs/adr/0007-storage.md";
        assert_eq!(resolve_document(&corpus, from, "0009-search"), Some(1));
    }

    #[test]
    fn an_ambiguous_page_name_resolves_to_nothing() {
        let mut corpus = corpus();
        corpus.push(doc("docs/x/shared.md", "Shared one"));
        corpus.push(doc("docs/y/shared.md", "Shared two"));
        let from = "docs/adr/0007-storage.md";
        // No `docs/adr/shared.md` and no `shared.md` at the root, so the page
        // name search runs — and finds two. Two is not an answer.
        assert_eq!(resolve_document(&corpus, from, "shared"), None);
    }
}
