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
    /// blake3 hash of the document's **newline-normalized text** — that is,
    /// of exactly the lines [`Self::span`] names, joined by `\n`.
    ///
    /// Deliberately not the hash of the raw file bytes: a fact's recorded
    /// range and its recorded hash must describe the same thing, or the hash
    /// cannot be used to detect that the cited span drifted. (The ingest
    /// layer keeps its own raw-bytes hash for change detection; that is a
    /// different job.)
    pub content_hash: String,
    /// Total number of lines in the document.
    pub line_count: u32,
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

    /// The whole document, `(1, line_count)` — the span
    /// [`Self::content_hash`] covers.
    ///
    /// This is the provenance range for document-level facts, whose evidence
    /// is the frontmatter plus the title rather than any single section.
    /// Derived from `line_count` rather than from the last section's end so
    /// that it stays exact for a document with no sections at all.
    pub fn span(&self) -> (u32, u32) {
        (1, self.line_count.max(1))
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
    /// Last source line of the subtree span, subsections included (1-based,
    /// inclusive).
    pub end_line: u32,
    /// Last line of the section's **own** content — the line before its first
    /// subsection, or [`Self::end_line`] when it has none. This, not
    /// `end_line`, is what [`Self::content_hash`] covers.
    pub own_end_line: u32,
    /// Index (into [`SourceDocument::sections`]) of the parent section.
    pub parent: Option<usize>,
    /// blake3 hash of the section's own text: lines `start_line ..=
    /// own_end_line`, joined by `\n`. Exactly the span [`Self::span`] names.
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

    /// The span [`Self::content_hash`] covers: `(start_line, own_end_line)`.
    ///
    /// A fact cited to this section records this range, **not** the subtree
    /// range — a range that named lines the hash does not cover could never
    /// be verified against it.
    pub fn span(&self) -> (u32, u32) {
        (self.start_line, self.own_end_line.max(self.start_line))
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
    /// The full text of that line.
    ///
    /// Carried so that a fact cited to a *single line* can record the hash of
    /// that line. Without it the extractor would have to borrow the enclosing
    /// section's hash, and the recorded range and hash would describe
    /// different spans — silently defeating the drift detection the hash
    /// exists for.
    pub line_text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> SourceDocument {
        SourceDocument {
            path: "docs/adr/0007-storage.md".into(),
            title: "Storage".into(),
            content_hash: "blake3:doc".into(),
            line_count: 24,
            frontmatter: vec![
                ("Type".into(), " decision ".into()),
                ("layer".into(), "  ".into()),
            ],
            sections: vec![SourceSection {
                heading_path: vec!["Storage".into()],
                depth: 1,
                start_line: 4,
                end_line: 20,
                own_end_line: 9,
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

    /// The document span is the whole file, so it matches the whole-file
    /// hash — not the last section's end, which can fall short of it.
    #[test]
    fn document_span_is_every_line_of_the_file() {
        assert_eq!(document().span(), (1, 24));
        let empty = SourceDocument {
            sections: Vec::new(),
            line_count: 3,
            ..document()
        };
        assert_eq!(empty.span(), (1, 3), "a document with no sections too");
    }

    /// A section's span must stop where its own content stops, because that
    /// is what its `content_hash` covers.
    #[test]
    fn section_span_covers_its_own_content_not_its_subtree() {
        let section = &document().sections[0];
        assert_eq!(section.end_line, 20, "the subtree runs to 20");
        assert_eq!(section.span(), (4, 9), "but its own content stops at 9");
    }

    #[test]
    fn stem_drops_directories_and_extension() {
        assert_eq!(document().stem(), "0007-storage");
    }
}
