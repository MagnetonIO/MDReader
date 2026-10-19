use mdreader_core::{ReaderStore, SourceRoot};
use std::time::{Duration, Instant};

fn source(store: &mut ReaderStore, directory: &std::path::Path, name: &str) -> String {
    store
        .add_local_source(
            name,
            SourceRoot::capture(directory.canonicalize().unwrap()).unwrap(),
        )
        .unwrap()
        .id
}
fn vector() -> Vec<f32> {
    let mut values = vec![0.0; 384];
    values[0] = 1.0;
    values
}
#[test]
fn source_status_counts_have_a_bounded_metadata_lookup_at_ten_thousand_vectors() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let markdown = (0..10_000)
        .map(|index| format!("## Section {index}\n\nSynthetic passage {index}.\n\n"))
        .collect::<String>();
    std::fs::write(notes.path().join("many.md"), markdown).unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = source(&mut store, notes.path(), "Synthetic counts");
    store.index_source(&source).unwrap();
    let (revision, inputs) = store.embedding_inputs(&source, "many.md").unwrap();
    assert_eq!(inputs.len(), 10_000);
    store
        .install_embeddings(&source, "many.md", &revision, &vec![vector(); inputs.len()])
        .unwrap();
    let selected = std::slice::from_ref(&source);
    let warm = store.index_status(selected).unwrap();
    assert_eq!(
        (warm.documents, warm.chunks, warm.embedded),
        (1, 10_000, 10_000)
    );
    let mut samples = Vec::new();
    for _ in 0..10 {
        let begin = Instant::now();
        let status = store.index_status(selected).unwrap();
        samples.push(begin.elapsed());
        assert_eq!(status.embedded, 10_000);
    }
    samples.sort();
    let maximum = samples[9];
    eprintln!("10k status count p50={:?} p95/max={maximum:?}", samples[4]);
    assert!(maximum<Duration::from_millis(20),"progress counting must use compact membership metadata instead of loading all vector payloads: {maximum:?}");
}

#[test]
fn memberships_commit_with_vectors_and_follow_revision_delete_and_forget_cascades() {
    let data = tempfile::tempdir().unwrap();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("a.md"), "## One\nFirst.\n\n## Two\nSecond.\n").unwrap();
    std::fs::write(b.path().join("b.md"), "# B\nOther source.\n").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let sa = source(&mut store, a.path(), "A");
    let sb = source(&mut store, b.path(), "B");
    store.index_source(&sa).unwrap();
    store.index_source(&sb).unwrap();
    let (revision, inputs) = store.embedding_inputs(&sa, "a.md").unwrap();
    assert_eq!(inputs.len(), 2);
    store
        .install_embeddings(&sa, "a.md", &revision, &vec![vector(); 2])
        .unwrap();
    let (other_revision, _) = store.embedding_inputs(&sb, "b.md").unwrap();
    store
        .install_embeddings(&sb, "b.md", &other_revision, &[vector()])
        .unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sa))
            .unwrap()
            .embedded,
        2
    );
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sb))
            .unwrap()
            .embedded,
        1
    );
    assert_eq!(
        store
            .index_status(&[sa.clone(), sb.clone()])
            .unwrap()
            .embedded,
        3
    );
    assert!(store.pending_embedding_documents(&sa).unwrap().is_empty());
    // Reinstall is idempotent, not an incrementing counter.
    store
        .install_embeddings(&sa, "a.md", &revision, &vec![vector(); 2])
        .unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sa))
            .unwrap()
            .embedded,
        2
    );
    let mut changed = vector();
    changed[0] = 0.0;
    changed[1] = 1.0;
    // The first replacement has already been written when validation of the
    // second fails; rollback must preserve BOTH vectors and their memberships.
    assert!(store
        .install_embeddings(&sa, "a.md", &revision, &[changed, vec![f32::NAN; 384]])
        .is_err());
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sa))
            .unwrap()
            .embedded,
        2
    );
    let preserved = store
        .semantic_search(&vector(), std::slice::from_ref(&sa), 10)
        .unwrap();
    assert_eq!(preserved.len(), 2);
    assert!(preserved.iter().all(|hit| (hit.score - 1.0).abs() < 1e-6));
    assert!(store
        .install_embeddings(&sa, "a.md", "stale-revision", &vec![vector(); 2])
        .is_err());
    store.record_index_job(&sa, "embedding", None).unwrap();
    assert_eq!(store.index_job(&sa).unwrap().unwrap().embedded, 2);
    // A later scan error must roll back memberships cascaded by replacing an
    // earlier file in the same atomic source transaction.
    std::fs::write(a.path().join("a.md"), "## Temporary\nNot committed.\n").unwrap();
    std::fs::write(a.path().join("z-invalid.md"), [0xff]).unwrap();
    assert!(store.index_source(&sa).is_err());
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sa))
            .unwrap()
            .embedded,
        2
    );
    assert_eq!(
        store
            .semantic_search(&vector(), std::slice::from_ref(&sa), 10)
            .unwrap()
            .len(),
        2
    );
    assert!(store.pending_embedding_documents(&sa).unwrap().is_empty());
    std::fs::remove_file(a.path().join("z-invalid.md")).unwrap();
    std::fs::write(
        a.path().join("a.md"),
        "## Changed\nNew first.\n\n## Second\nNew second.\n\n## Third\nNew third.\n",
    )
    .unwrap();
    store.index_source(&sa).unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sa))
            .unwrap()
            .embedded,
        0
    );
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sb))
            .unwrap()
            .embedded,
        1
    );
    assert_eq!(
        store.pending_embedding_documents(&sa).unwrap(),
        vec!["a.md"]
    );
    let (fresh, inputs) = store.embedding_inputs(&sa, "a.md").unwrap();
    store
        .install_embeddings(&sa, "a.md", &fresh, &vec![vector(); inputs.len()])
        .unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sa))
            .unwrap()
            .embedded,
        3
    );
    std::fs::remove_file(a.path().join("a.md")).unwrap();
    store.index_source(&sa).unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sa))
            .unwrap()
            .embedded,
        0
    );
    store.set_source_enabled(&sb, false).unwrap();
    assert!(store.index_status(std::slice::from_ref(&sb)).is_err());
    store.set_source_enabled(&sb, true).unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sb))
            .unwrap()
            .embedded,
        1
    );
    store.set_bookmark(&sb, "b.md", 1).unwrap();
    store.forget_source_index(&sb).unwrap();
    assert!(store.index_status(std::slice::from_ref(&sb)).is_err());
    store.set_source_enabled(&sb, true).unwrap();
    let forgotten = store.index_status(std::slice::from_ref(&sb)).unwrap();
    assert_eq!(
        (forgotten.documents, forgotten.chunks, forgotten.embedded),
        (0, 0, 0)
    );
    assert_eq!(store.bookmark(&sb, "b.md").unwrap(), Some(1));
    store.index_source(&sb).unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&sb))
            .unwrap()
            .embedded,
        0
    );
    assert_eq!(
        store.pending_embedding_documents(&sb).unwrap(),
        vec!["b.md"]
    );
}

