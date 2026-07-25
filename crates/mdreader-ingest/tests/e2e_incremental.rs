//! End-to-end acceptance tests for the vertical slice: import a corpus into
//! an [`EmbeddedCatDb`] through the real registration, then exercise the
//! incremental contract (edit one section / rename / delete), hybrid search
//! provenance, link resolution, and the instance-level equation check.
//!
//! Uses the deterministic `HashEmbedder` (`--no-default-features` friendly).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use catdb_embed::{EmbedError, EmbedResult, Embedder, HashEmbedder, ModelInfo};
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions, SearchOptions};
use catdb_storage::{CatalogStorage, LexicalQuery, QueryFilters, SemanticStore, VectorQuery};
use uuid::Uuid;

/// Item-4 invariant: every `rx.Chunk` object in the operation-log's
/// materialized view must correspond to a stored chunk row — a mid-sync
/// crash plus repair must never strand chunk facts without rows (or,
/// worse, rows without facts that a later retraction can no longer find).
fn assert_no_orphan_chunk_objects(engine: &EmbeddedCatDb) {
    let objects = engine
        .store()
        .objects_with_entity_prefix("rx.Chunk")
        .unwrap();
    let ids: Vec<Uuid> = objects.iter().map(|object| object.id).collect();
    let rows = engine.store().chunks_by_ids(&ids).unwrap();
    assert_eq!(
        rows.len(),
        ids.len(),
        "every rx.Chunk object must have a chunk row (objects: {}, rows: {})",
        ids.len(),
        rows.len()
    );
}

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
    assert!(
        check.violations.is_empty(),
        "corpus must be coherent: {:?}",
        check.violations
    );
    assert!(
        check.schemas_checked > 0,
        "the equation check must have run"
    );

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
        let jobs = failing.store().list_jobs().await.unwrap();
        let ingest_jobs: Vec<_> = jobs.iter().filter(|j| j.kind == "ingest").collect();
        assert_eq!(ingest_jobs.len(), 1);
        assert_eq!(ingest_jobs[0].status, catdb_catalog::JobStatus::Failed);
        // Even mid-crash, the op log and the chunk rows agree (each file's
        // sync is one transaction).
        assert_no_orphan_chunk_objects(&failing);
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
    assert!(
        check.violations.is_empty(),
        "corpus must be coherent: {:?}",
        check.violations
    );
    assert!(
        check.schemas_checked > 0,
        "the equation check must have run"
    );
    // After failure + repair: no chunk facts without rows, no leftovers.
    assert_no_orphan_chunk_objects(&engine);
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
    let versions = engine.store().list_index_versions().await.unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions.iter().filter(|v| v.is_active()).count(), 1);
    assert_eq!(
        versions
            .iter()
            .filter(|v| v.state == catdb_catalog::IndexState::Retired)
            .count(),
        1,
        "the displaced version is retired (terminal), not merely inactive"
    );
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
    // The real fixture corpus (49 markdown files): a full import must
    // succeed, resolve links, satisfy the md schema equations, and answer a
    // search with correct provenance.
    //
    // `docs/arch/` was added for the GUI harness's semantic steps and holds a
    // deliberately INCONSISTENT architecture (`broken-app` depends on an
    // interface another component exposes). That does not make the corpus
    // incoherent here: the equation check below reads the COMMITTED graph, this
    // engine is opened without a proposal store, and so no overlay fact is ever
    // extracted or approved into it. The violation only exists once a human
    // approves the claims — which is exactly what the GUI harness seeds.
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
    assert_eq!(report.files_scanned, 49);
    assert_eq!(report.chunks_embedded, 3130);
    assert_eq!(report.links_resolved, 110);

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

