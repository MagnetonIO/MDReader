//! KN-layer acceptance: frontmatter `tags:` become `kn.Concept` /
//! `kn.ArtifactConcept` facts, documents become `kn.Artifact`s, resolved
//! links assert `kn.links_to` morphisms between documents that
//! `relation_search` traverses (graph test to depth 2), and every derived
//! fact is retracted when its source dies — with the instance-level
//! equation check clean throughout (fixtures included).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions};
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
    for (_, entity) in engine.store().objects_with_entity_prefix("kn.").unwrap() {
        *counts.entry(entity).or_insert(0) += 1;
    }
    counts
}

fn kn_object_ids(engine: &EmbeddedCatDb, entity: &str) -> Vec<Uuid> {
    engine
        .store()
        .objects_with_entity_prefix("kn.")
        .unwrap()
        .into_iter()
        .filter(|(_, e)| e == entity)
        .map(|(id, _)| id)
        .collect()
}

fn links_to_edges(engine: &EmbeddedCatDb) -> Vec<(Uuid, Uuid)> {
    let mut edges: Vec<(Uuid, Uuid)> = engine
        .store()
        .morphisms_with_arrow_prefix("kn.links_to")
        .unwrap()
        .into_iter()
        .map(|e| (e.source, e.target))
        .collect();
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
    assert_eq!(check.violations, Vec::<String>::new());
}

#[tokio::test]
async fn resolved_links_assert_links_to_morphisms_that_related_traverses() {
    // Graph a -> b -> c, built purely from markdown links: resolution must
    // assert kn.links_to morphisms between DOCUMENTS, `related` must walk
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

    let mut expected = vec![(a.id, b.id), (b.id, c.id)];
    expected.sort();
    assert_eq!(links_to_edges(&engine), expected);

    // Depth-2 relation walk from a: b at one hop (0.5), c at two (0.25).
    let related = engine.related(a.id, 10).await.unwrap();
    let got: Vec<(&str, f32)> = related
        .iter()
        .map(|r| (r.file_path.as_str(), r.score))
        .collect();
    assert_eq!(got, vec![("b.md", 0.5), ("c.md", 0.25)]);

    // Deleting the TARGET document retracts the inbound edge (b -> c) and
    // unresolves b's link row so a reappearing target can re-resolve.
    std::fs::remove_file(root.join("c.md")).unwrap();
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(links_to_edges(&engine), vec![(a.id, b.id)]);
    let unresolved = engine.store().unresolved_links().unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].link.raw_target, "c.md");
    let related = engine.related(a.id, 10).await.unwrap();
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].file_path, "b.md");

    // Editing the SOURCE section away from the link retracts a -> b.
    write(&root, "a.md", "# A\n\nStart, no links now.\n");
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(links_to_edges(&engine), Vec::<(Uuid, Uuid)>::new());
    assert!(engine.related(a.id, 10).await.unwrap().is_empty());

    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());
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
        45,
        "every fixture document is an artifact"
    );
    assert!(counts.get("kn.ArtifactConcept").copied().unwrap_or(0) >= 4);

    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());
}
