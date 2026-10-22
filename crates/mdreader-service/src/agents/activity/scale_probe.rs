//! Resource probe, not a routine test: run explicitly with
//! `cargo test -p mdreader-service --release scale_probe -- --ignored --nocapture`
//! (under `/usr/bin/time -l` for peak RSS). Synthetic logs only.
use super::*;
use std::io::Write;
use std::time::Instant;

const LOGS: usize = 2_000;
const LOG_BYTES: usize = 100 * 1024;

fn write_logs(projects: &Path) -> u64 {
    let filler = "x".repeat(400);
    let mut total = 0;
    for i in 0..LOGS {
        let dir = projects.join(format!("-public-project-{}", i % 40));
        std::fs::create_dir_all(&dir).unwrap();
        let mut log = std::io::BufWriter::new(
            std::fs::File::create(dir.join(format!("{i:08}-0000-4000-8000-000000000000.jsonl")))
                .unwrap(),
        );
        let mut written = 0;
        let mut line = 0;
        while written < LOG_BYTES {
            let entry = if line % 20 == 5 {
                serde_json::json!({"type":"assistant","cwd":"/public/project","timestamp":"2026-10-20T10:00:00.000Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"Write","input":{"file_path":format!("/public/project/docs/{i}-{line}.md"),"content":filler}}]}})
            } else {
                serde_json::json!({"type":"assistant","cwd":"/public/project","timestamp":"2026-10-20T10:00:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":filler}]}})
            };
            let bytes = serde_json::to_vec(&entry).unwrap();
            log.write_all(&bytes).unwrap();
            log.write_all(b"\n").unwrap();
            written += bytes.len() + 1;
            line += 1;
        }
        total += written as u64;
    }
    total
}

fn pass(run: &dyn Fn(ActivityRequest) -> Result<Value>) -> (usize, Duration) {
    let started = Instant::now();
    let mut paths: Vec<PathBuf> =
        serde_json::from_value(run(ActivityRequest::Discover).unwrap()).unwrap();
    let found = paths.len();
    while !paths.is_empty() {
        let rest = paths.split_off(paths.len().min(64));
        let reply: IngestReply =
            serde_json::from_value(run(ActivityRequest::Ingest { paths }).unwrap()).unwrap();
        paths = reply.pending.into_iter().chain(rest).collect();
    }
    (found, started.elapsed())
}
use std::time::Duration;

#[test]
#[ignore = "resource probe; run explicitly"]
fn scale_probe_first_run_and_incremental_rerun() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap().join("home");
    let homes = Homes::under(&home);
    let bytes = write_logs(&homes.claude_projects);
    let store = std::sync::Mutex::new(ReaderStore::open(&temp.path().join("reader")).unwrap());
    let run = |request| {
        execute(
            &|| store.lock().map_err(|_| ReaderError::Unavailable),
            &homes,
            request,
        )
    };
    run(ActivityRequest::SetTracking {
        agent: Agent::Claude,
        enabled: true,
    })
    .unwrap();
    let (found, first) = pass(&run);
    let (again, second) = pass(&run);
    let page: Page = serde_json::from_value(
        run(ActivityRequest::List {
            agent: None,
            offset: 0,
            limit: 5,
            filter: None,
            utc_offset_minutes: None,
        })
        .unwrap(),
    )
    .unwrap();
    println!("logs={LOGS} bytes={bytes} first_found={found} first={first:?} rerun_found={again} rerun={second:?} top_files={}", page.sessions[0].file_count);
    assert_eq!(found, LOGS);
    assert_eq!(again, 0, "an unchanged rerun reads nothing");
}
