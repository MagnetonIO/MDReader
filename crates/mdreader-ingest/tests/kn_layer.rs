//! KN-layer acceptance: frontmatter `tags:` become `kn.Concept` /
//! `kn.ArtifactConcept` facts, documents become `kn.Artifact`s, resolved
//! links assert `kn.ArtifactLink` spans between artifacts that
//! `relation_search` traverses (graph test to depth 2), and every derived
//! fact is retracted when its source dies — with the instance-level
//! equation check clean throughout (fixtures included).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use catdb_sdk::embed::HashEmbedder;
use catdb_sdk::engine::{EmbeddedCatDb, EngineConfig, ImportOptions};
use uuid::Uuid;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn open(db: &Path) -> EmbeddedCatDb {
    EmbeddedCatDb::open(
        EngineConfig::new(db, Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration())
            .with_actor("kn-test"),
    )
    .unwrap()
}

/// Mirror of the ingestor's deterministic identity scheme — asserted here
/// so the scheme itself is contract, not accident (idempotent re-ingest
/// depends on it).
fn stable_uuid(parts: &[&str]) -> Uuid {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    Uuid::from_bytes(bytes)
}

fn concept_id(tag: &str) -> Uuid {
    stable_uuid(&["kn-concept", tag])
}

fn artifact_id(doc: Uuid) -> Uuid {
    stable_uuid(&["kn-artifact", &doc.to_string()])
}

fn artifact_concept_id(doc: Uuid, tag: &str) -> Uuid {
    stable_uuid(&["kn-artifact-concept", &doc.to_string(), tag])
}

/// Count stored `kn.*` objects by entity name.
fn kn_counts(engine: &EmbeddedCatDb) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for object in engine.store().objects_with_entity_prefix("kn.").unwrap() {
        *counts.entry(object.entity).or_insert(0) += 1;
    }
    counts
}

fn kn_object_ids(engine: &EmbeddedCatDb, entity: &str) -> Vec<Uuid> {
    engine
        .store()
        .objects_with_entity_prefix("kn.")
        .unwrap()
        .into_iter()
        .filter(|object| object.entity == entity)
        .map(|object| object.id)
        .collect()
}

/// The artifact-to-artifact edges encoded by the stored `kn.ArtifactLink`
/// spans: each span object's `kn.from` / `kn.to` projection morphisms are
/// joined on the span id into one `(from_artifact, to_artifact)` pair.
fn span_edges(engine: &EmbeddedCatDb) -> Vec<(Uuid, Uuid)> {
    let from: BTreeMap<Uuid, Uuid> = engine
        .store()
        .morphisms_with_arrow_prefix("kn.from")
        .unwrap()
        .into_iter()
        .filter(|e| e.arrow == "kn.from")
        .map(|e| (e.source, e.target))
        .collect();
    let to: BTreeMap<Uuid, Uuid> = engine
        .store()
        .morphisms_with_arrow_prefix("kn.to")
        .unwrap()
        .into_iter()
        .filter(|e| e.arrow == "kn.to")
        .map(|e| (e.source, e.target))
        .collect();
    assert_eq!(
        from.keys().collect::<Vec<_>>(),
        to.keys().collect::<Vec<_>>(),
        "every span must carry BOTH projections"
    );
    let mut edges: Vec<(Uuid, Uuid)> = from.iter().map(|(span, src)| (*src, to[span])).collect();
    edges.sort();
    edges
}