#[test]
fn v4_upgrade_backfills_public_vector_membership_and_rebuild_discards_only_derived_state() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    std::fs::write(
        notes.path().join("embedded.md"),
        "# Embedded\nReal membership.\n",
    )
    .unwrap();
    std::fs::write(
        notes.path().join("pending.md"),
        "# Pending\nNo vector yet.\n",
    )
    .unwrap();
    let selected;
    {
        let mut store = ReaderStore::open(data.path()).unwrap();
        selected = source(&mut store, notes.path(), "Migration");
        store.index_source(&selected).unwrap();
        let (revision, _) = store.embedding_inputs(&selected, "embedded.md").unwrap();
        store
            .install_embeddings(&selected, "embedded.md", &revision, &[vector()])
            .unwrap();
        store.set_bookmark(&selected, "embedded.md", 2).unwrap();
    }
    // A genuine v4 fixture has vectors and chunks, but no v5 metadata table.
    let old = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    old.execute_batch("DROP INDEX documents_parser_version; ALTER TABLE documents DROP COLUMN parser_version; DROP TABLE embedding_memberships; PRAGMA user_version=4;")
        .unwrap();
    drop(old);
    assert!(
        ReaderStore::open_readonly(data.path()).is_err(),
        "readonly must not silently migrate old schema"
    );
    let mut store = ReaderStore::open(data.path()).unwrap();
    let index = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    assert_eq!(
        index
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        index
            .query_row("SELECT count(*) FROM embedding_memberships", [], |row| row
                .get::<_, u64>(
                0
            ))
            .unwrap(),
        1
    );
    drop(index);
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&selected))
            .unwrap()
            .embedded,
        1
    );
    assert_eq!(
        store.pending_embedding_documents(&selected).unwrap(),
        vec!["pending.md"]
    );
    assert_eq!(
        store
            .semantic_search(&vector(), std::slice::from_ref(&selected), 10)
            .unwrap()[0]
            .path,
        "embedded.md"
    );
    assert_eq!(store.bookmark(&selected, "embedded.md").unwrap(), Some(2));
    store.rebuild_index().unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&selected))
            .unwrap()
            .embedded,
        0,
        "an older parse is rechunked, so its vectors are discarded"
    );
    let (revision, _) = store.embedding_inputs(&selected, "embedded.md").unwrap();
    store
        .install_embeddings(&selected, "embedded.md", &revision, &[vector()])
        .unwrap();
    store.rebuild_index().unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&selected))
            .unwrap()
            .embedded,
        1,
        "unchanged reindex preserves membership"
    );
    let reader = ReaderStore::open_readonly(data.path()).unwrap();
    assert_eq!(
        reader
            .index_status(std::slice::from_ref(&selected))
            .unwrap()
            .embedded,
        1
    );
    drop(reader);
    drop(store);
    std::fs::remove_file(data.path().join("index.sqlite")).unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&selected))
            .unwrap()
            .embedded,
        0
    );
    assert_eq!(store.bookmark(&selected, "embedded.md").unwrap(), Some(2));
    store.rebuild_index().unwrap();
    let rebuilt = store.index_status(std::slice::from_ref(&selected)).unwrap();
    assert_eq!(
        (rebuilt.documents, rebuilt.chunks, rebuilt.embedded),
        (2, 2, 0)
    );
    assert_eq!(
        store.pending_embedding_documents(&selected).unwrap(),
        vec!["embedded.md", "pending.md"]
    );
}
