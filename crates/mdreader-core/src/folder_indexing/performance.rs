//! Explicit synthetic probe, never part of routine tests or real-home discovery.
use super::*;
use std::{
    fs,
    time::{Duration, Instant},
};
#[test]
#[ignore = "explicit 50k-entry public synthetic performance gate"]
fn synthetic_50k_metadata_visibility_cached_search_and_event_latency() {
    let corpus = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    for folder in 0..100 {
        let dir = corpus.path().join(format!("folder-{folder:03}"));
        fs::create_dir(&dir).unwrap();
        for file in 0..500 {
            let extension = if file < 200 { "md" } else { "txt" };
            fs::write(
                dir.join(format!("note-{file:03}.{extension}")),
                format!("# Public {folder} {file}\nsyntheticcedar {folder} {file}\n"),
            )
            .unwrap();
        }
    }
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Synthetic corpus",
            SourceRoot::capture(corpus.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap()
        .id;
    store.queue_index(&source).unwrap();
    let generation = store.index_job(&source).unwrap().unwrap().generation;
    let began = Instant::now();
    let mut scan = store.begin_folder_scan(&source, generation).unwrap();
    let mut visible = None;
    let mut searchable = None;
    let mut query_times = Vec::new();
    let mut catalog_times = Vec::new();
    let until = Instant::now() + Duration::from_secs(600);
    let mut next_sample = Instant::now();
    loop {
        let phase = store.publish_folder_scan(&mut scan).unwrap();
        if visible.is_none()
            && !store
                .catalog_page(&source, None, None, 10)
                .unwrap()
                .documents
                .is_empty()
        {
            visible = Some(began.elapsed());
        }
        if Instant::now() >= next_sample {
            let q = Instant::now();
            let hits = store
                .search("syntheticcedar", &[source.clone()], 10)
                .unwrap();
            query_times.push(q.elapsed().as_micros());
            if !hits.is_empty() && searchable.is_none() {
                searchable = Some(began.elapsed());
            }
            let c = Instant::now();
            store.catalog_page(&source, None, None, 10).unwrap();
            catalog_times.push(c.elapsed().as_micros());
            next_sample = Instant::now() + Duration::from_millis(50);
        }
        if phase == FolderScanProgress::Complete {
            break;
        }
        assert!(Instant::now() < until);
        if phase == FolderScanProgress::Waiting {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let initial = began.elapsed();
    let initial_stats = scan.statistics();
    drop(scan);
    assert_eq!(
        store
            .folder_index_status(&source)
            .unwrap()
            .indexed_documents,
        20_000
    );
    query_times.sort_unstable();
    catalog_times.sort_unstable();
    let p95 = |times: &[u128]| times[(times.len() * 95 / 100).min(times.len() - 1)];
    store
        .record_index_job_if_current(&source, generation, "done", None)
        .unwrap();
    store.queue_index(&source).unwrap();
    let generation = store.index_job(&source).unwrap().unwrap().generation;
    let mut unchanged = store.begin_folder_scan(&source, generation).unwrap();
    let started = Instant::now();
    while store.publish_folder_scan(&mut unchanged).unwrap() != FolderScanProgress::Complete {
        assert!(started.elapsed() < Duration::from_secs(600));
        std::thread::sleep(Duration::from_millis(1));
    }
    let unchanged_time = started.elapsed();
    let unchanged_stats = unchanged.statistics();
    assert_eq!(unchanged_stats.body_reads, 0);
    assert_eq!(unchanged_stats.unchanged, 20_000);
    drop(unchanged);
    store
        .record_index_job_if_current(&source, generation, "done", None)
        .unwrap();
    fs::write(
        corpus.path().join("folder-000/note-000.md"),
        "# Changed\nsingleeventmaple\n",
    )
    .unwrap();
    let started = Instant::now();
    store
        .queue_folder_changes(&source, &["folder-000/note-000.md".into()])
        .unwrap();
    let generation = store.index_job(&source).unwrap().unwrap().generation;
    let mut event = store.begin_folder_scan(&source, generation).unwrap();
    while store.publish_folder_scan(&mut event).unwrap() != FolderScanProgress::Complete {
        std::thread::sleep(Duration::from_millis(1));
        assert!(started.elapsed() < Duration::from_secs(30));
    }
    let event_time = started.elapsed();
    assert_eq!(event.statistics().body_reads, 1);
    assert_eq!(
        store
            .search("singleeventmaple", &[source.clone()], 10)
            .unwrap()
            .len(),
        1
    );
    let mut warm_queries = Vec::new();
    let mut warm_catalog = Vec::new();
    for _ in 0..32 {
        let started = Instant::now();
        store
            .search("syntheticcedar", &[source.clone()], 10)
            .unwrap();
        warm_queries.push(started.elapsed().as_micros());
        let started = Instant::now();
        store.catalog_page(&source, None, None, 10).unwrap();
        warm_catalog.push(started.elapsed().as_micros());
    }
    warm_queries.sort_unstable();
    warm_catalog.sort_unstable();
    let discovery = Instant::now();
    let count = crate::visit_source_root(
        &store.stored_root(&source, true).unwrap(),
        &crate::ScanOptions {
            include_mdx: false,
            max_file_size: MAX_DOCUMENT_BYTES,
            respect_gitignore: true,
        },
        |_| Ok::<_, ReaderError>(()),
    )
    .unwrap();
    let discovery_time = discovery.elapsed();
    assert_eq!(count, 20_000);
    let rg_started = Instant::now();
    let rg = std::process::Command::new("rg")
        .args([
            "--files",
            "--no-require-git",
            "-g",
            "*.md",
            "-g",
            "*.markdown",
        ])
        .current_dir(corpus.path())
        .output();
    let rg_time = rg_started.elapsed();
    let rg_count = rg.ok().filter(|r| r.status.success()).map(|r| {
        r.stdout
            .split(|b| *b == b'\n')
            .filter(|r| !r.is_empty())
            .count()
    });
    if let Some(count) = rg_count {
        assert_eq!(count, 20_000);
    }
    println!("synthetic_probe entries=50000 markdown=20000 first_metadata_ms={} first_search_ms={} complete_ms={} during_index_search_p95_us={} during_index_catalog_p95_us={} initial_body_reads={} unchanged_ms={} unchanged_body_reads={} unchanged_reused={} event_ms={} event_body_reads={} rg_count={:?} warm_search_p95_us={} warm_catalog_p95_us={} retained_discovery_ms={} rg_discovery_ms={}",visible.unwrap().as_millis(),searchable.unwrap().as_millis(),initial.as_millis(),p95(&query_times),p95(&catalog_times),initial_stats.body_reads,unchanged_time.as_millis(),unchanged_stats.body_reads,unchanged_stats.unchanged,event_time.as_millis(),event.statistics().body_reads,rg_count,p95(&warm_queries),p95(&warm_catalog),discovery_time.as_millis(),rg_time.as_millis());
}
