//! The `decision_overlay` extractor: rules-based, deterministic, no model.
//!
//! # Rules
//!
//! **D1 — decision document.** A document whose frontmatter declares
//! `type` (or `kind`, or `doc_type`) as one of `decision`, `adr`,
//! `architecture-decision`, `architecture decision record` becomes exactly
//! one `Decision`. Its `decision_title` is the frontmatter `title` when
//! present, otherwise the document title. Confidence **0.95** — the document
//! says what it is, in a machine-readable field; there is nothing to infer.
//! Provenance spans the whole file, because the evidence is the frontmatter
//! plus the title rather than any one section.
//!
//! **D2 — decision section.** In a document D1 did *not* claim, a section
//! whose own heading reads `Decision: <something>` becomes one `Decision`
//! titled `<something>`. Confidence **0.80** — the heading is explicit, but
//! a section is a weaker unit of record than a whole document, and prose
//! headings are more easily accidental. Provenance is that section's span.
//!
//! **D3 — supersedes.** In a document D1 claimed, every link inside a section
//! whose own heading is exactly `Supersedes` (or `Supersedes:`) is promoted
//! to `supersedes : Decision → Decision`, provided the link resolves to a
//! *different* document that D1 also claimed. Confidence **0.90** —
//! superseding is stated under a heading whose only job is to state it.
//! Provenance is the link's own line, so the UI lands on the link itself.
//!
//! # Deliberately not extracted
//!
//! * **`Option`, `Requirement`, `Technology` objects and the `selects`,
//!   `motivated_by`, `applies_to` morphisms.** Deciding that a bullet under
//!   `## Options` is an option — and worse, that the bolded one is the
//!   *selected* one — is reading the author's mind. Markdown offers no
//!   marker for it, conventions differ per template, and a wrong `selects`
//!   edge misrepresents a decision that was actually made. Left to a human,
//!   or to a later model-backed extractor whose output is reviewed.
//! * **Links between decision records that are not under a `Supersedes`
//!   heading.** A decision that merely *mentions* another one is syntactic
//!   evidence and stays that way; "relates to" is not a relation this schema
//!   declares, and coercing it into `supersedes` would invert meaning.
//! * **A frontmatter `supersedes:` key.** The parsed view carries no line
//!   number for a frontmatter entry, so a fact derived from one could not
//!   satisfy this module's own locatability requirement. When the parser
//!   grows frontmatter line spans this becomes a straightforward D4.
//! * **`Decision` objects for documents merely *filed under* `decisions/`.**
//!   Directory layout is a filing convention, not a declaration.

use super::{morphism, object, stable_uuid, Derivation, Extraction, SourceDocument};
use crate::extract::resolve::{resolve_document, slug};

/// This extractor's id and version, recorded on every fact it produces.
pub const EXTRACTOR: &str = "mdreader.decision@1";

/// The overlay schema these facts are typed against.
pub const SCHEMA: &str = "decision_overlay";

/// Frontmatter keys inspected for the document-type declaration (D1).
const TYPE_KEYS: &[&str] = &["type", "kind", "doc_type"];

/// Frontmatter type values that declare a decision record (D1).
const DECISION_TYPES: &[&str] = &[
    "decision",
    "adr",
    "architecture decision",
    "architecture decision record",
];

/// Confidence of each rule; see the module docs for the reasoning.
const D1_CONFIDENCE: f32 = 0.95;
const D2_CONFIDENCE: f32 = 0.80;
const D3_CONFIDENCE: f32 = 0.90;

/// Whether D1 claims this document: does its frontmatter declare it a
/// decision record?
pub fn is_decision_document(document: &SourceDocument) -> bool {
    document
        .frontmatter_any(TYPE_KEYS)
        .map(slug)
        .is_some_and(|declared| DECISION_TYPES.contains(&declared.as_str()))
}

/// Stable id of the `Decision` a document declares under D1.
fn document_decision_id(path: &str) -> uuid::Uuid {
    stable_uuid(&[SCHEMA, "Decision", "document", path])
}

/// Stable id of the `Decision` a section declares under D2.
fn section_decision_id(path: &str, start_line: u32) -> uuid::Uuid {
    stable_uuid(&[SCHEMA, "Decision", "section", path, &start_line.to_string()])
}

