//! End-to-end proof of the overlay extractors over a real fixture corpus.
//!
//! The corpus under `tests/fixtures/` is small enough to line-count by hand,
//! which is the point: every assertion below names a file, a line range and a
//! confidence a reader can verify against the markdown itself.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig};
use mdreader_domain::extract::{architecture, decision, ApprovalState, Extraction, SourceDocument};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn corpus() -> Vec<SourceDocument> {
    support::read_corpus(&fixture_root())
}

/// `(entity, rule, value, file, start, end, confidence)` for every proposed
/// object, in output order — the shape the golden tests pin.
fn object_rows(extraction: &Extraction) -> Vec<(String, String, String, String, u32, u32, f32)> {
    extraction
        .objects
        .iter()
        .map(|extracted| {
            let provenance = &extracted.object.provenance;
            let (start, end) = provenance.range.expect("every object carries a line range");
            (
                extracted.object.entity.to_string(),
                extracted.rule.clone(),
                extracted.object.value.to_string(),
                provenance.source.clone().expect("source"),
                start,
                end,
                provenance.confidence.expect("confidence"),
            )
        })
        .collect()
}

/// `(arrow, source_title_or_id, target, file, line)` is unreadable, so
/// morphisms are pinned as `(arrow, rule, file, start, end)` plus a separate
/// endpoint check against the objects they connect.
fn morphism_rows(extraction: &Extraction) -> Vec<(String, String, String, u32, u32)> {
    extraction
        .morphisms
        .iter()
        .map(|extracted| {
            let provenance = &extracted.morphism.provenance;
            let (start, end) = provenance.range.expect("line range");
            (
                extracted.morphism.arrow.to_string(),
                extracted.rule.clone(),
                provenance.source.clone().expect("source"),
                start,
                end,
            )
        })
        .collect()
}

/// The id of the single object whose `value` contains `needle`.
fn object_id(extraction: &Extraction, needle: &str) -> uuid::Uuid {
    let matches: Vec<_> = extraction
        .objects
        .iter()
        .filter(|o| o.object.value.to_string().contains(needle))
        .collect();
    assert_eq!(matches.len(), 1, "`{needle}` must identify one object");
    *matches[0].object.id.as_uuid()
}

// ---------------------------------------------------------------------------
// Golden: exactly these facts, from exactly these lines
// ---------------------------------------------------------------------------

#[test]
fn decision_extraction_proposes_exactly_the_declared_decisions() {
    let extraction = decision::extract(&corpus());
    assert_eq!(extraction.extractor, "mdreader.decision@1");
    assert_eq!(extraction.schema, "decision_overlay");

    assert_eq!(
        object_rows(&extraction),
        vec![
            // D1: frontmatter `type: decision`, titled from frontmatter.
            (
                "decision_overlay.Decision".into(),
                "D1".into(),
                r#"{"decision_title":"Use flat files for storage"}"#.into(),
                "docs/adr/0003-storage.md".into(),
                1,
                10,
                0.95
            ),
            (
                "decision_overlay.Decision".into(),
                "D1".into(),
                r#"{"decision_title":"Store the catalog in SQLite"}"#.into(),
                "docs/adr/0007-storage.md".into(),
                1,
                18,
                0.95
            ),
            // D2: `## Decision: <title>` in a document with no frontmatter
            // type. Its span is the section, lines 3-6.
            (
                "decision_overlay.Decision".into(),
                "D2".into(),
                r#"{"decision_title":"adopt SQLite for the embedded store"}"#.into(),
                "docs/notes/meeting.md".into(),
                3,
                6,
                0.8
            ),
        ]
    );

    // D3: exactly ONE supersedes edge, from the link on line 14 under
    // `## Supersedes` — NOT from the link to the same document on line 18
    // under `## Consequences`, and NOT from meeting.md's passing mention.
    assert_eq!(
        morphism_rows(&extraction),
        vec![(
            "decision_overlay.supersedes".into(),
            "D3".into(),
            "docs/adr/0007-storage.md".into(),
            14,
            14
        )]
    );
    let edge = &extraction.morphisms[0].morphism;
    assert_eq!(
        *edge.source.as_uuid(),
        object_id(&extraction, "Store the catalog in SQLite")
    );
    assert_eq!(
        *edge.target.as_uuid(),
        object_id(&extraction, "Use flat files for storage")
    );
}

