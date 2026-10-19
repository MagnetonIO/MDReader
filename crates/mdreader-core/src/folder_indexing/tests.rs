use super::*;
use std::{
    fs,
    time::{Duration, Instant},
};
fn admit(store: &mut ReaderStore, root: &Path) -> String {
    store
        .add_local_source(
            "Public notes",
            SourceRoot::capture(root.canonicalize().unwrap()).unwrap(),
        )
        .unwrap()
        .id
}
fn drain(store: &mut ReaderStore, source: &str) -> Result<()> {
    let generation = store.index_job(source)?.unwrap().generation;
    let mut scan = store.begin_folder_scan(source, generation)?;
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match store.publish_folder_scan(&mut scan)? {
            FolderScanProgress::Complete => {
                store.record_index_job_if_current(source, generation, "done", None)?;
                return Ok(());
            }
            FolderScanProgress::Published => {}
            FolderScanProgress::Waiting => {
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
#[test]
fn full_then_targeted_add_rename_delete_preserves_unchanged_chunks_and_originals() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    fs::write(notes.path().join("keep.md"), "# Keep\ncedarretained").unwrap();
    fs::write(notes.path().join("old.md"), "# Old\nfirbefore").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = admit(&mut store, notes.path());
    store.queue_index(&source).unwrap();
    drain(&mut store, &source).unwrap();
    let row:i64=store.index.query_row("SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.path='keep.md'",[],|r|r.get(0)).unwrap();
    fs::rename(notes.path().join("old.md"), notes.path().join("renamed.md")).unwrap();
    fs::write(notes.path().join("added.markdown"), "# Added\nmapleadded").unwrap();
    store
        .queue_folder_changes(
            &source,
            &[
                "old.md".into(),
                "renamed.md".into(),
                "added.markdown".into(),
            ],
        )
        .unwrap();
    drain(&mut store, &source).unwrap();
    assert!(store
        .search("firbefore", std::slice::from_ref(&source), 10)
        .unwrap()[0]
        .path
        .ends_with("renamed.md"));
    assert_eq!(
        store
            .search("mapleadded", std::slice::from_ref(&source), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(row,store.index.query_row("SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.path='keep.md'",[],|r|r.get::<_,i64>(0)).unwrap());
    fs::remove_file(notes.path().join("added.markdown")).unwrap();
    store
        .queue_folder_changes(&source, &["added.markdown".into()])
        .unwrap();
    drain(&mut store, &source).unwrap();
    assert!(store
        .search("mapleadded", std::slice::from_ref(&source), 10)
        .unwrap()
        .is_empty());
    store.queue_index(&source).unwrap();
    drain(&mut store, &source).unwrap();
    assert_eq!(row,store.index.query_row("SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.path='keep.md'",[],|r|r.get::<_,i64>(0)).unwrap());
    assert_eq!(
        fs::read_to_string(notes.path().join("keep.md")).unwrap(),
        "# Keep\ncedarretained"
    );
    assert!(store.folder_index_status(&source).unwrap().complete);
}
#[test]
fn pause_reopen_and_interrupted_full_scan_keep_retained_rows_without_false_purge() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    fs::write(notes.path().join("note.md"), "# Note\nretainedpine").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = admit(&mut store, notes.path());
    store.queue_index(&source).unwrap();
    drain(&mut store, &source).unwrap();
    let observed = store.folder_index_status(&source).unwrap().observed_at_ms;
    store.set_folder_monitoring(&source, false).unwrap();
    drop(store);
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert!(!store.folder_index_status(&source).unwrap().monitor_enabled);
    store
        .queue_folder_changes(&source, &["note.md".into()])
        .unwrap();
    assert_eq!(store.folder_index_status(&source).unwrap().phase, "paused");
    store.set_folder_monitoring(&source, true).unwrap();
    let generation = store.index_job(&source).unwrap().unwrap().generation;
    let scan = store.begin_folder_scan(&source, generation).unwrap();
    drop(scan);
    drop(store);
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(
        store
            .search("retainedpine", std::slice::from_ref(&source), 10)
            .unwrap()
            .len(),
        1
    );
    let status = store.folder_index_status(&source).unwrap();
    assert!(!status.complete);
    assert_eq!(status.observed_at_ms, observed);
    drain(&mut store, &source).unwrap();
    assert!(store.folder_index_status(&source).unwrap().complete);
}
#[test]
fn event_during_scan_queues_next_generation_and_revocation_refuses_publication() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    fs::write(notes.path().join("note.md"), "# Public\nfirstbirch").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = admit(&mut store, notes.path());
    store.queue_index(&source).unwrap();
    let generation = store.index_job(&source).unwrap().unwrap().generation;
    let mut scan = store.begin_folder_scan(&source, generation).unwrap();
    store
        .queue_folder_changes(&source, &["new.md".into()])
        .unwrap();
    assert_eq!(
        generation,
        store.index_job(&source).unwrap().unwrap().generation
    );
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match store.publish_folder_scan(&mut scan) {
            Err(ReaderError::Cancelled) => break,
            Ok(_) => {
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(1));
            }
            other => panic!("unexpected scan result {other:?}"),
        }
    }
    assert!(store.index_job(&source).unwrap().unwrap().generation > generation);
    drop(scan);
    fs::write(notes.path().join("new.md"), "# New\nnewbirch").unwrap();
    drain(&mut store, &source).unwrap();
    assert_eq!(
        store
            .search("newbirch", std::slice::from_ref(&source), 10)
            .unwrap()
            .len(),
        1
    );
    store.queue_index(&source).unwrap();
    let generation = store.index_job(&source).unwrap().unwrap().generation;
    let mut scan = store.begin_folder_scan(&source, generation).unwrap();
    store.set_source_enabled(&source, false).unwrap();
    assert!(store.publish_folder_scan(&mut scan).is_err());
    assert!(store.read_document(&source, "note.md", None).is_err());
}
#[cfg(unix)]
#[test]
fn ignore_refusal_keeps_prior_snapshot_and_ignored_new_files_never_index() {
    use std::os::unix::fs::symlink;
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(notes.path().join("keep.md"), "# Keep\nstaycedar").unwrap();
    fs::write(notes.path().join(".ignore"), "ignored.md\n").unwrap();
    fs::write(notes.path().join("ignored.md"), "# Ignore\nneverindexed").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = admit(&mut store, notes.path());
    store.queue_index(&source).unwrap();
    drain(&mut store, &source).unwrap();
    assert!(store
        .search("neverindexed", std::slice::from_ref(&source), 10)
        .unwrap()
        .is_empty());
    fs::remove_file(notes.path().join("keep.md")).unwrap();
    fs::write(outside.path().join("rules"), "*").unwrap();
    symlink(
        outside.path().join("rules"),
        notes.path().join(".contextignore"),
    )
    .unwrap();
    store.queue_index(&source).unwrap();
    assert!(drain(&mut store, &source).is_err());
    assert_eq!(
        store
            .search("staycedar", std::slice::from_ref(&source), 10)
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn prior_schema_upgrade_is_additive_and_reopen_preserves_sources_and_bookmarks() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    fs::write(notes.path().join("a.md"), "# A\nupgradefir").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = admit(&mut store, notes.path());
    store.index_source(&source).unwrap();
    store
        .library
        .execute("INSERT INTO bookmarks VALUES(?1,'a.md',3)", [&source])
        .unwrap();
    for name in [
        "reader_folder_files_v1",
        "reader_folder_scans_v1",
        "reader_folder_index_schema_v1",
    ] {
        store
            .index
            .execute(&format!("DROP TABLE {name}"), [])
            .unwrap();
    }
    for name in [
        "reader_folder_events_v1",
        "reader_folder_preferences_v1",
        "reader_folder_schema_v1",
    ] {
        store
            .library
            .execute(&format!("DROP TABLE {name}"), [])
            .unwrap();
    }
    drop(store);
    let store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(
        store
            .search("upgradefir", std::slice::from_ref(&source), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .library
            .query_row(
                "SELECT line FROM bookmarks WHERE source_id=?1",
                [&source],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        3
    );
    assert_eq!(store.list_sources().unwrap().len(), 1);
}

#[test]
fn failed_reader_wakes_backpressured_producer_and_retains_prior_rows() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    fs::write(
        notes.path().join("retained.md"),
        "# Prior\nretainedworkerpine",
    )
    .unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = admit(&mut store, notes.path());
    store.queue_index(&source).unwrap();
    drain(&mut store, &source).unwrap();
    fs::remove_file(notes.path().join("retained.md")).unwrap();
    // More invalid inputs than both workers and the bounded input/output queues.
    for n in 0..64 {
        fs::write(notes.path().join(format!("{n:03}.md")), [0xff]).unwrap();
    }
    store.queue_index(&source).unwrap();
    let start = Instant::now();
    assert!(matches!(
        drain(&mut store, &source),
        Err(ReaderError::Invalid(_))
    ));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(
        store
            .search("retainedworkerpine", std::slice::from_ref(&source), 10)
            .unwrap()
            .len(),
        1
    );
    assert!(!store.folder_index_status(&source).unwrap().complete);
}
type Edge = (String, String, u32, u32);
fn edges(store: &ReaderStore, source: &str) -> Vec<Edge> {
    store.index.prepare("SELECT o.path,t.path,r.origin_line,r.target_line FROM resolved_links r JOIN documents o ON o.id=r.origin_id JOIN documents t ON t.id=r.target_id WHERE r.source_id=?1 ORDER BY 1,2,3,4").unwrap().query_map([source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().collect::<std::result::Result<_,_>>().unwrap()
}
/// Rescan after `change`; the published graph must equal a full rebuild.
/// Returns the scan's graph and how many full rebuilds it performed.
fn rescan(store: &mut ReaderStore, source: &str, change: impl FnOnce()) -> (Vec<Edge>, u32) {
    change();
    let before = crate::store::FULL_LINK_REBUILDS.with(|c| c.get());
    store.queue_index(source).unwrap();
    drain(store, source).unwrap();
    let rebuilds = crate::store::FULL_LINK_REBUILDS.with(|c| c.get()) - before;
    let scanned = edges(store, source);
    let tx = store.index.unchecked_transaction().unwrap();
    crate::store::rebuild_resolved_links(&tx, source, &crate::store::Cancellation::default())
        .unwrap();
    let full = edges(store, source);
    tx.rollback().unwrap();
    assert_eq!(scanned, full, "scan graph must equal a full rebuild");
    (scanned, rebuilds)
}
#[test]
fn incremental_link_resolution_equals_a_full_rebuild() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let root = notes.path().to_path_buf();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(
        root.join("a.md"),
        "# Alpha\n\nSee [b](b.md#second), [[b]] and [t](Beta%20Title).\n\n## Tail\n",
    )
    .unwrap();
    let beta = "# Beta Title\n\nIntro.\n\n## Second\nBody [a](a.md) [self](#second).\n";
    fs::write(root.join("b.md"), beta).unwrap();
    fs::write(
        root.join("sub/c.md"),
        "# Gamma\n\n[up](../b.md#second) [plain](a) [[gamma]]\n",
    )
    .unwrap();
    fs::write(root.join("d.md"), "# Delta\n[[gamma]] [none](nowhere.md)\n").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = admit(&mut store, &root);
    store.queue_index(&source).unwrap();
    drain(&mut store, &source).unwrap();
    let initial = edges(&store, &source);
    let second = |graph: &[Edge], from: &str| {
        graph
            .iter()
            .find(|e| e.0 == from && e.1 == "b.md" && e.3 > 1)
            .map(|e| e.3)
    };
    assert_eq!(second(&initial, "sub/c.md"), Some(5));

    // Nothing changed: the graph is kept without any resolution.
    let (graph, rebuilds) = rescan(&mut store, &source, || {});
    assert_eq!((graph, rebuilds), (initial.clone(), 0));

    // Body-only edit: heading lines move and an outgoing link is added, but
    // aliases and headings are unchanged, so only affected links re-resolve.
    let (graph, rebuilds) = rescan(&mut store, &source, || {
        fs::write(
            root.join("b.md"),
            "# Beta Title\n\nIntro.\n\nMore.\n\n## Second\nBody [a](a.md) [self](#second) [c](sub/c.md).\n",
        )
        .unwrap()
    });
    assert_eq!(
        rebuilds, 0,
        "body-only edit must not rebuild the source graph"
    );
    assert_eq!(second(&graph, "sub/c.md"), Some(7));
    assert_eq!(second(&graph, "a.md"), Some(7));
    assert!(graph.contains(&("b.md".into(), "sub/c.md".into(), 8, 1)));

    // Title change, file add and file delete alter what links can resolve to.
    let (_, rebuilds) = rescan(&mut store, &source, || {
        fs::write(
            root.join("b.md"),
            beta.replace("Beta Title", "Beta Renamed"),
        )
        .unwrap()
    });
    assert_eq!(rebuilds, 1);
    let (graph, rebuilds) = rescan(&mut store, &source, || {
        fs::write(root.join("sub/beta title.md"), "# Other\n").unwrap()
    });
    assert_eq!(rebuilds, 1);
    assert!(graph
        .iter()
        .any(|e| e.0 == "a.md" && e.1 == "sub/beta title.md"));
    let (graph, rebuilds) = rescan(&mut store, &source, || {
        fs::remove_file(root.join("sub/c.md")).unwrap()
    });
    assert_eq!(rebuilds, 1);
    assert!(!graph.iter().any(|e| e.0 == "sub/c.md" || e.1 == "sub/c.md"));
}
