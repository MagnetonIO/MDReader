//! P3 WATCH acceptance: live watching converges the database to disk state
//! with **no manual import calls**, bursts coalesce into a single sync,
//! reconcile catches whatever a watcher missed, and a killed import
//! resumes — with `index_status` reflecting Failed then Done throughout.
//!
//! Everything here drives the real registration (markdown ingestor) with
//! the deterministic `HashEmbedder` (`--no-default-features` friendly), and
//! every watcher-triggered sync flows through the engine's serialized
//! import path — these tests never write to storage directly.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use catdb_embed::{EmbedError, EmbedResult, Embedder, HashEmbedder, ModelInfo};
use catdb_engine::{
    EmbeddedCatDb, EngineConfig, ImportOptions, SearchOptions, WatchEvent, WatchEventKind,
};
use catdb_storage::CatalogStorage;
use tokio::sync::broadcast;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn open(db: &Path) -> EmbeddedCatDb {
    EmbeddedCatDb::open(
        EngineConfig::new(db, Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration())
            .with_actor("watch-test"),
    )
    .unwrap()
}

/// Poll until `check` holds (the DB is the source of truth; watcher events
/// are advisory), failing loudly on timeout. Generous deadline: macOS
/// FSEvents delivery plus the 500ms debounce window plus an import.
async fn converge(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if check() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for convergence: {what}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn next_event(rx: &mut broadcast::Receiver<WatchEvent>) -> WatchEvent {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("timed out waiting for a watch event")
        .expect("watch event channel closed")
}

fn doc_paths(engine: &EmbeddedCatDb) -> BTreeSet<String> {
    engine
        .store()
        .list_documents()
        .unwrap()
        .into_iter()
        .map(|d| d.file_path)
        .collect()
}

const ALPHA: &str = "\
# Alpha

Alpha intro paragraph.

## Details

Alpha details paragraph.
";

const BETA: &str = "\
# Beta

See [alpha](../a.md) and [[a]].
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_converges_create_edit_rename_delete_without_manual_imports() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(open(&db_dir.path().join("catdb.db")));

    // Register the source on an EMPTY vault — every document below arrives
    // through the watcher alone.
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    let handle = engine.watch_sources().await.unwrap();
    let mut rx = handle.subscribe();
    assert_eq!(handle.watched().len(), 1);

    // Watch start = reconcile (documented guarantee).
    let first = next_event(&mut rx).await;
    assert_eq!(first.kind, WatchEventKind::Reconcile);
    assert!(first.path.is_none());
    assert!(first.result.is_ok(), "start reconcile failed: {first:?}");

    // CREATE two files; the watcher syncs them in (links resolving too).
    write(&root, "a.md", ALPHA);
    write(&root, "notes/b.md", BETA);
    converge("both created documents ingested and linked", || {
        let paths = doc_paths(&engine);
        if paths != BTreeSet::from(["a.md".to_string(), "notes/b.md".to_string()]) {
            return false;
        }
        engine.store().unresolved_links().unwrap().is_empty()
    })
    .await;

    // Exact converged state after CREATE: documents, chunks, links.
    let a = engine.store().document_by_path("a.md").unwrap().unwrap();
    let b = engine
        .store()
        .document_by_path("notes/b.md")
        .unwrap()
        .unwrap();
    assert_eq!(engine.store().chunks_for_document(a.id).unwrap().len(), 2);
    let b_chunks = engine.store().chunks_for_document(b.id).unwrap();
    assert_eq!(b_chunks.len(), 1);
    let b_sections = engine.store().sections_for_document(b.id).unwrap();
    let b_links = engine
        .store()
        .links_for_sections(&b_sections.iter().map(|s| s.id).collect::<Vec<_>>())
        .unwrap();
    let targets: BTreeSet<(String, Option<uuid::Uuid>)> = b_links
        .iter()
        .map(|l| (l.raw_target.clone(), l.resolved_document))
        .collect();
    assert_eq!(
        targets,
        BTreeSet::from([
            ("../a.md".to_string(), Some(a.id)),
            ("a".to_string(), Some(a.id)),
        ]),
        "both of b's links resolve to a"
    );

    // EDIT one section of a.md; the watcher re-syncs just that file.
    write(
        &root,
        "a.md",
        &ALPHA.replace("details paragraph.", "details paragraph, revised."),
    );
    converge("edited section re-chunked", || {
        engine
            .store()
            .chunks_for_document(a.id)
            .unwrap()
            .iter()
            .any(|c| c.text.contains("revised"))
    })
    .await;

    // RENAME b.md (same content, new path).
    std::fs::rename(root.join("notes/b.md"), root.join("notes/c.md")).unwrap();
    converge("rename observed", || {
        doc_paths(&engine) == BTreeSet::from(["a.md".to_string(), "notes/c.md".to_string()])
    })
    .await;
    let c = engine
        .store()
        .document_by_path("notes/c.md")
        .unwrap()
        .unwrap();
    assert_eq!(c.id, b.id, "a rename keeps the document identity");

    // DELETE a.md; c's links to it unresolve (FK SET NULL) and its rows go.
    std::fs::remove_file(root.join("a.md")).unwrap();
    converge("deletion observed", || {
        doc_paths(&engine) == BTreeSet::from(["notes/c.md".to_string()])
    })
    .await;

    // Exact final state: one document, one chunk, two now-unresolved links.
    let unresolved = engine.store().unresolved_links().unwrap();
    let raw: BTreeSet<String> = unresolved
        .iter()
        .map(|u| u.link.raw_target.clone())
        .collect();
    assert_eq!(
        raw,
        BTreeSet::from(["../a.md".to_string(), "a".to_string()])
    );
    let status = engine.status().await.unwrap();
    assert_eq!(status.documents, 1);
    assert_eq!(status.chunks, 1);
    assert_eq!(status.links, 2);

    // Deleted content is not searchable; surviving content is.
    let hits = engine
        .search("Alpha details", SearchOptions::default())
        .await
        .unwrap();
    assert!(hits.iter().all(|h| h.file_path != "a.md"));

    // The op log stayed coherent through every watcher-driven mutation.
    let check = engine.check_equations().await.unwrap();
    assert_eq!(check.violations, Vec::<String>::new());

    // Stop: the handle winds the watcher down; further disk changes are
    // (deliberately) not observed until the next reconcile.
    assert!(handle.stop(handle.watched()[0]));
    assert!(handle.watched().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rapid_writes_to_one_file_coalesce_into_a_single_sync() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    write(&root, "burst.md", "# Burst\n\ndraft 0\n");
    let db_dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(open(&db_dir.path().join("catdb.db")));
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();

    let handle = engine.watch_sources().await.unwrap();
    let mut rx = handle.subscribe();
    let first = next_event(&mut rx).await;
    assert_eq!(first.kind, WatchEventKind::Reconcile);
    let baseline = handle.syncs_completed();
    assert_eq!(baseline, 1, "only the watch-start reconcile has run");

    // A burst of rapid successive writes to ONE file, well inside the
    // 500ms debounce window.
    for i in 1..=5 {
        write(&root, "burst.md", &format!("# Burst\n\ndraft {i}\n"));
    }
    converge("final draft ingested", || {
        let doc = engine
            .store()
            .document_by_path("burst.md")
            .unwrap()
            .unwrap();
        engine
            .store()
            .chunks_for_document(doc.id)
            .unwrap()
            .iter()
            .any(|c| c.text.contains("draft 5"))
    })
    .await;

    // Let any trailing debounce window flush, then assert the whole burst
    // was serviced by EXACTLY one sync.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        handle.syncs_completed() - baseline,
        1,
        "a coalesced burst must trigger exactly one sync"
    );
}

