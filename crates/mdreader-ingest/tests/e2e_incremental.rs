//! End-to-end acceptance tests for the vertical slice: import a corpus into
//! an [`EmbeddedCatDb`] through the real registration, then exercise the
//! incremental contract (edit one section / rename / delete), hybrid search
//! provenance, link resolution, and the instance-level equation check.
//!
//! Uses the deterministic `HashEmbedder` (`--no-default-features` friendly).

use std::path::Path;
use std::sync::Arc;

use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions, SearchOptions};

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn open(db: &Path) -> EmbeddedCatDb {
    EmbeddedCatDb::open(
        EngineConfig::new(db, Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration())
            .with_actor("e2e-test"),
    )
    .unwrap()
}

const GUIDE: &str = "\
# Search Guide

Introductory prose about retrieval.

## Lexical

BM25 full-text search over chunk text.

## Semantic

Vector similarity over embedded chunks.

## Fusion

Reciprocal rank fusion combines both result lists.
";

fn corpus(root: &Path) {
    write(root, "guide.md", GUIDE);
    write(
        root,
        "notes/links.md",
        "# Links\n\nSee the [guide](../guide.md) and the [[graph]] note.\n",
    );
    write(
        root,
        "notes/graph.md",
        "# Graph\n\nThe wikilink graph connects zettel notes.\n",
    );
}

#[tokio::test]
async fn import_edit_rename_delete_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    corpus(&root);
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("catdb.db");
    let engine = open(&db);

    // Initial import: 3 files, 6 sections (guide 4 + links 1 + graph 1),
    // one chunk each, both links resolved.
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_scanned, 3);
    assert_eq!(report.files_skipped_unchanged, 0);
    assert_eq!(report.sections_upserted, 6);
    assert_eq!(report.chunks_embedded, 6);
    assert_eq!(report.links_resolved, 2);

    // Hybrid search with provenance.
    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(hits[0].file_path, "notes/graph.md");
    assert_eq!(hits[0].heading_path, vec!["Graph".to_string()]);
    assert!(hits[0].start_line >= 1 && hits[0].end_line >= hits[0].start_line);

    // Unchanged re-import: everything short-circuits.
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_skipped_unchanged, 3);
    assert_eq!(report.sections_upserted, 0);
    assert_eq!(report.chunks_embedded, 0);
    assert_eq!(report.files_renamed, 0);
    assert_eq!(report.files_deleted, 0);

    // Edit ONE section (Semantic) of the multi-section guide: exactly that
    // section's chunks re-embed; the other five sections stay untouched.
    write(
        &root,
        "guide.md",
        &GUIDE.replace(
            "Vector similarity over embedded chunks.",
            "Vector similarity over embedded chunks, now with reranking.",
        ),
    );
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_scanned, 3);
    assert_eq!(report.files_skipped_unchanged, 2);
    assert_eq!(report.sections_upserted, 1, "only the edited section");
    assert_eq!(report.chunks_embedded, 1, "only the edited section's chunk");
    assert_eq!(report.files_deleted, 0);

    // Rename: same content, new path → path-only update, zero re-embeds.
    std::fs::rename(
        root.join("notes/graph.md"),
        root.join("notes/graph-view.md"),
    )
    .unwrap();
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_renamed, 1);
    assert_eq!(report.chunks_embedded, 0, "renames must not re-embed");
    assert_eq!(report.files_deleted, 0);
    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(hits[0].file_path, "notes/graph-view.md");

    // Delete: the file's chunks vanish from search.
    std::fs::remove_file(root.join("notes/graph-view.md")).unwrap();
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_deleted, 1);
    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert!(
        hits.iter().all(|h| h.file_path != "notes/graph-view.md"),
        "deleted document must not be searchable"
    );

    // The operation log mirrors the corpus coherently at every step.
    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());

    let status = engine.status().await.unwrap();
    assert_eq!(status.documents, 2);
    assert_eq!(status.sources.len(), 1);
    assert!(status.active_index.is_some());
}

#[tokio::test]
async fn fixtures_corpus_imports_cleanly() {
    // The real fixture corpus (45 markdown files): a full import must
    // succeed, resolve links, satisfy the md schema equations, and answer a
    // search with correct provenance.
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../fixtures/corpus");
    let db_dir = tempfile::tempdir().unwrap();
    let engine = open(&db_dir.path().join("catdb.db"));

    let report = engine
        .import(&corpus, ImportOptions::default())
        .await
        .unwrap();
    assert!(
        report.files_scanned >= 40,
        "scanned {}",
        report.files_scanned
    );
    assert!(report.chunks_embedded > 0);
    assert!(
        report.links_resolved >= 20,
        "resolved {}",
        report.links_resolved
    );

    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());

    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert!(!hits.is_empty());
    assert!(hits[0].start_line >= 1);
    assert!(!hits[0].heading_path.is_empty() || hits[0].file_path.ends_with(".md"));

    // Incremental over the fixtures: nothing changed, nothing re-embedded.
    let report = engine
        .import(&corpus, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.chunks_embedded, 0);
    assert_eq!(report.files_skipped_unchanged, report.files_scanned);
}
