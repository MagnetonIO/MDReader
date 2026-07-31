//! Multi-source correctness: two sources in one database. Import,
//! reconcile, and watch each converge exactly their OWN document set —
//! cross-source documents are untouched by any pass (the v5 `source_id`
//! scoping), same-named relative paths coexist, link resolution never
//! leaks across sources, and deleting a whole source's content removes
//! only that source's documents.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use catdb_sdk::embed::HashEmbedder;
use catdb_sdk::engine::{EmbeddedCatDb, EngineConfig, ImportOptions};
use catdb_sdk::storage::CatalogStorage;
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
            .with_actor("multi-source-test"),
    )
    .unwrap()
}

/// The source id registered for a root (roots are stored canonicalized).
async fn source_id(engine: &EmbeddedCatDb, root: &Path) -> Uuid {
    let canonical = root.canonicalize().unwrap().display().to_string();
    engine
        .store()
        .list_sources()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.root_path == canonical)
        .unwrap_or_else(|| panic!("no source registered for {canonical}"))
        .source_id
}

fn paths_for(engine: &EmbeddedCatDb, source: Uuid) -> BTreeSet<String> {
    engine
        .store()
        .documents_for_source(source)
        .unwrap()
        .into_iter()
        .map(|d| d.file_path)
        .collect()
}

#[tokio::test]
async fn imports_and_reconciles_stay_scoped_to_their_source() {
    let dir = tempfile::tempdir().unwrap();
    let root_a = dir.path().join("vault-a");
    let root_b = dir.path().join("vault-b");
    // Same relative path in BOTH sources — they must coexist.
    write(&root_a, "index.md", "# A Index\n\nAlpha index.\n");
    // A wikilink whose stem exists ONLY in source B: resolution must NOT
    // leak across sources.
    write(&root_a, "linker.md", "# Linker\n\nSee [[b-note]].\n");
    write(&root_b, "index.md", "# B Index\n\nBeta index.\n");
    write(&root_b, "b-note.md", "# B Note\n\nBeta note.\n");

    let db_dir = tempfile::tempdir().unwrap();
    let engine = open(&db_dir.path().join("catdb.db"));
    let report_a = engine
        .import(&root_a, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report_a.files_scanned, 2);
    let report_b = engine
        .import(&root_b, ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(report_b.files_scanned, 2);
    assert_eq!(
        report_b.files_deleted, 0,
        "importing B must never delete A's documents"
    );

    let src_a = source_id(&engine, &root_a).await;
    let src_b = source_id(&engine, &root_b).await;
    assert_ne!(src_a, src_b);
    assert_eq!(
        paths_for(&engine, src_a),
        BTreeSet::from(["index.md".to_string(), "linker.md".to_string()])
    );
    assert_eq!(
        paths_for(&engine, src_b),
        BTreeSet::from(["b-note.md".to_string(), "index.md".to_string()])
    );

    // Cross-source link isolation: A's [[b-note]] stays unresolved even
    // though B has b-note.md.
    let unresolved = engine.store().unresolved_links().unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].link.raw_target, "b-note");
    assert_eq!(unresolved[0].source_path, "linker.md");

    // Reconciling A is a no-op for B (and vice versa).
    let report = engine.reconcile(src_a).await.unwrap();
    assert_eq!(report.files_deleted, 0);
    assert_eq!(report.files_scanned, 2);
    let report = engine.reconcile(src_b).await.unwrap();
    assert_eq!(report.files_deleted, 0);
    let status = engine.status().await.unwrap();
    assert_eq!(status.documents, 4);

    // Deleting one file in B: exactly that document goes; A untouched.
    std::fs::remove_file(root_b.join("b-note.md")).unwrap();
    let report = engine.reconcile(src_b).await.unwrap();
    assert_eq!(report.files_deleted, 1);
    assert_eq!(
        paths_for(&engine, src_a),
        BTreeSet::from(["index.md".to_string(), "linker.md".to_string()]),
        "A's documents survive B's deletion pass"
    );
    assert_eq!(
        paths_for(&engine, src_b),
        BTreeSet::from(["index.md".to_string()])
    );

    // Emptying source A entirely removes ONLY A's documents.
    std::fs::remove_file(root_a.join("index.md")).unwrap();
    std::fs::remove_file(root_a.join("linker.md")).unwrap();
    let report = engine.reconcile(src_a).await.unwrap();
    assert_eq!(report.files_scanned, 0);
    assert_eq!(report.files_deleted, 2);
    assert!(paths_for(&engine, src_a).is_empty());
    assert_eq!(
        paths_for(&engine, src_b),
        BTreeSet::from(["index.md".to_string()]),
        "B's whole set survives A's total deletion"
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watch_converges_each_source_independently() {
    let dir = tempfile::tempdir().unwrap();
    let root_a = dir.path().join("vault-a");
    let root_b = dir.path().join("vault-b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(open(&db_dir.path().join("catdb.db")));
    engine
        .import(&root_a, ImportOptions::default())
        .await
        .unwrap();
    engine
        .import(&root_b, ImportOptions::default())
        .await
        .unwrap();
    let src_a = source_id(&engine, &root_a).await;
    let src_b = source_id(&engine, &root_b).await;

    let handle = engine.watch_sources().await.unwrap();
    assert_eq!(handle.watched().len(), 2);

    // Create in both vaults; each watcher converges its own set.
    write(&root_a, "note.md", "# A note\n\nalpha\n");
    write(&root_b, "note.md", "# B note\n\nbeta\n");
    converge("both sources ingested their own note", || {
        paths_for(&engine, src_a) == BTreeSet::from(["note.md".to_string()])
            && paths_for(&engine, src_b) == BTreeSet::from(["note.md".to_string()])
    })
    .await;

    // Delete A's note: A converges to empty, B keeps its document.
    std::fs::remove_file(root_a.join("note.md")).unwrap();
    converge("A empty, B intact", || {
        paths_for(&engine, src_a).is_empty()
            && paths_for(&engine, src_b) == BTreeSet::from(["note.md".to_string()])
    })
    .await;

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
