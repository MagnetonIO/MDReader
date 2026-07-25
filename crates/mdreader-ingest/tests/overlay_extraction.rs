//! Overlay extraction wired into ingest, end to end.
//!
//! Two things are proven here, and they are different:
//!
//! 1. **The adapter is faithful.** The real comrak parser, run through
//!    [`mdreader_ingest::source_document`], produces a corpus the extractors
//!    read to *exactly* the facts — same rules, same line ranges, same
//!    confidences — that `mdreader-domain`'s golden test pins with its own
//!    hand-rolled fixture reader. That is why this file reads the **domain
//!    crate's** fixtures rather than a private copy: the value of the test is
//!    that two independent readers of the same bytes agree, and a private copy
//!    would let them drift apart silently.
//!
//! 2. **Extraction during ingest writes no unapproved fact into the graph.**
//!    Proposals land in the proposal store; the overlay entities stay empty
//!    until a human approves one, and a rejected fact never appears at all.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions};
use mdreader_domain::extract::{architecture, decision, ApprovalState, Extraction, SourceDocument};
use mdreader_ingest::proposals::{
    approve_and_promote, reject_and_retract, ProposalFilter, ProposalStore, StoredFact,
};
use uuid::Uuid;

/// The `mdreader-domain` fixture corpus — deliberately shared, see the module
/// docs.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../mdreader-domain/tests/fixtures")
}