#[tokio::test]
async fn frontmatter_tags_become_concepts_and_survive_diffs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    // Inline array form, mixed case (concept identity is lowercased).
    write(
        &root,
        "a.md",
        "---\ntags: [Retrieval, graphs]\n---\n\n# A\n\nAlpha body.\n",
    );
    // Block-list form, quoted item.
    write(
        &root,
        "b.md",
        "---\ntitle: B\ntags:\n  - retrieval\n  - \"Search\"\n---\n\n# B\n\nBeta body.\n",
    );
    let db_dir = tempfile::tempdir().unwrap();
    let engine = open(&db_dir.path().join("catdb.db"));
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();

    let a = engine.store().document_by_path("a.md").unwrap().unwrap();
    let b = engine.store().document_by_path("b.md").unwrap().unwrap();

    // 2 artifacts, 3 concepts (`retrieval` is SHARED — global identity),
    // 4 join rows.
    let counts = kn_counts(&engine);
    assert_eq!(counts.get("kn.Artifact"), Some(&2));
    assert_eq!(counts.get("kn.Concept"), Some(&3));
    assert_eq!(counts.get("kn.ArtifactConcept"), Some(&4));

    // The deterministic identity scheme, exactly.
    let concepts = kn_object_ids(&engine, "kn.Concept");
    for tag in ["retrieval", "graphs", "search"] {
        assert!(
            concepts.contains(&concept_id(tag)),
            "concept `{tag}` must exist under its stable id"
        );
    }
    let artifacts = kn_object_ids(&engine, "kn.Artifact");
    assert!(artifacts.contains(&artifact_id(a.id)));
    assert!(artifacts.contains(&artifact_id(b.id)));

    // Join morphisms: 4 of each projection.
    assert_eq!(
        engine
            .store()
            .morphisms_with_arrow_prefix("kn.artifact")
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        engine
            .store()
            .morphisms_with_arrow_prefix("kn.concept")
            .unwrap()
            .len(),
        4
    );

    // Removing a tag retracts its join row (and morphisms) — the concept
    // object stays (stable identity; other/future artifacts may share it).
    write(
        &root,
        "a.md",
        "---\ntags: [Retrieval]\n---\n\n# A\n\nAlpha body.\n",
    );
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    let acs = kn_object_ids(&engine, "kn.ArtifactConcept");
    assert_eq!(acs.len(), 3);
    assert!(
        !acs.contains(&artifact_concept_id(a.id, "graphs")),
        "the dropped tag's join row must be retracted"
    );
    assert_eq!(kn_counts(&engine).get("kn.Concept"), Some(&3));

    // Deleting a document tears down its artifact and joins.
    std::fs::remove_file(root.join("b.md")).unwrap();
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    let counts = kn_counts(&engine);
    assert_eq!(counts.get("kn.Artifact"), Some(&1));
    assert_eq!(counts.get("kn.ArtifactConcept"), Some(&1));
    assert_eq!(
        engine
            .store()
            .morphisms_with_arrow_prefix("kn.artifact")
            .unwrap()
            .len(),
        1
    );

    // Re-import is idempotent (deterministic ids overwrite, never grow).
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(kn_counts(&engine), counts);

    let check = engine.check_equations().await.unwrap();
    assert!(
        check.violations.is_empty(),
        "corpus must be coherent: {:?}",
        check.violations
    );
    assert!(
        check.schemas_checked > 0,
        "the equation check must have run"
    );
}