#[tokio::test]
async fn reconcile_converges_changes_made_while_nothing_watched() {
    // The documented guarantee: whatever a watcher missed (here: no watcher
    // at all) converges at the next reconcile — no manual import options,
    // no watcher required.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    write(&root, "a.md", ALPHA);
    let db_dir = tempfile::tempdir().unwrap();
    let engine = open(&db_dir.path().join("catdb.db"));
    engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();

    // Offline drift: create, edit, rename, delete — all unobserved.
    write(&root, "new.md", "# New\n\nCreated while unwatched.\n");
    write(
        &root,
        "a.md",
        &ALPHA.replace("intro", "intro (edited offline)"),
    );

    let sources = engine.store().list_sources().await.unwrap();
    let report = engine.reconcile(sources[0].source_id).await.unwrap();
    assert_eq!(report.files_scanned, 2);

    assert_eq!(
        doc_paths(&engine),
        BTreeSet::from(["a.md".to_string(), "new.md".to_string()])
    );
    let a = engine.store().document_by_path("a.md").unwrap().unwrap();
    assert!(engine
        .store()
        .chunks_for_document(a.id)
        .unwrap()
        .iter()
        .any(|c| c.text.contains("edited offline")));
}

/// Same model info as [`HashEmbedder`] but fails every embed call:
/// simulates a process dying mid-import, after rows but before vectors.
struct FailingEmbedder {
    inner: HashEmbedder,
}