#[tokio::test]
async fn force_rechunk_with_new_boundaries_sweeps_stale_chunk_rows() {
    // A REAL chunk-boundary change: the same file re-chunked with a smaller
    // split threshold (a genuine chunker change, carried by a distinct
    // chunker version). Section hashes are untouched — only the chunk SET
    // changes — so this exercises exactly the force-rechunk path: the old
    // rows' ids match nothing in the new set and must be deleted (rows, FTS
    // entries, vectors), not merely have their objects retracted.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    // One section, long enough that a 64-token limit splits it into several
    // chunks while the default limit keeps it whole. The marker word
    // appears throughout so every boundary regime indexes it.
    let paragraphs: Vec<String> = (0..12)
        .map(|i| {
            format!(
                "Paragraph {i} about the zebrachunk experiment: retrieval quality \
                 depends on stable boundaries, and stale rows must never linger \
                 in the searchable index once boundaries move."
            )
        })
        .collect();
    write(
        &root,
        "long.md",
        &format!("# Long\n\n{}\n", paragraphs.join("\n\n")),
    );
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("catdb.db");

    let (old_chunk_ids, old_version_id) = {
        let engine = open(&db);
        engine
            .import(&root, ImportOptions::default())
            .await
            .unwrap();
        let doc = engine
            .store()
            .document_by_path("long.md")
            .unwrap()
            .expect("document imported");
        let chunks = engine.store().chunks_for_document(doc.id).unwrap();
        assert_eq!(
            chunks.len(),
            1,
            "the default limit must keep the section in one chunk"
        );
        let active = engine.store().active_index_version().await.unwrap();
        (
            chunks.iter().map(|c| c.id).collect::<Vec<Uuid>>(),
            active.unwrap().id,
        )
    };

    // Re-open with a narrower chunker (and, as any real chunker change
    // must, a distinct chunker version) — the import force-rebuilds.
    let mut registration = mdreader_ingest::registration();
    registration.ingestors = vec![Arc::new(
        mdreader_ingest::MarkdownIngestor::with_chunk_token_limit(64),
    )];
    registration.chunker_version = format!("{}+narrow64", mdreader_ingest::CHUNKER_VERSION);
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
    assert_eq!(report.files_skipped_unchanged, 0);

    let doc = engine.store().document_by_path("long.md").unwrap().unwrap();
    let new_chunks = engine.store().chunks_for_document(doc.id).unwrap();
    let new_ids: HashSet<Uuid> = new_chunks.iter().map(|c| c.id).collect();
    assert!(
        new_chunks.len() > 1,
        "the narrow limit must split the section (got {})",
        new_chunks.len()
    );
    for old_id in &old_chunk_ids {
        assert!(
            !new_ids.contains(old_id),
            "boundary change must produce new chunk ids"
        );
    }

    // The old rows are GONE — not present, not lexically searchable, and
    // their vectors purged from every index version's table.
    assert!(
        engine
            .store()
            .chunks_by_ids(&old_chunk_ids)
            .unwrap()
            .is_empty(),
        "old chunk rows must be deleted"
    );
    let hits = engine
        .store()
        .lexical_search(LexicalQuery {
            text: "zebrachunk".to_string(),
            limit: 50,
            filters: QueryFilters::default(),
        })
        .await
        .unwrap();
    assert!(!hits.is_empty(), "the content is still searchable");
    for hit in &hits {
        assert!(
            new_ids.contains(&hit.id),
            "FTS must only surface the new-boundary chunks, found stale {}",
            hit.id
        );
    }
    // The old version is retired by the rotation: the state machine makes
    // its vectors — stale boundaries included — categorically unreachable.
    let query_vec = HashEmbedder::new()
        .embed(&[new_chunks[0].embedding_text.clone()])
        .await
        .unwrap()
        .remove(0);
    let err = engine
        .store()
        .vector_search(VectorQuery {
            index_version: old_version_id,
            embedding: query_vec,
            limit: 50,
            filters: QueryFilters::default(),
        })
        .await
        .expect_err("a retired version must not be servable");
    assert!(
        err.to_string().contains("not servable"),
        "unexpected error: {err}"
    );

    // End-to-end search serves only new-boundary chunks, and the op log
    // agrees with the rows.
    let hits = engine
        .search("zebrachunk experiment", SearchOptions::default())
        .await
        .unwrap();
    assert!(!hits.is_empty());
    assert!(new_ids.contains(&hits[0].chunk_id));
    assert_no_orphan_chunk_objects(&engine);
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
async fn concurrent_imports_serialize_without_resurrecting_deletions() {
    // The single-writer model, end to end: two concurrent import() calls on
    // one engine serialize (the second waits), so a deletion observed by
    // one pass can never be resurrected by a stale concurrent scan.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    corpus(&root);
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("catdb.db");
    let engine = open(&db);
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();

    std::fs::remove_file(root.join("notes/graph.md")).unwrap();
    let (a, b) = tokio::join!(
        engine.import(&root, ImportOptions::default()),
        engine.import(&root, ImportOptions::default()),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        a.files_deleted + b.files_deleted,
        1,
        "exactly one serialized pass performs the deletion"
    );

    let status = engine.status().await.unwrap();
    assert_eq!(status.documents, 2, "the deleted document stays deleted");
    assert!(engine
        .store()
        .document_by_path("notes/graph.md")
        .unwrap()
        .is_none());
    let hits = engine
        .search("wikilink graph", SearchOptions::default())
        .await
        .unwrap();
    assert!(
        hits.iter().all(|h| h.file_path != "notes/graph.md"),
        "a concurrently deleted document must not resurface in search"
    );
    assert_no_orphan_chunk_objects(&engine);
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
