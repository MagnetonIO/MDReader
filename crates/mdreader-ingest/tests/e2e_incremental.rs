//! End-to-end acceptance tests for the vertical slice: import a corpus into
//! an [`EmbeddedCatDb`] through the real registration, then exercise the
//! incremental contract (edit one section / rename / delete), hybrid search
//! provenance, link resolution, and the instance-level equation check.
//!
//! Uses the deterministic `HashEmbedder` (`--no-default-features` friendly).

use std::path::Path;
use std::sync::Arc;

use catdb_embed::{EmbedError, EmbedResult, Embedder, HashEmbedder, ModelInfo};
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions, SearchOptions};

/// An injectable embedder that reports the same model info as
/// [`HashEmbedder`] (so a retry with the real one matches the stored index
/// version) but fails every embed call — simulating an import that dies
/// after its rows are committed but before its vectors are.
struct FailingEmbedder {
    inner: HashEmbedder,
}

#[async_trait::async_trait]
impl Embedder for FailingEmbedder {
    fn info(&self) -> ModelInfo {
        self.inner.info()
    }

    async fn embed(&self, _texts: &[String]) -> EmbedResult<Vec<Vec<f32>>> {
        Err(EmbedError::Embed("injected embedding failure".to_string()))
    }
}

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
async fn failed_embedding_leaves_files_reprocessable() {
    // The failure-atomicity invariant: a file's stored content_hash may only
    // advance once its sections, chunks, AND embeddings are durably
    // committed. An embedder that fails mid-import must leave a Failed job
    // and fully re-processable files — never a poisoned "unchanged" state.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    corpus(&root);
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("catdb.db");

    {
        let failing = EmbeddedCatDb::open(
            EngineConfig::new(
                &db,
                Arc::new(FailingEmbedder {
                    inner: HashEmbedder::new(),
                }),
            )
            .with_domain(mdreader_ingest::registration())
            .with_actor("e2e-test"),
        )
        .unwrap();
        let err = failing
            .import(&root, ImportOptions::default())
            .await
            .expect_err("import must fail when embedding fails");
        assert!(
            err.to_string().contains("injected embedding failure"),
            "unexpected error: {err}"
        );
        // The failure is recorded as a Failed ingest job.
        use catdb_storage::CatalogStorage;
        let jobs = failing.store().list_jobs().await.unwrap();
        let ingest_jobs: Vec<_> = jobs.iter().filter(|j| j.kind == "ingest").collect();
        assert_eq!(ingest_jobs.len(), 1);
        assert_eq!(ingest_jobs[0].status, catdb_catalog::JobStatus::Failed);
    }

    // Retry with a working embedder: every file re-processes (nothing
    // short-circuits on the poisoned state) and the repair is complete.
    let engine = open(&db);
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        report.files_skipped_unchanged, 0,
        "failed files must not be skipped as unchanged"
    );
    assert_eq!(
        report.sections_upserted, 6,
        "the retry re-processes all sections"
    );
    assert_eq!(report.chunks_embedded, 6, "the retry re-embeds all chunks");

    // Correct chunks (no duplicates from the repair) and correct vectors
    // (the semantic modality ranks the repaired chunks).
    let status = engine.status().await.unwrap();
    assert_eq!(status.documents, 3);
    assert_eq!(status.chunks, 6);
    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(hits[0].file_path, "notes/graph.md");
    assert!(
        hits[0].sem_rank.is_some(),
        "vectors must exist after repair"
    );

    // The corpus is now stable: a further import short-circuits everything.
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_skipped_unchanged, 3);
    assert_eq!(report.chunks_embedded, 0);

    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());
}