#[tokio::test]
async fn resolved_links_assert_artifact_link_spans_that_related_traverses() {
    // Graph a -> b -> c, built purely from markdown links: resolution must
    // assert kn.ArtifactLink spans between ARTIFACTS, `related` must walk
    // them to depth 2 with depth-decayed scores, and death (link removed /
    // target deleted) must retract exactly the right edge.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    write(&root, "a.md", "# A\n\nStart [next](b.md).\n");
    write(&root, "b.md", "# B\n\nMiddle [end](c.md).\n");
    write(&root, "c.md", "# C\n\nTerminal note.\n");
    let db_dir = tempfile::tempdir().unwrap();
    let engine = open(&db_dir.path().join("catdb.db"));
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.links_resolved, 2);

    let a = engine.store().document_by_path("a.md").unwrap().unwrap();
    let b = engine.store().document_by_path("b.md").unwrap().unwrap();
    let c = engine.store().document_by_path("c.md").unwrap().unwrap();

    // The span projections land on kn.Artifact ids (the schema's
    // `from`/`to : ArtifactLink → Artifact` arrows), not raw document ids.
    let mut expected = vec![
        (artifact_id(a.id), artifact_id(b.id)),
        (artifact_id(b.id), artifact_id(c.id)),
    ];
    expected.sort();
    assert_eq!(span_edges(&engine), expected);

    // `related()` is the unified surface (graph + concept + vector). Its GRAPH
    // component is the depth-2 relation walk this test exercises: b at one hop
    // (0.5), c at two (0.25). Assert that component directly (the total score
    // also folds in the vector arm, which these tiny docs contribute a little
    // to) so the span-edge lifecycle below stays the contract under test.
    let related = engine.related(a.id, 10).await.unwrap();
    let graph: Vec<(&str, f32)> = related
        .iter()
        .map(|r| (r.file_path.as_str(), r.components.graph))
        .collect();
    assert_eq!(graph, vec![("b.md", 0.5), ("c.md", 0.25)]);
    // Best-first, and the span-graph edges are what put b ahead of c.
    for pair in related.windows(2) {
        assert!(pair[0].score >= pair[1].score);
    }

    // Deleting the TARGET document retracts the inbound edge (b -> c) and
    // unresolves b's link row so a reappearing target can re-resolve.
    std::fs::remove_file(root.join("c.md")).unwrap();
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        span_edges(&engine),
        vec![(artifact_id(a.id), artifact_id(b.id))]
    );
    let unresolved = engine.store().unresolved_links().unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].link.raw_target, "c.md");
    let related = engine.related(a.id, 10).await.unwrap();
    let b_related = related
        .iter()
        .find(|r| r.file_path == "b.md")
        .expect("b.md still related via the a -> b span edge");
    assert_eq!(b_related.components.graph, 0.5);
    // c.md is gone: no related row can carry its (now-retracted) 2-hop edge.
    assert!(related.iter().all(|r| r.file_path != "c.md"));

    // Editing the SOURCE section away from the link retracts a -> b, so the
    // GRAPH component of every related row drops to zero — the span edge is
    // fully gone. (b.md still exists on disk, so a residual vector-arm score
    // can keep it in the unified list; the graph edge, the thing retracted, is
    // what we assert.)
    write(&root, "a.md", "# A\n\nStart, no links now.\n");
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(span_edges(&engine), Vec::<(Uuid, Uuid)>::new());
    let related = engine.related(a.id, 10).await.unwrap();
    assert!(
        related.iter().all(|r| r.components.graph == 0.0),
        "every span edge retracted: {related:?}"
    );

    let check = engine.check_equations().await.unwrap();
    assert!(
        check.violations.is_empty(),
        "corpus must be coherent: {:?}",
        check.violations
    );
    assert!(
        check.schemas_checked > 0,
        "the equation check must have run"
    );
}

#[tokio::test]
async fn fixture_corpus_concepts_extract_and_equations_hold() {
    // The committed frontmatter fixtures: tags extract into concepts under
    // their stable ids, and the equation-check job still reports zero
    // violations over the whole corpus with the KN layer emitted.
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../fixtures/corpus");
    let db_dir = tempfile::tempdir().unwrap();
    let engine = open(&db_dir.path().join("catdb.db"));
    engine
        .import(&corpus, ImportOptions::default())
        .await
        .unwrap();

    let concepts = kn_object_ids(&engine, "kn.Concept");
    // frontmatter/typical.md carries a BLOCK-LIST `tags:` (example,
    // frontmatter); index.md carries an inline array ([hub, index]).
    for tag in ["example", "frontmatter", "hub", "index"] {
        assert!(
            concepts.contains(&concept_id(tag)),
            "fixture tag `{tag}` must extract into a concept"
        );
    }
    let counts = kn_counts(&engine);
    assert_eq!(
        counts.get("kn.Artifact").copied().unwrap_or(0),
        49,
        "every fixture document is an artifact"
    );
    assert!(counts.get("kn.ArtifactConcept").copied().unwrap_or(0) >= 4);

    let check = engine.check_equations().await.unwrap();
    assert!(
        check.violations.is_empty(),
        "corpus must be coherent: {:?}",
        check.violations
    );
    assert!(
        check.schemas_checked > 0,
        "the equation check must have run"
    );
}