/// Run the decision extractor over a whole corpus.
///
/// The corpus, not a single file, is the unit of extraction: D3 may only
/// assert `supersedes` when it can see that the *target* is also a decision
/// record, which is knowledge no single document carries.
pub fn extract(corpus: &[SourceDocument]) -> Extraction {
    let mut extraction = Extraction {
        extractor: EXTRACTOR.to_string(),
        schema: SCHEMA.to_string(),
        ..Extraction::default()
    };

    for document in corpus {
        if is_decision_document(document) {
            // D1
            let (start, end) = document.span();
            let title = document
                .frontmatter_any(&["title"])
                .unwrap_or(&document.title);
            extraction.objects.push(object(
                SCHEMA,
                "Decision",
                document_decision_id(&document.path),
                serde_json::json!({ "decision_title": title }),
                "D1",
                &Derivation {
                    extractor: EXTRACTOR,
                    file: &document.path,
                    lines: (start, end),
                    content_hash: &document.content_hash,
                    confidence: D1_CONFIDENCE,
                },
            ));
        } else {
            // D2 — only in documents D1 did not already claim, so a
            // `## Decision` section inside an ADR never doubles its record.
            for section in &document.sections {
                let Some(title) = decision_section_title(section.heading()) else {
                    continue;
                };
                extraction.objects.push(object(
                    SCHEMA,
                    "Decision",
                    section_decision_id(&document.path, section.start_line),
                    serde_json::json!({ "decision_title": title }),
                    "D2",
                    &Derivation {
                        extractor: EXTRACTOR,
                        file: &document.path,
                        lines: section.span(),
                        content_hash: &section.content_hash,
                        confidence: D2_CONFIDENCE,
                    },
                ));
            }
        }
    }

    // D3 — a second pass, because it needs every D1 verdict decided first.
    for document in corpus.iter().filter(|d| is_decision_document(d)) {
        let source = document_decision_id(&document.path);
        for section in &document.sections {
            if slug(section.heading()).trim_end_matches(':') != "supersedes" {
                continue;
            }
            for link in &section.links {
                let target = super::split_fragment(&link.raw_target);
                let Some(index) = resolve_document(corpus, &document.path, target.document) else {
                    continue; // unresolved or ambiguous: no assertion
                };
                let target_document = &corpus[index];
                // Only decision records can be superseded, and nothing
                // supersedes itself.
                if target_document.path == document.path || !is_decision_document(target_document) {
                    continue;
                }
                extraction.morphisms.push(morphism(
                    SCHEMA,
                    "supersedes",
                    source,
                    document_decision_id(&target_document.path),
                    "D3",
                    &Derivation {
                        extractor: EXTRACTOR,
                        file: &document.path,
                        lines: (link.line, link.line),
                        // The cited span is the link's line, so that is what
                        // is hashed — not the enclosing section.
                        content_hash: &super::blake3_hex(&link.line_text),
                        confidence: D3_CONFIDENCE,
                    },
                ));
            }
        }
    }

    extraction
}

/// The title a `Decision: <title>` heading declares (D2), or `None` when the
/// heading is not of that shape. A bare `## Decision` declares no title and
/// is deliberately not enough.
fn decision_section_title(heading: &str) -> Option<&str> {
    let (prefix, rest) = heading.split_once(':')?;
    (slug(prefix) == "decision")
        .then(|| rest.trim())
        .filter(|title| !title.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_declared_decision_types_only() {
        for value in ["decision", "ADR", "Architecture-Decision", "adr"] {
            assert!(slug(value).len() > 1);
            assert!(DECISION_TYPES.contains(&slug(value).as_str()), "{value}");
        }
        for value in ["note", "component", "decisions"] {
            assert!(!DECISION_TYPES.contains(&slug(value).as_str()), "{value}");
        }
    }

    #[test]
    fn a_decision_heading_must_carry_a_title() {
        assert_eq!(
            decision_section_title("Decision: use SQLite"),
            Some("use SQLite")
        );
        assert_eq!(
            decision_section_title("decision:  Adopt Tauri"),
            Some("Adopt Tauri")
        );
        // A bare heading, or a colon with nothing after it, declares nothing.
        assert_eq!(decision_section_title("Decision"), None);
        assert_eq!(decision_section_title("Decision:   "), None);
        // And an unrelated heading that happens to contain a colon does not
        // become a decision.
        assert_eq!(decision_section_title("Context: background"), None);
    }

    #[test]
    fn identities_are_stable_and_distinct_per_origin() {
        let a = document_decision_id("docs/a.md");
        assert_eq!(a, document_decision_id("docs/a.md"));
        assert_ne!(a, document_decision_id("docs/b.md"));
        assert_ne!(a, section_decision_id("docs/a.md", 1));
        assert_ne!(
            section_decision_id("docs/a.md", 1),
            section_decision_id("docs/a.md", 2)
        );
    }
}
