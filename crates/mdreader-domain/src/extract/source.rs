//! The parsed-markdown view the extractors read.
//!
//! These types mirror the structural output of MDReader's parser
//! (`mdreader_ingest::parse::ParsedDocument` and friends) but are declared
//! here, in the domain crate, on purpose: `mdreader-ingest` depends on
//! `mdreader-domain`, so the dependency cannot run the other way, and this
//! crate is deliberately parser-free (see the crate docs). The ingest layer —
//! or any other producer, including a test fixture reader — converts its
//! parse output into this view.
//!
//! Everything here is read-only input. Extraction never writes to a file,
//! and it cannot: it is handed values, not paths.

use serde::{Deserialize, Serialize};

/// One parsed markdown document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceDocument {
    /// Repo-relative path, as recorded on the document row (the `source` of
    /// every fact extracted from it).
    pub path: String,
    /// The document title: its first level-1 heading, or the file stem.
    pub title: String,
    /// blake3 hash of the whole file at parse time.
    pub content_hash: String,
    /// Tolerantly parsed `key: value` frontmatter pairs, in source order.
    pub frontmatter: Vec<(String, String)>,
    /// Heading-delimited sections in document order.
    pub sections: Vec<SourceSection>,
}

impl SourceDocument {
    /// The first frontmatter value under any of `keys` (case-insensitive on
    /// the key), trimmed. Returns `None` for an absent or blank value, so a
    /// `type:` with nothing after it never triggers a rule.
    pub fn frontmatter_any(&self, keys: &[&str]) -> Option<&str> {
        self.frontmatter.iter().find_map(|(key, value)| {
            let matches = keys.iter().any(|k| k.eq_ignore_ascii_case(key.trim()));
            let value = value.trim();
            (matches && !value.is_empty()).then_some(value)
        })
    }

    /// The document's full line span: `(1, last section's end line)`. Used as
    /// the provenance range for document-level facts, whose evidence is the
    /// frontmatter plus the title rather than any single section.
    pub fn span(&self) -> (u32, u32) {
        (
            1,
            self.sections.iter().map(|s| s.end_line).max().unwrap_or(1),
        )
    }

    /// The file stem (basename without the `.md` extension), used for
    /// wikilink resolution.
    pub fn stem(&self) -> &str {
        let base = self.path.rsplit('/').next().unwrap_or(&self.path);
        base.strip_suffix(".md").unwrap_or(base)
    }
}

/// One heading-delimited section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSection {
    /// Heading breadcrumb from the document root down to this section's own
    /// heading. Empty for a preamble.
    pub heading_path: Vec<String>,
    /// Heading level (1–6); 0 for a preamble.
    pub depth: u8,
    /// First source line of the section span (1-based, inclusive).
    pub start_line: u32,
    /// Last source line of the span, subsections included (1-based,
    /// inclusive).
    pub end_line: u32,
    /// Index (into [`SourceDocument::sections`]) of the parent section.
    pub parent: Option<usize>,
    /// blake3 hash of the section's own content.
    pub content_hash: String,
    /// Links written in the section's **own** content — those in a
    /// subsection belong to that subsection, not to this one. The rules rely
    /// on it: a link under `## Supersedes` is a superseding claim, and a link
    /// under a `### Details` nested inside it is not.
    pub links: Vec<SourceLink>,
}

impl SourceSection {
    /// This section's own heading (the last breadcrumb element); `""` for a
    /// preamble.
    pub fn heading(&self) -> &str {
        self.heading_path.last().map_or("", String::as_str)
    }

    /// The section's line span as a provenance range.
    pub fn span(&self) -> (u32, u32) {
        (self.start_line, self.end_line)
    }
}

/// A link discovered in a section — *syntactic evidence*, never a typed
/// assertion on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceLink {
    /// The raw target exactly as written, fragment included.
    pub raw_target: String,
    /// `markdown`, `wikilink`, or `image`.
    pub kind: String,
    /// Source line the link starts on.
    pub line: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> SourceDocument {
        SourceDocument {
            path: "docs/adr/0007-storage.md".into(),
            title: "Storage".into(),
            content_hash: "blake3:doc".into(),
            frontmatter: vec![
                ("Type".into(), " decision ".into()),
                ("layer".into(), "  ".into()),
            ],
            sections: vec![SourceSection {
                heading_path: vec!["Storage".into()],
                depth: 1,
                start_line: 4,
                end_line: 20,
                parent: None,
                content_hash: "blake3:s".into(),
                links: Vec::new(),
            }],
        }
    }

    #[test]
    fn frontmatter_lookup_is_key_insensitive_and_value_trimmed() {
        assert_eq!(
            document().frontmatter_any(&["type", "kind"]),
            Some("decision")
        );
    }

    #[test]
    fn a_blank_frontmatter_value_is_absent() {
        assert_eq!(document().frontmatter_any(&["layer"]), None);
    }

    #[test]
    fn document_span_runs_to_the_last_section() {
        assert_eq!(document().span(), (1, 20));
    }

    #[test]
    fn stem_drops_directories_and_extension() {
        assert_eq!(document().stem(), "0007-storage");
    }
}