/// Every `.md` file under `root`, read through the REAL parser and adapted
/// into the extractors' input view, in sorted repo-relative path order (the
/// order the ingestor itself uses).
fn corpus_through_the_parser(root: &Path) -> Vec<SourceDocument> {
    let mut paths = Vec::new();
    collect(root, root, &mut paths);
    paths.sort();
    paths
        .iter()
        .map(|relative| {
            let text = std::fs::read_to_string(root.join(relative)).expect("fixture is readable");
            mdreader_ingest::parse_source_document(relative, &text)
        })
        .collect()
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("readable directory")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(
                path.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

/// `(entity, rule, value, file, start, end, confidence)` — the same shape the
/// domain crate's golden pins.
fn object_rows(extraction: &Extraction) -> Vec<(String, String, String, String, u32, u32, f32)> {
    extraction
        .objects
        .iter()
        .map(|extracted| {
            let provenance = &extracted.object.provenance;
            let (start, end) = provenance.range.expect("range");
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

/// `(arrow, rule, file, start, end)` — as above.
fn morphism_rows(extraction: &Extraction) -> Vec<(String, String, String, u32, u32)> {
    extraction
        .morphisms
        .iter()
        .map(|extracted| {
            let provenance = &extracted.morphism.provenance;
            let (start, end) = provenance.range.expect("range");
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

// ---------------------------------------------------------------------------
// Adapter goldens
// ---------------------------------------------------------------------------

/// Parsed markdown in, exactly these decision facts out — file, rule, value,
/// line range, confidence. Pinned literally rather than compared against a
/// second run of the extractor, so a change to either the parser or the
/// adapter has to be justified line by line.
#[test]
fn the_adapter_feeds_the_decision_rules_exactly_the_facts_the_goldens_pin() {
    let extraction = decision::extract(&corpus_through_the_parser(&fixture_root()));
    assert_eq!(extraction.extractor, "mdreader.decision@1");
    assert_eq!(extraction.schema, "decision_overlay");

    assert_eq!(
        object_rows(&extraction),
        vec![
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

    // One supersedes edge, from the link under `## Supersedes` on line 14 —
    // not from the link to the same document under `## Consequences`.
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
}

/// The architecture rules, same treatment. This is the one that would catch a
/// parser difference in heading nesting: `Path Prover` owns a `#### Limitations`
/// subsection, so its subtree runs to line 25 while the fact must be cited to
/// 17-20 — the span its hash covers.
#[test]
fn the_adapter_feeds_the_architecture_rules_exactly_the_facts_the_goldens_pin() {
    let extraction = architecture::extract(&corpus_through_the_parser(&fixture_root()));
    assert_eq!(extraction.extractor, "mdreader.architecture@1");

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
                25,
                0.95
            ),
            (
                "architecture_overlay.Layer".into(),
                "A2".into(),
                r#"{"layer_name":"platform"}"#.into(),
                "docs/arch/catdb-core.md".into(),
                1,
                25,
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
                20,
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

    let arrows: Vec<(String, u32, u32)> = morphism_rows(&extraction)
        .into_iter()
        .map(|(arrow, _, _, start, end)| (arrow, start, end))
        .collect();
    assert_eq!(
        arrows,
        vec![
            ("architecture_overlay.inLayer".to_string(), 1, 15),
            ("architecture_overlay.inLayer".to_string(), 1, 25),
            ("architecture_overlay.inLayer".to_string(), 1, 19),
            ("architecture_overlay.inLayer".to_string(), 1, 13),
            ("architecture_overlay.exposes".to_string(), 13, 16),
            ("architecture_overlay.exposes".to_string(), 17, 20),
            ("architecture_overlay.exposes".to_string(), 13, 16),
            ("architecture_overlay.dependant".to_string(), 15, 15),
            ("architecture_overlay.provider".to_string(), 15, 15),
            ("architecture_overlay.via".to_string(), 15, 15),
            ("architecture_overlay.dependant".to_string(), 19, 19),
            ("architecture_overlay.provider".to_string(), 19, 19),
            ("architecture_overlay.via".to_string(), 19, 19),
            ("architecture_overlay.dependant".to_string(), 13, 13),
            ("architecture_overlay.provider".to_string(), 13, 13),
            ("architecture_overlay.via".to_string(), 13, 13),
        ]
    );
}

/// The span contract, re-derived from the raw files against facts the REAL
/// parser produced.
///
/// `mdreader-domain` asserts this over its own fixture reader; the adapter is
/// the other producer of the same view, and it is the one that has to
/// *recompute* the section hash (the parser's own `content_hash` mixes in the
/// title and heading path). If it ever reused that field instead, every
/// section-cited fact would carry a hash that names lines it does not cover —
/// and only this assertion would notice.
#[test]
fn every_fact_from_the_real_parser_hashes_exactly_the_lines_its_range_names() {
    let root = fixture_root();
    let corpus = corpus_through_the_parser(&root);
    let mut checked = 0usize;
    for extraction in [decision::extract(&corpus), architecture::extract(&corpus)] {
        let provenances = extraction
            .objects
            .iter()
            .map(|object| (&object.object.provenance, object.rule.clone()))
            .chain(
                extraction
                    .morphisms
                    .iter()
                    .map(|morphism| (&morphism.morphism.provenance, morphism.rule.clone())),
            );
        for (provenance, rule) in provenances {
            let file = provenance.source.as_ref().expect("source");
            let (start, end) = provenance.range.expect("range");
            let text = std::fs::read_to_string(root.join(file)).expect("readable");
            let lines: Vec<&str> = text.lines().collect();
            let cited = lines[(start as usize - 1)..(end as usize)].join("\n");
            assert_eq!(
                provenance.content_hash.as_deref(),
                Some(format!("blake3:{}", blake3::hash(cited.as_bytes()).to_hex()).as_str()),
                "{rule} cites {file}:{start}-{end} but hashed something else"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4 + 28, "every fact in the corpus was checked");
}

// ---------------------------------------------------------------------------
// Extraction during ingest
// ---------------------------------------------------------------------------

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).expect("create destination");
    for entry in std::fs::read_dir(source).expect("readable source") {
        let entry = entry.expect("dir entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// A workspace: a scratch copy of the fixture corpus, an engine whose ingest
/// records proposals, and the store it records into.
struct Workspace {
    _scratch: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
    proposals_path: PathBuf,
    store: Arc<ProposalStore>,
    engine: EmbeddedCatDb,
}

fn workspace() -> Workspace {
    let scratch = tempfile::tempdir().expect("tempdir");
    let root = scratch.path().join("corpus");
    copy_tree(&fixture_root(), &root);
    let db = scratch.path().join("catdb.db");
    let proposals_path = scratch.path().join("proposals.db");
    let store = Arc::new(ProposalStore::open(&proposals_path).expect("open proposal store"));
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(&db, Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration_with_proposals(Arc::clone(
                &store,
            )))
            .with_actor("overlay-test"),
    )
    .expect("engine opens");
    Workspace {
        _scratch: scratch,
        root,
        db,
        proposals_path,
        store,
        engine,
    }
}

/// Every overlay object and morphism actually committed to the graph.
fn overlay_in_graph(engine: &EmbeddedCatDb) -> (Vec<String>, Vec<String>) {
    let mut objects = Vec::new();
    let mut morphisms = Vec::new();
    for schema in ["decision_overlay", "architecture_overlay"] {
        for header in engine
            .store()
            .objects_with_entity_prefix(&format!("{schema}."))
            .expect("read objects")
        {
            objects.push(format!("{}:{}", header.entity, header.id));
        }
        for edge in engine
            .store()
            .morphisms_with_arrow_prefix(&format!("{schema}."))
            .expect("read morphisms")
        {
            morphisms.push(format!("{}:{}->{}", edge.arrow, edge.source, edge.target));
        }
    }
    objects.sort();
    morphisms.sort();
    (objects, morphisms)
}

fn import(workspace: &Workspace) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime
        .block_on(
            workspace
                .engine
                .import(&workspace.root, ImportOptions::default()),
        )
        .expect("import");
}

/// THE property this whole design exists for: an import extracts, and not one
/// extracted fact reaches the authoritative graph, because not one of them has
/// been approved.
#[test]
fn extraction_during_ingest_writes_no_unapproved_fact_into_the_overlay_graph() {
    let workspace = workspace();
    import(&workspace);

    let proposals = workspace
        .store
        .list(&ProposalFilter::default())
        .expect("list");
    assert_eq!(
        proposals.len(),
        4 + 28,
        "the whole corpus was extracted into the proposal store"
    );
    assert!(
        proposals
            .iter()
            .all(|proposal| proposal.state() == ApprovalState::Proposed),
        "extraction proposes; it never approves"
    );

    let (objects, morphisms) = overlay_in_graph(&workspace.engine);
    assert!(
        objects.is_empty() && morphisms.is_empty(),
        "the overlay graph must be empty until a human approves something, got {objects:?} / {morphisms:?}"
    );

    // And the corpus itself was still indexed — extraction is an addition to
    // ingest, not a replacement for it.
    let status = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(workspace.engine.status())
        .expect("status");
    assert_eq!(status.documents, 7);
}

/// Approving promotes the fact into the graph, through the approved path — the
/// reviewer is stamped on `provenance.approved_by`, which is exactly what the
/// engine's approval check reads.
#[test]
fn approving_a_proposal_puts_it_in_the_graph_with_its_reviewer_recorded() {
    let workspace = workspace();
    import(&workspace);

    let component = workspace
        .store
        .list(&ProposalFilter {
            source_path: Some("docs/arch/catdb-core.md".into()),
            ..ProposalFilter::default()
        })
        .expect("list")
        .into_iter()
        .find(|proposal| proposal.qualified() == "architecture_overlay.Component")
        .expect("the catdb-core component was proposed");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let outcome = runtime
        .block_on(approve_and_promote(
            &workspace.store,
            &workspace.engine,
            component.fact_id,
            "alice",
        ))
        .expect("approve");
    assert_eq!(outcome.promoted, vec![component.fact_id]);

    let stored = workspace
        .engine
        .store()
        .objects_with_entity_prefix("architecture_overlay.")
        .expect("read back");
    assert_eq!(stored.len(), 1, "only the approved fact is in the graph");
    assert_eq!(stored[0].id, component.fact_id);
    assert_eq!(stored[0].entity, "architecture_overlay.Component");
    // Provenance survives the promotion intact, reviewer included.
    assert_eq!(
        stored[0].provenance.source.as_deref(),
        Some("docs/arch/catdb-core.md")
    );
    assert_eq!(stored[0].provenance.range, Some([1, 25]));
    assert_eq!(
        stored[0].provenance.extractor.as_deref(),
        Some("mdreader.architecture@1")
    );
    assert_eq!(stored[0].provenance.approved_by.as_deref(), Some("alice"));

    // ...and the store agrees.
    let after = workspace
        .store
        .get(component.fact_id)
        .expect("get")
        .expect("still there");
    assert_eq!(after.state(), ApprovalState::Approved);
    assert_eq!(after.review().reviewer.as_deref(), Some("alice"));
}

/// A rejected fact never reaches the graph — not on rejection, and not on the
/// re-import that re-extracts it.
#[test]
fn a_rejected_proposal_never_reaches_the_graph_even_across_a_reimport() {
    let workspace = workspace();
    import(&workspace);

    let decision = workspace
        .store
        .list(&ProposalFilter {
            schema: Some("decision_overlay".into()),
            source_path: Some("docs/adr/0007-storage.md".into()),
            ..ProposalFilter::default()
        })
        .expect("list")
        .into_iter()
        .find(|proposal| matches!(proposal.fact, StoredFact::Object(_)))
        .expect("the ADR decision was proposed");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let outcome = runtime
        .block_on(reject_and_retract(
            &workspace.store,
            &workspace.engine,
            decision.fact_id,
            "bob",
        ))
        .expect("reject");
    assert!(
        outcome.retracted.is_empty(),
        "nothing was ever written, so nothing is retracted"
    );

    let (objects, morphisms) = overlay_in_graph(&workspace.engine);
    assert!(objects.is_empty() && morphisms.is_empty());

    // A second import re-extracts the same fact — and must NOT re-propose it,
    // or every re-import would silently re-ask a question already answered.
    runtime
        .block_on(
            workspace
                .engine
                .import(&workspace.root, ImportOptions::default()),
        )
        .expect("re-import");
    let after = workspace
        .store
        .get(decision.fact_id)
        .expect("get")
        .expect("still there");
    assert_eq!(after.state(), ApprovalState::Rejected);
    assert_eq!(after.review().reviewer.as_deref(), Some("bob"));

    let (objects, morphisms) = overlay_in_graph(&workspace.engine);
    assert!(
        objects.is_empty() && morphisms.is_empty(),
        "a rejected fact is never materialized, however many imports run"
    );
}

/// An edge is withheld until BOTH its endpoints are approved — approving the
/// second endpoint is what finally emits it, and it must arrive with its own
/// provenance (the link's line), not the endpoint's.
#[test]
fn an_edge_lands_only_once_both_its_endpoints_are_approved() {
    let workspace = workspace();
    import(&workspace);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let all = workspace
        .store
        .list(&ProposalFilter {
            schema: Some("decision_overlay".into()),
            ..ProposalFilter::default()
        })
        .expect("list");
    let supersedes = all
        .iter()
        .find(|proposal| proposal.qualified() == "decision_overlay.supersedes")
        .expect("the supersedes edge was proposed");
    let StoredFact::Morphism(edge) = &supersedes.fact else {
        panic!("supersedes is a morphism");
    };
    let (source, target) = (
        *edge.morphism.source.as_uuid(),
        *edge.morphism.target.as_uuid(),
    );

    // Approve the edge itself first: both endpoints are still proposals, so
    // nothing may be written.
    let outcome = runtime
        .block_on(approve_and_promote(
            &workspace.store,
            &workspace.engine,
            supersedes.fact_id,
            "alice",
        ))
        .expect("approve edge");
    assert!(
        outcome.promoted.is_empty(),
        "an edge with unapproved endpoints is withheld"
    );
    let (_, morphisms) = overlay_in_graph(&workspace.engine);
    assert!(morphisms.is_empty());

    // First endpoint: still nothing to hang the edge on.
    runtime
        .block_on(approve_and_promote(
            &workspace.store,
            &workspace.engine,
            source,
            "alice",
        ))
        .expect("approve source");
    let (_, morphisms) = overlay_in_graph(&workspace.engine);
    assert!(morphisms.is_empty(), "one endpoint is not enough");

    // Second endpoint: the withheld edge is emitted with it.
    let outcome = runtime
        .block_on(approve_and_promote(
            &workspace.store,
            &workspace.engine,
            target,
            "alice",
        ))
        .expect("approve target");
    assert!(outcome.promoted.contains(&supersedes.fact_id));
    let (_, morphisms) = overlay_in_graph(&workspace.engine);
    assert_eq!(
        morphisms,
        vec![format!("decision_overlay.supersedes:{source}->{target}")]
    );
}

/// Rejecting an approved object takes its approved edges down with it: an edge
/// whose endpoint a human has ruled out cannot stand, which is the same "no
/// dangling edges" invariant `approved_operations` enforces on the way in.
#[test]
fn rejecting_an_approved_object_retracts_it_and_cascades_to_its_edges() {
    let workspace = workspace();
    import(&workspace);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let all = workspace
        .store
        .list(&ProposalFilter {
            schema: Some("decision_overlay".into()),
            ..ProposalFilter::default()
        })
        .expect("list");
    let supersedes = all
        .iter()
        .find(|proposal| proposal.qualified() == "decision_overlay.supersedes")
        .expect("edge");
    let StoredFact::Morphism(edge) = &supersedes.fact else {
        panic!("morphism")
    };
    let (source, target) = (
        *edge.morphism.source.as_uuid(),
        *edge.morphism.target.as_uuid(),
    );
    for id in [source, target, supersedes.fact_id] {
        runtime
            .block_on(approve_and_promote(
                &workspace.store,
                &workspace.engine,
                id,
                "alice",
            ))
            .expect("approve");
    }
    let (objects, morphisms) = overlay_in_graph(&workspace.engine);
    assert_eq!(objects.len(), 2);
    assert_eq!(morphisms.len(), 1);

    let outcome = runtime
        .block_on(reject_and_retract(
            &workspace.store,
            &workspace.engine,
            source,
            "bob",
        ))
        .expect("reject");
    assert_eq!(outcome.cascaded, vec![supersedes.fact_id]);
    assert_eq!(outcome.retracted, vec![supersedes.fact_id, source]);

    let (objects, morphisms) = overlay_in_graph(&workspace.engine);
    assert!(morphisms.is_empty(), "the dangling edge went with it");
    assert_eq!(objects.len(), 1, "the other endpoint stands");
    assert_eq!(
        workspace
            .store
            .get(supersedes.fact_id)
            .expect("get")
            .expect("there")
            .state(),
        ApprovalState::Rejected,
        "the cascade is recorded, not just applied"
    );
}

/// The whole point of a *durable* store: the Inspector must be able to review
/// a corpus indexed yesterday, in a process that no longer holds the engine
/// that indexed it.
#[test]
fn the_proposal_store_survives_a_reopen_with_its_review_decisions_intact() {
    let workspace = workspace();
    import(&workspace);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let component = workspace
        .store
        .list(&ProposalFilter {
            source_path: Some("docs/arch/catdb-core.md".into()),
            ..ProposalFilter::default()
        })
        .expect("list")
        .into_iter()
        .find(|proposal| proposal.qualified() == "architecture_overlay.Component")
        .expect("proposed");
    runtime
        .block_on(approve_and_promote(
            &workspace.store,
            &workspace.engine,
            component.fact_id,
            "alice",
        ))
        .expect("approve");
    let before = workspace
        .store
        .list(&ProposalFilter::default())
        .expect("list");

    // A new session: new store handle, new engine handle, same two files.
    let Workspace {
        _scratch,
        root,
        db,
        proposals_path,
        store,
        engine,
    } = workspace;
    drop(store);
    drop(engine);

    let reopened = ProposalStore::open(&proposals_path).expect("reopen");
    let after = reopened.list(&ProposalFilter::default()).expect("list");
    assert_eq!(before, after, "every proposal survived, byte for byte");
    assert_eq!(
        reopened
            .get(component.fact_id)
            .expect("get")
            .expect("there")
            .review()
            .reviewer
            .as_deref(),
        Some("alice"),
        "and so did who reviewed it"
    );

    // The graph the approval wrote is durable too.
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(&db, Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration_with_proposals(Arc::new(
                ProposalStore::open(&proposals_path).expect("open"),
            )))
            .with_actor("overlay-test"),
    )
    .expect("engine reopens");
    let (objects, _) = overlay_in_graph(&engine);
    assert_eq!(objects.len(), 1);
    let _ = root;
}

/// Re-importing an unchanged corpus must change no review state — the ids
/// extraction derives are deterministic, so every fact is *recognized* rather
/// than proposed afresh.
#[test]
fn a_reimport_of_an_unchanged_corpus_recognizes_every_fact_it_already_holds() {
    let workspace = workspace();
    import(&workspace);
    let first = workspace
        .store
        .list(&ProposalFilter::default())
        .expect("list");
    import(&workspace);
    let second = workspace
        .store
        .list(&ProposalFilter::default())
        .expect("list");
    assert_eq!(first, second);
    assert_eq!(first.len(), 32);
}

/// When the cited text itself changes under a fact's (deterministic) id, the
/// review resets: a human accepted *that* text, and this is no longer it.
#[test]
fn editing_the_cited_text_re_proposes_the_fact_it_supported() {
    let workspace = workspace();
    import(&workspace);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let interface = workspace
        .store
        .list(&ProposalFilter {
            source_path: Some("docs/arch/catdb-engine.md".into()),
            ..ProposalFilter::default()
        })
        .expect("list")
        .into_iter()
        .find(|proposal| proposal.qualified() == "architecture_overlay.Interface")
        .expect("Search API was proposed");
    runtime
        .block_on(approve_and_promote(
            &workspace.store,
            &workspace.engine,
            interface.fact_id,
            "alice",
        ))
        .expect("approve");

    // Rewrite the prose *under* the `### Search API` heading. The heading (and
    // so the fact's identity) is unchanged; the text it cites is not.
    let path = workspace.root.join("docs/arch/catdb-engine.md");
    let text = std::fs::read_to_string(&path).expect("read");
    std::fs::write(
        &path,
        text.replace(
            "Hybrid retrieval over the indexed corpus.",
            "Completely different prose about something else entirely.",
        ),
    )
    .expect("write");
    import(&workspace);

    let after = workspace
        .store
        .get(interface.fact_id)
        .expect("get")
        .expect("still there");
    assert_eq!(
        after.state(),
        ApprovalState::Proposed,
        "the approval was for text that no longer exists"
    );
    assert_eq!(after.review().reviewer, None);
}

/// A workspace with no proposal store configured extracts nothing — there
/// would be nowhere to put a proposal, and the one place it must never go is
/// the graph. This is the `catdb` CLI's configuration.
#[test]
fn ingest_without_a_proposal_store_extracts_nothing_at_all() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let root = scratch.path().join("corpus");
    copy_tree(&fixture_root(), &root);
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(
            scratch.path().join("catdb.db"),
            Arc::new(HashEmbedder::new()),
        )
        .with_domain(mdreader_ingest::registration())
        .with_actor("plain"),
    )
    .expect("engine opens");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(engine.import(&root, ImportOptions::default()))
        .expect("import");
    let (objects, morphisms) = overlay_in_graph(&engine);
    assert!(objects.is_empty() && morphisms.is_empty());
}

/// A proposal id is the fact's own id, so the Inspector can ask for exactly
/// the facts derived from a document + line range and get them back located.
#[test]
fn proposals_are_addressable_by_document_and_overlapping_line_range() {
    let workspace = workspace();
    import(&workspace);

    // Lines 11-16 of catdb-engine.md cover `## Interfaces` and its
    // `### Search API` child.
    let hits = workspace
        .store
        .list(&ProposalFilter {
            source_path: Some("docs/arch/catdb-engine.md".into()),
            line_range: Some((13, 16)),
            ..ProposalFilter::default()
        })
        .expect("list");
    let named: Vec<(String, u32, u32)> = hits
        .iter()
        .map(|proposal| {
            let (start, end) = proposal.provenance().range.expect("range");
            (proposal.qualified(), start, end)
        })
        .collect();
    assert_eq!(
        named,
        vec![
            // Grouped by the span each fact cites (the whole-file component
            // facts overlap the selection), objects before morphisms within a
            // span — the order an Inspector renders top-down.
            ("architecture_overlay.Component".to_string(), 1, 19),
            ("architecture_overlay.inLayer".to_string(), 1, 19),
            ("architecture_overlay.Interface".to_string(), 13, 16),
            ("architecture_overlay.exposes".to_string(), 13, 16),
        ],
        "located facts come back grouped by span, objects first"
    );
    // ...and the dependency on line 19 does not.
    assert!(!hits
        .iter()
        .any(|proposal| proposal.qualified().ends_with("Dependency")));
}

/// An id nobody proposed is an error, not a silent no-op — a UI that lost
/// track of a fact must find out.
#[test]
fn approving_an_unknown_id_is_an_error() {
    let workspace = workspace();
    import(&workspace);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(approve_and_promote(
        &workspace.store,
        &workspace.engine,
        Uuid::nil(),
        "alice",
    ));
    assert!(result.is_err());
}