#[async_trait::async_trait]
impl Embedder for FailingEmbedder {
    fn info(&self) -> ModelInfo {
        self.inner.info()
    }

    async fn embed(&self, _texts: &[String]) -> EmbedResult<Vec<Vec<f32>>> {
        Err(EmbedError::Embed("injected kill".to_string()))
    }
}

#[tokio::test]
async fn killed_import_resumes_and_status_reads_failed_then_done() {
    // Kill-and-resume: an import aborted mid-run (injected failure in the
    // embedding phase — sections/chunks already written, vectors not),
    // then a reopen; the next import completes, and index_status reflects
    // Failed, then Done, purely from DB state.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("vault");
    write(&root, "a.md", ALPHA);
    write(&root, "b.md", BETA);
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("catdb.db");

    {
        let dying = EmbeddedCatDb::open(
            EngineConfig::new(
                &db,
                Arc::new(FailingEmbedder {
                    inner: HashEmbedder::new(),
                }),
            )
            .with_domain(mdreader_ingest::registration())
            .with_actor("watch-test"),
        )
        .unwrap();
        dying
            .import(&root, ImportOptions::default())
            .await
            .expect_err("the injected failure must abort the import");
        let status = dying.index_status().await.unwrap();
        assert_eq!(status.phase, "failed");
        assert!(
            status
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("injected kill"),
            "status must carry the failure: {status:?}"
        );
        // The kill hit the embedding phase: file syncs were already
        // reported (progress is real, not cosmetic).
        assert_eq!(status.files_scanned, 2);
        assert_eq!(status.files_done, 2);
    }

    // Reopen (fresh process) and import with a working embedder: the
    // pending-marker machinery re-processes everything; status flips Done.
    let engine = open(&db);
    let report = engine
        .import(&root, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        report.files_skipped_unchanged, 0,
        "nothing may short-circuit"
    );
    assert_eq!(report.chunks_embedded, 3);

    let status = engine.index_status().await.unwrap();
    assert_eq!(status.phase, "done", "the NEWEST job wins: {status:?}");
    assert_eq!(status.files_scanned, 2);
    assert_eq!(status.chunks_embedded, 3);
    assert_eq!(status.chunks_total, Some(3));

    // Both terminal states remain visible in the durable job history.
    let jobs = engine.store().list_jobs().await.unwrap();
    let ingest: Vec<_> = jobs.iter().filter(|j| j.kind == "ingest").collect();
    assert_eq!(ingest.len(), 2);
    assert!(ingest
        .iter()
        .any(|j| j.status == catdb_catalog::JobStatus::Failed));
    assert!(ingest
        .iter()
        .any(|j| j.status == catdb_catalog::JobStatus::Done));

    // And the resumed corpus actually serves search with vectors.
    let hits = engine
        .search("Alpha details", SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(hits[0].file_path, "a.md");
    assert!(hits[0].sem_rank.is_some(), "vectors exist after the resume");
}