#[tokio::test]
async fn artifact_link_spans_are_total_and_faithful_to_fanout() {
    // The REAL instance-level validity of the span model, on stored data:
    // load the ingested kn instance and assert (a) totality — every
    // ArtifactLink carries BOTH projections and both endpoints are stored
    // artifacts — and (b) fan-out fidelity — artifacts with 0, 1, and 3
    // outgoing links round-trip as exactly 0/1/3 ArtifactLink rows. Fan-out
    // is precisely what an `Artifact → Artifact` ARROW could not represent:
    // an arrow is a function (exactly one target per source; MemInstance's
    // set_fk overwriting duplicates is faithful to that), while a span
    // entity carries one row per link.
    use catdb_sdk::core::MemInstance;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    write(&root, "zero.md", "# Zero\n\nNo links at all.\n");
    write(&root, "one.md", "# One\n\nSee [zero](zero.md).\n");
    write(
        &root,
        "three.md",
        "# Three\n\n[a](zero.md) then [b](one.md) then [c](four.md).\n",
    );
    write(&root, "four.md", "# Four\n\nA target.\n");
    let db_dir = tempfile::tempdir().unwrap();
    let engine = open(&db_dir.path().join("catdb.db"));
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.links_resolved, 4);

    // Load the stored kn instance.
    let artifacts: std::collections::BTreeSet<Uuid> =
        kn_object_ids(&engine, "kn.Artifact").into_iter().collect();
    let spans: std::collections::BTreeSet<Uuid> = kn_object_ids(&engine, "kn.ArtifactLink")
        .into_iter()
        .collect();
    let from: BTreeMap<Uuid, Uuid> = engine
        .store()
        .morphisms_with_arrow_prefix("kn.from")
        .unwrap()
        .into_iter()
        .filter(|e| e.arrow == "kn.from")
        .map(|e| (e.source, e.target))
        .collect();
    let to: BTreeMap<Uuid, Uuid> = engine
        .store()
        .morphisms_with_arrow_prefix("kn.to")
        .unwrap()
        .into_iter()
        .filter(|e| e.arrow == "kn.to")
        .map(|e| (e.source, e.target))
        .collect();

    // (a) Totality of `from`/`to`: defined on every span, endpoints exist.
    assert_eq!(spans.len(), 4);
    for span in &spans {
        let f = from
            .get(span)
            .unwrap_or_else(|| panic!("span {span} lacks its `from` projection"));
        let t = to
            .get(span)
            .unwrap_or_else(|| panic!("span {span} lacks its `to` projection"));
        assert!(
            artifacts.contains(f),
            "`from` endpoint must be a stored artifact"
        );
        assert!(
            artifacts.contains(t),
            "`to` endpoint must be a stored artifact"
        );
    }
    // No dangling projections either (every edge belongs to a span row).
    assert!(from.keys().all(|s| spans.contains(s)));
    assert!(to.keys().all(|s| spans.contains(s)));

    // (b) Fan-out fidelity: 0 / 1 / 3 outgoing links round-trip exactly.
    let outgoing = |path: &str| {
        let doc = engine.store().document_by_path(path).unwrap().unwrap();
        let a = artifact_id(doc.id);
        from.values().filter(|src| **src == a).count()
    };
    assert_eq!(outgoing("zero.md"), 0);
    assert_eq!(outgoing("one.md"), 1);
    assert_eq!(outgoing("three.md"), 3);

    // (c) The loaded instance satisfies the kn schema's equations. NOTE:
    // that equation loop is INTENTIONALLY empty — the builtin kn schema
    // declares no path equations (none are invented just to have some);
    // totality and fan-out above are the real instance-level obligations,
    // and F_extract's prover validation lives in catdb-validation's
    // builtin tests.
    let kn = mdreader_domain::builtin::kn_schema();
    assert!(kn.equations.is_empty(), "kn is equation-free by design");
    let mut instance = MemInstance::new("kn");
    for object in engine.store().objects_with_entity_prefix("kn.").unwrap() {
        let local = &object.entity["kn.".len()..];
        if kn.entities.contains(&local.into()) {
            instance.add_row(local, object.id.to_string());
        }
    }
    for edge in engine.store().morphisms_with_arrow_prefix("kn.").unwrap() {
        let local = &edge.arrow["kn.".len()..];
        if kn.arrows.contains_key(&local.into()) {
            instance.set_fk(local, edge.source.to_string(), edge.target.to_string());
        }
    }
    assert_eq!(instance.check_equations(&kn), Vec::new());
}