#[test]
fn architecture_extraction_proposes_exactly_the_declared_components() {
    let extraction = architecture::extract(&corpus());
    assert_eq!(extraction.extractor, "mdreader.architecture@1");
    assert_eq!(extraction.schema, "architecture_overlay");

    assert_eq!(
        object_rows(&extraction),
        vec![
            (
                "architecture_overlay.Component".into(),
                "A1".into(),
                r#"{"component_kind":"crate","component_name":"broken-app"}"#.into(),
                "docs/arch/broken-app.md".into(),
                1,
                15,
                0.95
            ),
            // The `app` layer is cited to the first document that named it.
            (
                "architecture_overlay.Layer".into(),
                "A2".into(),
                r#"{"layer_name":"app"}"#.into(),
                "docs/arch/broken-app.md".into(),
                1,
                15,
                0.95
            ),
            (
                "architecture_overlay.Component".into(),
                "A1".into(),
                r#"{"component_kind":"crate","component_name":"catdb-core"}"#.into(),
                "docs/arch/catdb-core.md".into(),
                1,
                19,
                0.95
            ),
            (
                "architecture_overlay.Layer".into(),
                "A2".into(),
                r#"{"layer_name":"platform"}"#.into(),
                "docs/arch/catdb-core.md".into(),
                1,
                19,
                0.95
            ),
            (
                "architecture_overlay.Component".into(),
                "A1".into(),
                r#"{"component_kind":"crate","component_name":"catdb-engine"}"#.into(),
                "docs/arch/catdb-engine.md".into(),
                1,
                19,
                0.95
            ),
            (
                "architecture_overlay.Component".into(),
                "A1".into(),
                r#"{"component_kind":"crate","component_name":"mdreader-app"}"#.into(),
                "docs/arch/mdreader-app.md".into(),
                1,
                13,
                0.95
            ),
            // A3: the direct children of `## Interfaces`, at their own spans.
            (
                "architecture_overlay.Interface".into(),
                "A3".into(),
                r#"{"interface_name":"Schema Validation"}"#.into(),
                "docs/arch/catdb-core.md".into(),
                13,
                16,
                0.85
            ),
            (
                "architecture_overlay.Interface".into(),
                "A3".into(),
                r#"{"interface_name":"Path Prover"}"#.into(),
                "docs/arch/catdb-core.md".into(),
                17,
                19,
                0.85
            ),
            (
                "architecture_overlay.Interface".into(),
                "A3".into(),
                r#"{"interface_name":"Search API"}"#.into(),
                "docs/arch/catdb-engine.md".into(),
                13,
                16,
                0.85
            ),
            // A4: one reified dependency per resolvable `doc#interface` link,
            // located at the link's own line.
            (
                "architecture_overlay.Dependency".into(),
                "A4".into(),
                r#"{"dependency_kind":"depends_on"}"#.into(),
                "docs/arch/broken-app.md".into(),
                15,
                15,
                0.9
            ),
            (
                "architecture_overlay.Dependency".into(),
                "A4".into(),
                r#"{"dependency_kind":"depends_on"}"#.into(),
                "docs/arch/catdb-engine.md".into(),
                19,
                19,
                0.9
            ),
            (
                "architecture_overlay.Dependency".into(),
                "A4".into(),
                r#"{"dependency_kind":"depends_on"}"#.into(),
                "docs/arch/mdreader-app.md".into(),
                13,
                13,
                0.9
            ),
        ]
    );

    assert_eq!(
        morphism_rows(&extraction),
        vec![
            (
                "architecture_overlay.inLayer".into(),
                "A2".into(),
                "docs/arch/broken-app.md".into(),
                1,
                15
            ),
            (
                "architecture_overlay.inLayer".into(),
                "A2".into(),
                "docs/arch/catdb-core.md".into(),
                1,
                19
            ),
            (
                "architecture_overlay.inLayer".into(),
                "A2".into(),
                "docs/arch/catdb-engine.md".into(),
                1,
                19
            ),
            (
                "architecture_overlay.inLayer".into(),
                "A2".into(),
                "docs/arch/mdreader-app.md".into(),
                1,
                13
            ),
            (
                "architecture_overlay.exposes".into(),
                "A3".into(),
                "docs/arch/catdb-core.md".into(),
                13,
                16
            ),
            (
                "architecture_overlay.exposes".into(),
                "A3".into(),
                "docs/arch/catdb-core.md".into(),
                17,
                19
            ),
            (
                "architecture_overlay.exposes".into(),
                "A3".into(),
                "docs/arch/catdb-engine.md".into(),
                13,
                16
            ),
            (
                "architecture_overlay.dependant".into(),
                "A4".into(),
                "docs/arch/broken-app.md".into(),
                15,
                15
            ),
            (
                "architecture_overlay.provider".into(),
                "A4".into(),
                "docs/arch/broken-app.md".into(),
                15,
                15
            ),
            (
                "architecture_overlay.via".into(),
                "A4".into(),
                "docs/arch/broken-app.md".into(),
                15,
                15
            ),
            (
                "architecture_overlay.dependant".into(),
                "A4".into(),
                "docs/arch/catdb-engine.md".into(),
                19,
                19
            ),
            (
                "architecture_overlay.provider".into(),
                "A4".into(),
                "docs/arch/catdb-engine.md".into(),
                19,
                19
            ),
            (
                "architecture_overlay.via".into(),
                "A4".into(),
                "docs/arch/catdb-engine.md".into(),
                19,
                19
            ),
            (
                "architecture_overlay.dependant".into(),
                "A4".into(),
                "docs/arch/mdreader-app.md".into(),
                13,
                13
            ),
            (
                "architecture_overlay.provider".into(),
                "A4".into(),
                "docs/arch/mdreader-app.md".into(),
                13,
                13
            ),
            (
                "architecture_overlay.via".into(),
                "A4".into(),
                "docs/arch/mdreader-app.md".into(),
                13,
                13
            ),
        ]
    );

    // The broken dependency's two sides really do disagree — this is the
    // corpus fact the schema law exists to catch.
    let core = object_id(&extraction, r#""component_name":"catdb-core""#);
    let engine = object_id(&extraction, r#""component_name":"catdb-engine""#);
    let search_api = object_id(&extraction, r#""interface_name":"Search API""#);
    let broken = &extraction.morphisms[7..10];
    assert_eq!(*broken[1].morphism.target.as_uuid(), core, "provider");
    assert_eq!(*broken[2].morphism.target.as_uuid(), search_api, "via");
    // ...while `Search API` is exposed by catdb-engine, not catdb-core.
    let exposes_search = extraction
        .morphisms
        .iter()
        .find(|m| *m.morphism.source.as_uuid() == search_api)
        .expect("exposes edge");
    assert_eq!(*exposes_search.morphism.target.as_uuid(), engine);
}

#[test]
fn re_extracting_an_unchanged_corpus_is_bit_for_bit_identical() {
    let corpus = corpus();
    assert_eq!(decision::extract(&corpus), decision::extract(&corpus));
    assert_eq!(
        architecture::extract(&corpus),
        architecture::extract(&corpus)
    );
}

#[test]
fn every_proposed_fact_carries_a_complete_derivation_record() {
    let corpus = corpus();
    for extraction in [decision::extract(&corpus), architecture::extract(&corpus)] {
        let provenances = extraction
            .objects
            .iter()
            .map(|o| (&o.object.provenance, o.rule.clone()))
            .chain(
                extraction
                    .morphisms
                    .iter()
                    .map(|m| (&m.morphism.provenance, m.rule.clone())),
            );
        for (provenance, rule) in provenances {
            assert!(provenance.source.is_some(), "{rule}: source file");
            assert!(provenance.range.is_some(), "{rule}: line range");
            assert!(provenance.content_hash.is_some(), "{rule}: content hash");
            assert_eq!(
                provenance.schema_version.as_deref(),
                Some("1"),
                "{rule}: schema version"
            );
            assert_eq!(
                provenance.extractor.as_deref(),
                Some(extraction.extractor.as_str()),
                "{rule}: extractor id"
            );
            let confidence = provenance.confidence.expect("confidence");
            assert!((0.0..=1.0).contains(&confidence), "{rule}: {confidence}");
            // Extraction never claims review.
            assert_eq!(provenance.approved_by, None, "{rule}");
        }
        // And every proposal starts unreviewed.
        assert!(extraction
            .objects
            .iter()
            .all(|o| o.review.state == ApprovalState::Proposed));
        assert!(extraction
            .morphisms
            .iter()
            .all(|m| m.review.state == ApprovalState::Proposed));
    }
}

/// A section's recorded content hash must be the hash of the span the
/// provenance points at — otherwise "the source moved under this fact" could
/// never be detected.
#[test]
fn a_facts_content_hash_is_the_hash_of_the_span_it_names() {
    let corpus = corpus();
    let extraction = architecture::extract(&corpus);
    let interface = extraction
        .objects
        .iter()
        .find(|o| o.object.value.to_string().contains("Search API"))
        .expect("the Search API interface");
    let document = corpus
        .iter()
        .find(|d| d.path == "docs/arch/catdb-engine.md")
        .expect("provider document");
    let section = document
        .sections
        .iter()
        .find(|s| s.heading() == "Search API")
        .expect("its section");
    assert_eq!(
        interface.object.provenance.content_hash.as_deref(),
        Some(section.content_hash.as_str())
    );
}

// ---------------------------------------------------------------------------
// Non-destructive
// ---------------------------------------------------------------------------

/// Extraction is an **overlay**: the markdown is never rewritten, moved, or
/// annotated. Proven the only way worth proving it — by fingerprinting every
/// byte of a scratch copy of the corpus before and after a full extract →
/// apply → equation-check cycle.
#[tokio::test]
async fn extraction_leaves_the_corpus_byte_for_byte_identical() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let root = scratch.path().join("corpus");
    support::copy_tree(&fixture_root(), &root);

    let before = support::fingerprint(&root);
    assert_eq!(before.len(), 7, "the fixture corpus is seven files");

    let corpus = support::read_corpus(&root);
    let engine = open_engine();
    for extraction in [decision::extract(&corpus), architecture::extract(&corpus)] {
        engine
            .apply_operations(extraction.operations(), Some("extractor".into()))
            .await
            .expect("apply");
    }
    engine.check_equations().await.expect("check");

    let after = support::fingerprint(&root);
    assert_eq!(
        before, after,
        "extraction must not create, delete, or modify a single byte of the corpus"
    );
}

// ---------------------------------------------------------------------------
// Provenance round-trip
// ---------------------------------------------------------------------------

fn open_engine() -> EmbeddedCatDb {
    EmbeddedCatDb::open(
        EngineConfig::in_memory(Arc::new(HashEmbedder::new())).with_domain(
            mdreader_domain::engine_registration(Vec::new(), "fixture-chunker@1"),
        ),
    )
    .expect("engine opens with the mdreader domain")
}

#[tokio::test]
async fn extracted_provenance_survives_the_round_trip_through_storage() {
    let corpus = corpus();
    let extraction = architecture::extract(&corpus);
    let engine = open_engine();
    engine
        .apply_operations(extraction.operations(), Some("extractor".into()))
        .await
        .expect("apply");

    let stored = engine
        .store()
        .objects_with_entity_prefix("architecture_overlay.")
        .expect("read back");
    assert_eq!(stored.len(), extraction.objects.len());

    for extracted in &extraction.objects {
        let header = stored
            .iter()
            .find(|header| header.id == *extracted.object.id.as_uuid())
            .unwrap_or_else(|| panic!("object {} was stored", extracted.object.id));
        let original = &extracted.object.provenance;
        assert_eq!(header.entity, extracted.object.entity.to_string());
        assert_eq!(header.provenance.source, original.source);
        assert_eq!(
            header.provenance.range,
            original.range.map(|(start, end)| [start, end])
        );
        assert_eq!(header.provenance.content_hash, original.content_hash);
        assert_eq!(header.provenance.schema_version, original.schema_version);
        assert_eq!(header.provenance.extractor, original.extractor);
        assert_eq!(header.provenance.confidence, original.confidence);
        // Unreviewed on the way in, unreviewed on the way out.
        assert_eq!(header.provenance.approved_by, None);
    }
}

#[tokio::test]
async fn an_approved_fact_stores_its_reviewer() {
    let corpus = corpus();
    let mut extraction = decision::extract(&corpus);
    extraction.objects[0]
        .review
        .approve("alice")
        .expect("approve");
    let approved = *extraction.objects[0].object.id.as_uuid();

    let engine = open_engine();
    engine
        .apply_operations(extraction.operations(), Some("extractor".into()))
        .await
        .expect("apply");

    let stored = engine
        .store()
        .objects_with_entity_prefix("decision_overlay.")
        .expect("read back");
    for header in &stored {
        let expected = (header.id == approved).then(|| "alice".to_string());
        assert_eq!(header.provenance.approved_by, expected, "{}", header.id);
    }
}

// ---------------------------------------------------------------------------
// Approval gates materialization
// ---------------------------------------------------------------------------

#[test]
fn only_approved_facts_reach_the_authoritative_subset() {
    let mut extraction = decision::extract(&corpus());
    assert_eq!(extraction.objects.len(), 3);
    assert!(
        extraction.approved_operations().is_empty(),
        "nothing is authoritative until a human says so"
    );
    // All four facts (3 objects + 1 morphism) are proposals, and proposals do
    // enter the reviewable graph.
    assert_eq!(extraction.operations().len(), 4);

    extraction.objects[0].review.approve("alice").unwrap();
    extraction.objects[1].review.approve("alice").unwrap();
    extraction.morphisms[0].review.approve("alice").unwrap();
    assert_eq!(extraction.approved_operations().len(), 3);
}

#[tokio::test]
async fn a_rejected_fact_is_never_asserted() {
    let mut extraction = architecture::extract(&corpus());
    let rejected = *extraction.objects[0].object.id.as_uuid();
    extraction.objects[0].review.reject("alice").unwrap();

    let engine = open_engine();
    engine
        .apply_operations(extraction.operations(), Some("extractor".into()))
        .await
        .expect("apply");

    let stored = engine
        .store()
        .objects_with_entity_prefix("architecture_overlay.")
        .expect("read back");
    assert_eq!(stored.len(), extraction.objects.len() - 1);
    assert!(
        !stored.iter().any(|header| header.id == rejected),
        "a rejected proposal must not reach the graph"
    );
}

// ---------------------------------------------------------------------------
// Located violations
// ---------------------------------------------------------------------------

/// The payoff: a corpus that genuinely contradicts itself produces a
/// violation naming the law AND the exact markdown line that caused it.
///
/// `docs/arch/broken-app.md` says it depends on `catdb-core` *through*
/// `Search API`, but `Search API` is declared under `catdb-engine`. The
/// architecture overlay's law `Dependency.via.exposes = Dependency.provider`
/// is exactly that claim, and it fails.
#[tokio::test]
async fn the_inconsistent_dependency_is_reported_with_its_law_and_its_line() {
    let corpus = corpus();
    let extraction = architecture::extract(&corpus);
    let engine = open_engine();
    engine
        .apply_operations(extraction.operations(), Some("extractor".into()))
        .await
        .expect("apply");

    let report = engine.check_equations().await.expect("check");
    assert!(
        report.schemas_checked >= 1,
        "the check must actually have run"
    );
    assert_eq!(
        report.violations.len(),
        1,
        "only broken-app is inconsistent: {:?}",
        report.violation_messages()
    );

    let violation = &report.violations[0];
    assert_eq!(violation.schema, "architecture_overlay");
    assert_eq!(
        violation.law,
        "Dependency.via.exposes = Dependency.provider"
    );
    assert_eq!(violation.lhs_path, vec!["via", "exposes"]);
    assert_eq!(violation.rhs_path, vec!["provider"]);

    // The offending object is broken-app's dependency, and the UI can jump
    // straight to the line of markdown that asserted it.
    assert_eq!(violation.subject.entity, "architecture_overlay.Dependency");
    let span = violation.subject.source.as_ref().expect("located");
    assert_eq!(span.file, "docs/arch/broken-app.md");
    assert_eq!(span.start_line, Some(15));
    assert_eq!(span.end_line, Some(15));

    // Both landings are named, each locatable in its own file.
    let lhs = violation.lhs.as_ref().expect("lhs lands somewhere");
    let rhs = violation.rhs.as_ref().expect("rhs lands somewhere");
    assert_eq!(lhs.entity, "architecture_overlay.Component");
    assert_eq!(
        lhs.source.as_ref().map(|s| s.file.as_str()),
        Some("docs/arch/catdb-engine.md"),
        "via.exposes lands on the component that really exposes `Search API`"
    );
    assert_eq!(
        rhs.source.as_ref().map(|s| s.file.as_str()),
        Some("docs/arch/catdb-core.md"),
        "provider is the component broken-app claimed"
    );

    // And the prose rendering still carries all of it, for the CLI.
    let rendered = report.violation_messages();
    assert!(
        rendered[0].contains("architecture_overlay")
            && rendered[0].contains("via.exposes")
            && rendered[0].contains("docs/arch/broken-app.md:15"),
        "{}",
        rendered[0]
    );
}

/// The same corpus minus its one inconsistency checks clean — so the test
/// above is measuring the corpus, not a check that always fires.
#[tokio::test]
async fn the_consistent_part_of_the_corpus_checks_clean() {
    let corpus: Vec<SourceDocument> = corpus()
        .into_iter()
        .filter(|document| document.path != "docs/arch/broken-app.md")
        .collect();
    let extraction = architecture::extract(&corpus);
    let engine = open_engine();
    engine
        .apply_operations(extraction.operations(), Some("extractor".into()))
        .await
        .expect("apply");

    let report = engine.check_equations().await.expect("check");
    assert_eq!(report.schemas_checked, 1, "the check must have run");
    assert!(
        report.violations.is_empty(),
        "{:?}",
        report.violation_messages()
    );
}