#[tokio::test]
async fn deleting_a_nested_repo_directory_cleans_up_its_documents() {
    // Deletion is scoped against stored state under the source root, so a
    // fully deleted nested git repo (its repo identity absent from the new
    // scan) must still have its documents, chunks, and links cleaned up.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    corpus(&root);
    std::fs::create_dir_all(root.join("nested/proj/.git")).unwrap();
    write(
        &root,
        "nested/proj/readme.md",
        "# Nested\n\nSee the [guide](../../guide.md).\n",
    );

    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("catdb.db");
    let engine = open(&db);
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_scanned, 4);
    assert_eq!(report.links_resolved, 3);
    let status = engine.status().await.unwrap();
    assert_eq!(status.documents, 4);

    // Delete the whole nested repo directory (its last markdown file goes
    // with it, and so does its repo identity).
    std::fs::remove_dir_all(root.join("nested")).unwrap();
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.files_deleted, 1);

    let status = engine.status().await.unwrap();
    assert_eq!(status.documents, 3, "the nested document is gone");
    let hits = engine
        .search("nested", SearchOptions::default())
        .await
        .unwrap();
    assert!(
        hits.iter().all(|h| h.file_path != "nested/proj/readme.md"),
        "deleted nested-repo document must not be searchable"
    );
    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());
}

#[tokio::test]
async fn chunker_version_bump_reprocesses_unchanged_files() {
    // A chunker-version change must rotate to a new index version AND bypass
    // the file-hash short-circuit: every file re-chunks and re-embeds even
    // though its content hash is unchanged.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    corpus(&root);
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("catdb.db");

    {
        let engine = open(&db);
        let report = engine
            .import(&root, ImportOptions::default())
            .await
            .unwrap();
        assert_eq!(report.chunks_embedded, 6);
    }

    // Re-open with a bumped chunker version (same embedder model).
    let mut registration = mdreader_ingest::registration();
    let bumped = format!("{}+test-bump", mdreader_ingest::CHUNKER_VERSION);
    registration.chunker_version = bumped.clone();
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(&db, Arc::new(HashEmbedder::new()))
            .with_domain(registration)
            .with_actor("e2e-test"),
    )
    .unwrap();
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        report.files_skipped_unchanged, 0,
        "unchanged files must not short-circuit on a chunker bump"
    );
    assert_eq!(report.sections_upserted, 6);
    assert_eq!(report.chunks_embedded, 6, "everything re-embeds");

    // The old index version is retired; the new active one carries the
    // bumped chunker version and serves search.
    use catdb_storage::CatalogStorage;
    let versions = engine.store().list_index_versions().await.unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions.iter().filter(|v| v.active).count(), 1);
    let status = engine.status().await.unwrap();
    assert_eq!(status.active_index.unwrap().chunker_version, bumped);
    assert_eq!(status.chunks, 6, "no duplicate chunks from the rebuild");
    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(hits[0].file_path, "notes/graph.md");
    assert!(hits[0].sem_rank.is_some(), "new index version has vectors");
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
    // Exact values for the committed fixture corpus: any change to the
    // corpus or the chunker shows up here deliberately (and the latter
    // requires a CHUNKER_VERSION bump).
    assert_eq!(report.files_scanned, 45);
    assert_eq!(report.chunks_embedded, 3117);
    assert_eq!(report.links_resolved, 107);

    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());

    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert!(!hits.is_empty());
    assert!(hits[0].start_line >= 1);
    assert!(!hits[0].heading_path.is_empty() || hits[0].file_path.ends_with(".md"));

    // A lexical exact-phrase query lands on the exact file that contains the
    // phrase (deterministic under the HashEmbedder: the quote is verbatim
    // from index.md's "Also see" section).
    let hits = engine
        .search(
            "\"deliberately small but shaped like a real vault\"",
            SearchOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(hits[0].file_path, "index.md");
    assert_eq!(hits[0].lex_rank, Some(1));

    // Incremental over the fixtures: nothing changed, nothing re-embedded.
    let report = engine
        .import(&corpus, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report.chunks_embedded, 0);
    assert_eq!(report.files_skipped_unchanged, report.files_scanned);
}
