//! The `architecture_overlay` extractor: rules-based, deterministic, no model.
//!
//! # Rules
//!
//! **A1 — component.** A document whose frontmatter declares `type` (or
//! `kind`) as one of `component`, `service`, `crate`, `module`, `package`
//! becomes exactly one `Component`. `component_name` is the frontmatter
//! `name` when present, otherwise the document title; `component_kind` is the
//! declared value. Confidence **0.95**; provenance spans the whole file.
//!
//! **A2 — layer.** A component document with a frontmatter `layer` value
//! yields a `Layer` (deduplicated across the corpus by normalized name) and
//! an `inLayer : Component → Layer`. Confidence **0.95**. The `Layer`
//! object's provenance points at the first document that named it — the
//! layer is a corpus-level concept, and pretending it was declared in one
//! place it was not would be a false citation.
//!
//! **A3 — interface.** Inside a component document, every *direct child* of a
//! section whose own heading is `Interface`, `Interfaces` or `API` becomes an
//! `Interface` named by that child's heading, plus `exposes : Interface →
//! Component` back to the document's own component. Confidence **0.85** —
//! the enclosing heading is an explicit declaration, but heading nesting is
//! a weaker signal than a frontmatter field. Provenance is the child
//! section's span.
//!
//! **A4 — dependency.** Inside a component document, every link in a section
//! whose own heading is `Depends on`, `Dependencies` or `Depends upon`, whose
//! target has the form `<document>#<interface>`, becomes a reified
//! `Dependency` plus its three morphisms `dependant`, `provider`, `via` —
//! but only when *both* halves resolve: `<document>` to another A1 component
//! and `<interface>` to exactly one A3 interface in the corpus. Confidence
//! **0.90**. Provenance is the link's own line.
//!
//! # Why A4 resolves the two halves independently
//!
//! The schema's one law is `Dependency.via.exposes = Dependency.provider`:
//! *the interface you depend on must be exposed by the component you say you
//! depend on*. That law is only worth checking because its two sides come
//! from different places in the corpus — `provider` from the dependant's own
//! link, `exposes` from the provider document's interface section. Resolving
//! the interface *globally* (rather than "the interface under the component
//! I already resolved") is what keeps them independent, and therefore what
//! lets a genuinely inconsistent corpus produce a genuine violation instead
//! of a tautology.
//!
//! # Deliberately not extracted
//!
//! * **Dependencies with no interface fragment** (`[[catdb-core]]` under
//!   `## Depends on`). The schema requires a `via`, and picking one of a
//!   provider's several interfaces on the author's behalf is guessing. The
//!   link stays syntactic evidence.
//! * **Dependencies inferred from prose links, code fences, or import
//!   statements.** A component document that mentions another in a sentence
//!   has not declared a dependency.
//! * **Components inferred from directory layout** (`crates/foo/README.md`),
//!   from a heading that happens to name a crate, or from a `Cargo.toml`
//!   sitting next door. Only a frontmatter declaration counts.
//! * **Interface *signatures* or their types.** The overlay models that an
//!   interface exists and who exposes it; parsing an API surface out of prose
//!   is a different job with a different error profile.
//! * **`inLayer` for components with no declared `layer`.** Arrows are total
//!   in a schema, so a partial `inLayer` is visible as such rather than
//!   filled in with a guessed default.

use std::collections::HashMap;

use super::{
    morphism, object, stable_uuid, Derivation, ExtractedObject, Extraction, SourceDocument,
    SourceSection,
};
use crate::extract::resolve::{resolve_document, slug, split_fragment};

/// This extractor's id and version, recorded on every fact it produces.
pub const EXTRACTOR: &str = "mdreader.architecture@1";

/// The overlay schema these facts are typed against.
pub const SCHEMA: &str = "architecture_overlay";

/// Frontmatter keys inspected for the document-type declaration (A1).
const TYPE_KEYS: &[&str] = &["type", "kind"];

/// Frontmatter type values that declare a component (A1).
const COMPONENT_TYPES: &[&str] = &["component", "service", "crate", "module", "package"];

/// Section headings that introduce an interface listing (A3).
const INTERFACE_HEADINGS: &[&str] = &["interface", "interfaces", "api"];

/// Section headings that introduce a dependency listing (A4).
const DEPENDENCY_HEADINGS: &[&str] = &["depends on", "dependencies", "depends upon"];

