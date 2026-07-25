//! The [`ParsedDocument`] → [`SourceDocument`] adapter: how real parsed
//! markdown reaches the domain's overlay extractors.
//!
//! `mdreader-domain` is parser-free by design — it declares its own input view
//! ([`SourceDocument`]) because `mdreader-ingest` depends on *it*, not the
//! other way around. This module is the one place the two shapes meet.
//!
//! # It is not a field-for-field copy
//!
//! Two fields must be **recomputed**, and getting either wrong is silent:
//!
//! * [`SourceSection::content_hash`]. [`ParsedSection::content_hash`] hashes
//!   `(title, heading_path, own text)` — deliberately, so a heading rename
//!   re-embeds its descendants. A fact citing `file:start-own_end` and
//!   carrying *that* hash could never be checked against the lines it names,
//!   which silently destroys the drift detection the hash exists for. So the
//!   overlay view hashes exactly the lines its span covers, sliced out of the
//!   source here rather than borrowed from any field.
//! * [`SourceLink::line_text`]. The parser records a link's line number but
//!   not its text; a fact cited to that single line needs the line's own hash.
//!
//! Both are sliced from the same `lines` vector the ranges index into, so the
//! "range and hash describe the same span" contract holds by construction.
//!
//! Everything else is a structural copy. Sections stay **1:1 and in order**
//! with [`ParsedDocument::sections`], because [`SourceSection::parent`] is an
//! index into that list — dropping the preamble (which is inert for every
//! extraction rule: its heading is empty) would silently re-point every
//! parent.

use mdreader_domain::extract::{SourceDocument, SourceLink, SourceSection};

use crate::parse::{parse_markdown, ParsedDocument};

/// blake3 hash in the `blake3:<hex>` form the parser and the extractors share.
fn blake3_hex(text: &str) -> String {
    format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
}

/// The text of lines `start ..= end` (1-based, inclusive), newline-joined.
/// Clamped to what the file actually has, so an out-of-range span yields the
/// text of the lines that exist rather than panicking.
fn lines_text(lines: &[&str], start: u32, end: u32) -> String {
    let first = (start.max(1) as usize - 1).min(lines.len());
    let last = (end.max(start) as usize).min(lines.len());
    if first >= last {
        return String::new();
    }
    lines[first..last].join("\n")
}

/// Adapt an already-parsed document into the overlay extractors' input view.
///
/// `source_text` is the same text `parsed` was produced from: the recomputed
/// hashes are sliced out of it (see the module docs).
pub fn source_document(path: &str, source_text: &str, parsed: &ParsedDocument) -> SourceDocument {
    let lines: Vec<&str> = source_text.lines().collect();
    let sections = parsed
        .sections
        .iter()
        .map(|section| {
            // The span a fact cites for this section — its OWN content, which
            // is exactly what the hash below covers.
            let start = section.start_line;
            let own_end = section.own_end_line.max(section.start_line);
            SourceSection {
                heading_path: section.heading_path.clone(),
                depth: section.depth,
                start_line: start,
                end_line: section.end_line.max(start),
                own_end_line: own_end,
                parent: section.parent,
                content_hash: blake3_hex(&lines_text(&lines, start, own_end)),
                links: section
                    .links
                    .iter()
                    .map(|link| SourceLink {
                        raw_target: link.raw_target.clone(),
                        kind: link.kind.clone(),
                        line: link.line,
                        line_text: lines
                            .get(link.line.saturating_sub(1) as usize)
                            .map(|text| (*text).to_string())
                            .unwrap_or_default(),
                    })
                    .collect(),
            }
        })
        .collect();

    SourceDocument {
        path: path.to_string(),
        title: parsed.title.clone(),
        // Exactly the lines `SourceDocument::span()` names: the whole file,
        // newline-normalized. NOT the raw file bytes (the ingest layer keeps
        // its own byte hash for change detection — a different job).
        content_hash: blake3_hex(&lines.join("\n")),
        line_count: lines.len() as u32,
        frontmatter: parsed.frontmatter.clone(),
        sections,
    }
}

