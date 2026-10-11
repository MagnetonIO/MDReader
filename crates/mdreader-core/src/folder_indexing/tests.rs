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
    assert!(store.search("firbefore", &[source.clone()], 10).unwrap()[0]
        .path
        .ends_with("renamed.md"));
    assert_eq!(
        store
            .search("mapleadded", &[source.clone()], 10)
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
        .search("mapleadded", &[source.clone()], 10)
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
            .search("retainedpine", &[source.clone()], 10)
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
            .search("newbirch", &[source.clone()], 10)
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
        .search("neverindexed", &[source.clone()], 10)
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
            .search("staycedar", &[source.clone()], 10)
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
            .search("upgradefir", &[source.clone()], 10)
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
            .search("retainedworkerpine", &[source.clone()], 10)
            .unwrap()
            .len(),
        1
    );
    assert!(!store.folder_index_status(&source).unwrap().complete);
}