const A1_CONFIDENCE: f32 = 0.95;
const A2_CONFIDENCE: f32 = 0.95;
const A3_CONFIDENCE: f32 = 0.85;
const A4_CONFIDENCE: f32 = 0.90;

/// The `component_kind` a document declares under A1, or `None`.
pub fn component_kind(document: &SourceDocument) -> Option<String> {
    let declared = slug(document.frontmatter_any(TYPE_KEYS)?);
    COMPONENT_TYPES
        .contains(&declared.as_str())
        .then_some(declared)
}

fn component_id(path: &str) -> uuid::Uuid {
    stable_uuid(&[SCHEMA, "Component", path])
}

fn layer_id(name: &str) -> uuid::Uuid {
    stable_uuid(&[SCHEMA, "Layer", &slug(name)])
}

fn interface_id(path: &str, start_line: u32) -> uuid::Uuid {
    stable_uuid(&[SCHEMA, "Interface", path, &start_line.to_string()])
}

fn dependency_id(path: &str, line: u32, raw_target: &str) -> uuid::Uuid {
    stable_uuid(&[SCHEMA, "Dependency", path, &line.to_string(), raw_target])
}

/// One interface discovered by A3, kept for A4's global name resolution.
struct DiscoveredInterface {
    id: uuid::Uuid,
    slug: String,
}

/// Run the architecture extractor over a whole corpus.
///
/// Three passes, because each depends on the previous one's verdicts:
/// components (A1/A2), then their interfaces (A3), then the dependencies
/// (A4) that reference both.
pub fn extract(corpus: &[SourceDocument]) -> Extraction {
    let mut extraction = Extraction {
        extractor: EXTRACTOR.to_string(),
        schema: SCHEMA.to_string(),
        ..Extraction::default()
    };

    // Pass 1 — A1 components, and A2 layers.
    let mut components: HashMap<usize, uuid::Uuid> = HashMap::new();
    let mut layers: HashMap<String, uuid::Uuid> = HashMap::new();
    for (index, document) in corpus.iter().enumerate() {
        let Some(kind) = component_kind(document) else {
            continue;
        };
        let id = component_id(&document.path);
        components.insert(index, id);
        let derivation = Derivation {
            extractor: EXTRACTOR,
            file: &document.path,
            lines: document.span(),
            content_hash: &document.content_hash,
            confidence: A1_CONFIDENCE,
        };
        let name = document
            .frontmatter_any(&["name"])
            .unwrap_or(&document.title);
        extraction.objects.push(object(
            SCHEMA,
            "Component",
            id,
            serde_json::json!({ "component_name": name, "component_kind": kind }),
            "A1",
            &derivation,
        ));

        // A2
        let Some(layer) = document.frontmatter_any(&["layer"]) else {
            continue;
        };
        let layer_derivation = Derivation {
            confidence: A2_CONFIDENCE,
            ..derivation
        };
        let layer_key = slug(layer);
        let layer_object = *layers.entry(layer_key).or_insert_with(|| {
            let id = layer_id(layer);
            extraction.objects.push(object(
                SCHEMA,
                "Layer",
                id,
                serde_json::json!({ "layer_name": layer }),
                "A2",
                &layer_derivation,
            ));
            id
        });
        extraction.morphisms.push(morphism(
            SCHEMA,
            "inLayer",
            id,
            layer_object,
            "A2",
            &layer_derivation,
        ));
    }

    // Pass 2 — A3 interfaces, indexed by normalized name for A4.
    let mut by_name: HashMap<String, Vec<DiscoveredInterface>> = HashMap::new();
    for (index, component) in sorted(&components) {
        let document = &corpus[index];
        for section in child_sections_of(document, INTERFACE_HEADINGS) {
            let name = document.sections[section].heading();
            if name.is_empty() {
                continue;
            }
            let section = &document.sections[section];
            let id = interface_id(&document.path, section.start_line);
            let derivation = Derivation {
                extractor: EXTRACTOR,
                file: &document.path,
                lines: section.span(),
                content_hash: &section.content_hash,
                confidence: A3_CONFIDENCE,
            };
            extraction.objects.push(object(
                SCHEMA,
                "Interface",
                id,
                serde_json::json!({ "interface_name": name }),
                "A3",
                &derivation,
            ));
            extraction.morphisms.push(morphism(
                SCHEMA,
                "exposes",
                id,
                component,
                "A3",
                &derivation,
            ));
            by_name
                .entry(slug(name))
                .or_default()
                .push(DiscoveredInterface {
                    id,
                    slug: slug(name),
                });
        }
    }

    // Pass 3 — A4 dependencies.
    for (index, dependant) in sorted(&components) {
        let document = &corpus[index];
        for section in document
            .sections
            .iter()
            .filter(|s| DEPENDENCY_HEADINGS.contains(&heading_slug(s).as_str()))
        {
            for link in &section.links {
                let target = split_fragment(&link.raw_target);
                let Some(fragment) = target.fragment else {
                    continue; // no interface named: nothing defensible to assert
                };
                let Some(provider_index) =
                    resolve_document(corpus, &document.path, target.document)
                else {
                    continue;
                };
                let Some(&provider) = components.get(&provider_index) else {
                    continue; // the target is not a declared component
                };
                // Resolve the interface GLOBALLY and only when unambiguous —
                // see the module docs on why this must not be scoped to the
                // provider.
                let Some(interface) = by_name.get(&slug(fragment)).filter(|c| c.len() == 1) else {
                    continue;
                };
                let interface = &interface[0];
                debug_assert_eq!(interface.slug, slug(fragment));

                let id = dependency_id(&document.path, link.line, &link.raw_target);
                let derivation = Derivation {
                    extractor: EXTRACTOR,
                    file: &document.path,
                    lines: (link.line, link.line),
                    content_hash: &section.content_hash,
                    confidence: A4_CONFIDENCE,
                };
                extraction.objects.push(object(
                    SCHEMA,
                    "Dependency",
                    id,
                    serde_json::json!({ "dependency_kind": "depends_on" }),
                    "A4",
                    &derivation,
                ));
                for (arrow, target) in [
                    ("dependant", dependant),
                    ("provider", provider),
                    ("via", interface.id),
                ] {
                    extraction.morphisms.push(morphism(
                        SCHEMA,
                        arrow,
                        id,
                        target,
                        "A4",
                        &derivation,
                    ));
                }
            }
        }
    }

    extraction
}