/// Parse `source_text` and adapt it in one step — the entry point for callers
/// that hold only the text (tests, and any consumer outside the ingest loop,
/// which parses once and reuses the [`ParsedDocument`]).
///
/// The fallback title is the file stem of `path`, matching how ingest names a
/// document with no level-1 heading.
pub fn parse_source_document(path: &str, source_text: &str) -> SourceDocument {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(path);
    let parsed = parse_markdown(source_text, stem);
    source_document(path, source_text, &parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The section hash must be the hash of the lines the section's span
    /// names — NOT the parser's own `content_hash`, which mixes in the title
    /// and heading path.
    #[test]
    fn the_section_hash_is_rebuilt_and_is_not_the_parsers() {
        let text = "# Top\n\nbody\n\n## Sub\n\nmore\n";
        let parsed = parse_markdown(text, "t");
        let view = source_document("a.md", text, &parsed);
        let top = &view.sections[0];
        assert_eq!(top.span(), (1, 4));
        assert_eq!(top.content_hash, blake3_hex("# Top\n\nbody\n"));
        assert_ne!(
            top.content_hash, parsed.sections[0].content_hash,
            "the parser's hash mixes in title/heading path and must not leak through"
        );
    }

    /// A fact cited to a link's line hashes that line, so the link's own text
    /// has to survive the adaptation.
    #[test]
    fn links_carry_the_text_of_the_line_they_sit_on() {
        let text = "# T\n\n## Supersedes\n\n- [[0003-storage]]\n";
        let view = parse_source_document("docs/a.md", text);
        let link = &view.sections[1].links[0];
        assert_eq!(link.line, 5);
        assert_eq!(link.line_text, "- [[0003-storage]]");
    }

    /// `SourceSection::parent` indexes `SourceDocument::sections`, so the two
    /// section lists must stay 1:1 — including the (extraction-inert) preamble.
    #[test]
    fn sections_stay_one_to_one_with_the_parse_so_parent_indices_hold() {
        let text = "intro\n\n# A\n\n## B\n\nb\n";
        let parsed = parse_markdown(text, "t");
        let view = source_document("a.md", text, &parsed);
        assert_eq!(view.sections.len(), parsed.sections.len());
        assert_eq!(view.sections[0].depth, 0, "the preamble is kept");
        assert_eq!(view.sections[2].parent, Some(1));
        assert_eq!(view.sections[2].heading_path, vec!["A", "B"]);
    }

    /// The document hash covers exactly `span()` — every line of the file,
    /// newline-normalized — so a CRLF file hashes the same as its LF twin.
    #[test]
    fn the_document_hash_covers_every_line_newline_normalized() {
        let view = parse_source_document("a.md", "# A\r\n\r\nbody\r\n");
        assert_eq!(view.line_count, 3);
        assert_eq!(view.span(), (1, 3));
        assert_eq!(view.content_hash, blake3_hex("# A\n\nbody"));
    }

    #[test]
    fn the_title_falls_back_to_the_file_stem() {
        let view = parse_source_document("docs/notes/0007-storage.md", "no heading here\n");
        assert_eq!(view.title, "0007-storage");
        assert_eq!(
            parse_source_document("docs/a.md", "# Real Title\n").title,
            "Real Title"
        );
    }

    /// Frontmatter reaches the rules: D1/A1 read it to decide what a document
    /// even is.
    #[test]
    fn frontmatter_pairs_reach_the_view() {
        let view = parse_source_document("a.md", "---\ntype: decision\n---\n\n# T\n");
        assert_eq!(view.frontmatter_any(&["type"]), Some("decision"));
    }

    /// An empty file must not panic the slicer or claim a span it has no
    /// lines for.
    #[test]
    fn an_empty_document_has_no_sections_and_a_degenerate_span() {
        let view = parse_source_document("a.md", "");
        assert_eq!(view.line_count, 0);
        assert!(view.sections.is_empty());
        assert_eq!(view.content_hash, blake3_hex(""));
    }
}
