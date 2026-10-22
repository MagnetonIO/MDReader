//! A real owner indexes a recorded Markdown file outside every source, so
//! search finds it, while an unrecorded sibling stays unindexed. Synthetic
//! logs in a disposable home only.
use mdreader_service::{
    agents::activity::ActivityRequest, call, catalog::ReaderOwnedSources, Request,
};
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn activity(data: &std::path::Path, request: ActivityRequest) -> Value {
    call(data, &Request::AgentActivity { request }).unwrap()
}

#[test]
fn the_owner_indexes_a_recorded_file_outside_every_source() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let (home, data, repo) = (base.join("home"), base.join("data"), base.join("repo"));
    let log_dir = home.join(".claude/projects/-public-repo");
    for dir in [&log_dir, &data, &repo] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(repo.join("x.md"), "# X\nsyntheticprocessword").unwrap();
    std::fs::write(repo.join("y.md"), "# Y\nsyntheticsiblingword").unwrap();
    let line = json!({"type":"assistant","cwd":repo,"timestamp":"2026-10-21T08:00:00.000Z",
        "message":{"role":"assistant","content":[{"type":"tool_use","id":"w","name":"Write",
        "input":{"file_path":repo.join("x.md"),"content":"x"}}]}});
    let log = log_dir.join("77777777-7777-4777-8777-777777777777.jsonl");
    std::fs::write(&log, format!("{line}\n")).unwrap();
    let _daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_mdreader-service"))
            .args(["--no-model".as_ref(), data.as_os_str()])
            .env("HOME", &home)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ready = Instant::now();
    while call(&data, &Request::Ping).is_err() {
        assert!(ready.elapsed() < Duration::from_secs(10), "owner ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    activity(
        &data,
        ActivityRequest::SetTracking {
            agent: mdreader_core::agent_activity::Agent::Claude,
            enabled: true,
        },
    );
    activity(&data, ActivityRequest::Ingest { paths: vec![log] });
    let owned: ReaderOwnedSources =
        serde_json::from_value(call(&data, &Request::ReaderOwnedSources).unwrap()).unwrap();
    assert_eq!(owned.agent_files.len(), 1);
    let search = |query: &str| -> Vec<mdreader_core::Evidence> {
        serde_json::from_value(
            call(
                &data,
                &Request::Search {
                    query: query.into(),
                    sources: owned.agent_files.clone(),
                    limit: 10,
                },
            )
            .unwrap(),
        )
        .unwrap()
    };
    let indexed = Instant::now();
    loop {
        let hits = search("syntheticprocessword");
        if let [hit] = hits.as_slice() {
            assert_eq!(hit.path, "x.md");
            break;
        }
        assert!(
            indexed.elapsed() < Duration::from_secs(20),
            "owner indexed the recorded file"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(search("syntheticsiblingword").is_empty());
}