/// Component indices in corpus order — `HashMap` iteration is not ordered,
/// and this module's output ordering is part of its contract.
fn sorted(components: &HashMap<usize, uuid::Uuid>) -> Vec<(usize, uuid::Uuid)> {
    let mut pairs: Vec<(usize, uuid::Uuid)> = components.iter().map(|(&i, &id)| (i, id)).collect();
    pairs.sort_by_key(|(index, _)| *index);
    pairs
}

fn heading_slug(section: &SourceSection) -> String {
    slug(section.heading()).trim_end_matches(':').to_string()
}

/// Indices of the sections that are direct children of a section whose own
/// heading is one of `headings`.
fn child_sections_of(document: &SourceDocument, headings: &[&str]) -> Vec<usize> {
    let parents: Vec<usize> = document
        .sections
        .iter()
        .enumerate()
        .filter(|(_, s)| headings.contains(&heading_slug(s).as_str()))
        .map(|(index, _)| index)
        .collect();
    document
        .sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.parent.is_some_and(|p| parents.contains(&p)))
        .map(|(index, _)| index)
        .collect()
}

/// The objects an extraction proposes for one entity, in output order.
/// Exposed for tests and for the review UI's per-entity listings.
pub fn objects_of<'a>(extraction: &'a Extraction, entity: &str) -> Vec<&'a ExtractedObject> {
    let qualified = format!("{SCHEMA}.{entity}");
    extraction
        .objects
        .iter()
        .filter(|o| o.object.entity.as_str() == qualified)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::SourceLink;

    fn section(
        heading: &str,
        depth: u8,
        start: u32,
        end: u32,
        parent: Option<usize>,
    ) -> SourceSection {
        SourceSection {
            heading_path: vec![heading.to_string()],
            depth,
            start_line: start,
            end_line: end,
            parent,
            content_hash: format!("blake3:{heading}"),
            links: Vec::new(),
        }
    }

    #[test]
    fn only_declared_component_types_count() {
        let mut document = SourceDocument {
            path: "docs/x.md".into(),
            title: "X".into(),
            content_hash: "blake3:x".into(),
            frontmatter: vec![("type".into(), "Crate".into())],
            sections: vec![section("X", 1, 1, 3, None)],
        };
        assert_eq!(component_kind(&document).as_deref(), Some("crate"));
        document.frontmatter = vec![("type".into(), "decision".into())];
        assert_eq!(component_kind(&document), None);
        document.frontmatter = Vec::new();
        assert_eq!(component_kind(&document), None);
    }

    #[test]
    fn child_sections_are_found_under_any_interface_heading_spelling() {
        let document = SourceDocument {
            path: "docs/x.md".into(),
            title: "X".into(),
            content_hash: "blake3:x".into(),
            frontmatter: Vec::new(),
            sections: vec![
                section("X", 1, 1, 20, None),                    // 0
                section("Interfaces", 2, 3, 12, Some(0)),        // 1
                section("Search API", 3, 5, 8, Some(1)),         // 2
                section("Ingest", 3, 9, 12, Some(1)),            // 3
                section("Notes", 2, 13, 20, Some(0)),            // 4
                section("Not an interface", 3, 15, 20, Some(4)), // 5
            ],
        };
        assert_eq!(child_sections_of(&document, INTERFACE_HEADINGS), vec![2, 3]);
    }

    #[test]
    fn a_dependency_link_without_an_interface_fragment_is_ignored() {
        let mut depends = section("Depends on", 2, 5, 9, Some(0));
        depends.links = vec![
            SourceLink {
                raw_target: "catdb-core".into(),
                kind: "wikilink".into(),
                line: 6,
            },
            SourceLink {
                raw_target: "catdb-core#Search API".into(),
                kind: "wikilink".into(),
                line: 7,
            },
        ];
        let corpus = vec![
            SourceDocument {
                path: "docs/app.md".into(),
                title: "app".into(),
                content_hash: "blake3:app".into(),
                frontmatter: vec![("type".into(), "crate".into())],
                sections: vec![section("app", 1, 1, 9, None), depends],
            },
            SourceDocument {
                path: "docs/catdb-core.md".into(),
                title: "catdb-core".into(),
                content_hash: "blake3:core".into(),
                frontmatter: vec![("type".into(), "crate".into())],
                sections: vec![
                    section("catdb-core", 1, 1, 9, None),
                    section("Interfaces", 2, 3, 9, Some(0)),
                    section("Search API", 3, 5, 9, Some(1)),
                ],
            },
        ];
        let extraction = extract(&corpus);
        let dependencies = objects_of(&extraction, "Dependency");
        assert_eq!(dependencies.len(), 1, "only the fragment-bearing link");
        assert_eq!(
            dependencies[0].object.provenance.range,
            Some((7, 7)),
            "provenance points at the link's own line"
        );
    }

    /// A4 resolves the interface fragment *globally*, which is what keeps the
    /// schema law falsifiable — but it means an interface name claimed by two
    /// components is genuinely ambiguous. Picking one would fabricate a `via`
    /// edge (and, through the law, an apparent violation) out of nothing, so
    /// nothing is asserted at all.
    #[test]
    fn an_ambiguous_interface_name_yields_no_dependency_at_all() {
        let component = |path: &str, title: &str, interface: &str| SourceDocument {
            path: path.into(),
            title: title.into(),
            content_hash: format!("blake3:{title}"),
            frontmatter: vec![("type".into(), "crate".into())],
            sections: vec![
                section(title, 1, 1, 9, None),
                section("Interfaces", 2, 3, 9, Some(0)),
                section(interface, 3, 5, 9, Some(1)),
            ],
        };
        let mut depends = section("Depends on", 2, 5, 7, Some(0));
        depends.links = vec![SourceLink {
            raw_target: "one#Search API".into(),
            kind: "wikilink".into(),
            line: 6,
        }];
        let mut corpus = vec![
            SourceDocument {
                path: "docs/app.md".into(),
                title: "app".into(),
                content_hash: "blake3:app".into(),
                frontmatter: vec![("type".into(), "crate".into())],
                sections: vec![section("app", 1, 1, 7, None), depends],
            },
            component("docs/one.md", "one", "Search API"),
        ];
        // With a single `Search API` in the corpus the dependency extracts...
        assert_eq!(objects_of(&extract(&corpus), "Dependency").len(), 1);

        // ...and the moment a second component claims the same interface
        // name, the extractor refuses to guess which one was meant.
        corpus.push(component("docs/two.md", "two", "Search API"));
        let extraction = extract(&corpus);
        assert!(
            objects_of(&extraction, "Dependency").is_empty(),
            "an ambiguous interface name must not produce a dependency"
        );
        // The unambiguous facts around it survive: three components, and one
        // interface per declaring document.
        assert_eq!(objects_of(&extraction, "Component").len(), 3);
        assert_eq!(objects_of(&extraction, "Interface").len(), 2);
    }
}
