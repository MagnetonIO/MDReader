use mdreader_core::SourceRoot;
use mdreader_service::{call, Request};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::tempdir;
struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(directory: &std::path::Path) -> Daemon {
    let process = Command::new(env!("CARGO_BIN_EXE_mdreader-service"))
        .arg("--no-model")
        .arg(directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut daemon = Daemon(process);
    for _ in 0..100 {
        if call(directory, &Request::Ping).is_ok() {
            return daemon;
        }
        if let Some(status) = daemon.0.try_wait().unwrap() {
            panic!("service exited: {status}");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("service did not become ready");
}
#[test]
fn independent_clients_share_one_durable_writer_and_survive_restart() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    std::fs::write(
        notes.path().join("plan.md"),
        "# Plan\nDurable search evidence.",
    )
    .unwrap();
    let daemon = start(data.path());
    let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
    let source = call(
        data.path(),
        &Request::Admit {
            name: "Notes".into(),
            path: root.path,
            identity: root.identity,
        },
    )
    .unwrap();
    let id = source["id"].as_str().unwrap().to_string();
    call(
        data.path(),
        &Request::Index {
            source_id: id.clone(),
        },
    )
    .unwrap();
    let search = Request::Search {
        query: "Durable".into(),
        sources: vec![id.clone()],
        limit: 10,
    };
    assert_eq!(call(data.path(), &search).unwrap()[0]["path"], "plan.md");
    let other = Command::new(env!("CARGO_BIN_EXE_mdreader-service"))
        .arg("--no-model")
        .arg(data.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!other.success());
    assert_eq!(call(data.path(), &search).unwrap()[0]["path"], "plan.md");
    drop(daemon);
    let _restarted = start(data.path());
    assert_eq!(call(data.path(), &search).unwrap()[0]["path"], "plan.md");
    assert!(call(
        data.path(),
        &Request::Search {
            query: "Durable".into(),
            sources: vec!["ungranted".into()],
            limit: 10
        }
    )
    .is_err());
}

#[test]
fn service_bootstrap_launches_once_and_preserves_nullable_user_state() {
    let data = tempdir().unwrap();
    let executable = std::path::Path::new(env!("CARGO_BIN_EXE_mdreader-service"));
    let child = mdreader_service::ensure_running(data.path(), executable, &["--no-model".into()])
        .unwrap()
        .expect("first client launches the service");
    let _daemon = Daemon(child);
    assert!(
        mdreader_service::ensure_running(data.path(), executable, &["--no-model".into()])
            .unwrap()
            .is_none()
    );
    let notes = tempdir().unwrap();
    std::fs::write(notes.path().join("a.md"), "# A\nPersisted bookmark").unwrap();
    let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
    let id = call(
        data.path(),
        &Request::Admit {
            name: "Notes".into(),
            path: root.path,
            identity: root.identity,
        },
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let get = Request::Bookmark {
        source_id: id.clone(),
        path: "a.md".into(),
    };
    assert!(call(data.path(), &get).unwrap().is_null());
    call(
        data.path(),
        &Request::SetBookmark {
            source_id: id,
            path: "a.md".into(),
            line: 2,
        },
    )
    .unwrap();
    assert_eq!(call(data.path(), &get).unwrap(), 2);
}

#[test]
fn queued_index_is_durable_and_lexical_search_works_without_model() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    std::fs::write(
        notes.path().join("plan.md"),
        "# Plan\nOffline durable search",
    )
    .unwrap();
    let _daemon = start(data.path());
    let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
    let id = call(
        data.path(),
        &Request::Admit {
            name: "Notes".into(),
            path: root.path,
            identity: root.identity,
        },
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        data.path(),
        &Request::StartIndex {
            source_id: id.clone(),
        },
    )
    .unwrap();
    let mut terminal = false;
    for _ in 0..150 {
        let job = call(
            data.path(),
            &Request::JobStatus {
                source_id: id.clone(),
            },
        )
        .unwrap();
        if job["phase"] == "model-unavailable" {
            terminal = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        terminal,
        "lexical-only job must finish without a model download"
    );
    assert!(!data.path().join("models").exists());
    let outcome = call(
        data.path(),
        &Request::SearchRanked {
            query: "offline".into(),
            sources: vec![id.clone()],
            limit: 10,
            mode: mdreader_service::SearchMode::Hybrid,
        },
    )
    .unwrap();
    assert_eq!(outcome["hits"][0]["path"], "plan.md");
    assert!(outcome["semantic_unavailable"].is_string());
    let exact = call(
        data.path(),
        &Request::SearchRanked {
            query: "offline".into(),
            sources: vec![id.clone()],
            limit: 10,
            mode: mdreader_service::SearchMode::Exact,
        },
    )
    .unwrap();
    assert!(exact["semantic_unavailable"].is_null());
    let semantic = call(
        data.path(),
        &Request::SearchRanked {
            query: "offline".into(),
            sources: vec![id],
            limit: 10,
            mode: mdreader_service::SearchMode::Semantic,
        },
    )
    .unwrap();
    assert!(semantic["hits"].as_array().unwrap().is_empty());
}

#[test]
#[cfg(unix)]
fn windowless_service_admits_capture_and_indexes_it_without_manual_import() {
    use mdreader_capture::{Adapter, CaptureOrigin, Limits, ProjectPolicy, Spool};
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    let root = notes.path().canonicalize().unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "windowless-project".into(),
        project_root: root.clone(),
        repository_root: root.clone(),
        worktree_roots: vec![root.clone()],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    let _daemon = start(data.path());
    call(
        data.path(),
        &Request::ConfigureCapture {
            policy: policy.clone(),
        },
    )
    .unwrap();
    let spool = Spool::open(data.path().join("captures"), Limits::default()).unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"windowless","cwd":root,"last_assistant_message":"# Handoff\nWindowlessdurability verification"})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match spool.enqueue(Adapter::ClaudeCode, &event, &policy, CaptureOrigin::Live) {
            Err(mdreader_capture::CaptureError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => {
                result.unwrap();
                break;
            }
        }
    }
    let mut found = false;
    for _ in 0..200 {
        let sources = call(data.path(), &Request::ListSources).unwrap();
        let id = sources[0]["id"].as_str().unwrap().to_string();
        let hits = call(
            data.path(),
            &Request::Search {
                query: "Windowlessdurability".into(),
                sources: vec![id],
                limit: 10,
            },
        )
        .unwrap();
        if hits.as_array().unwrap().len() == 1 {
            found = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        found,
        "headless writer must consume its persisted project policy and spool"
    );
    assert!(spool.pending(10).unwrap().is_empty());
}
