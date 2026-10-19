#![cfg(unix)]
use mdreader_capture::{Adapter, ProjectPolicy};
use mdreader_service::{
    call,
    capture::backfill::{BackfillReply, BackfillRequest},
    Request,
};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(directory: &Path) -> Daemon {
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_mdreader-service"))
            .arg("--no-model")
            .arg(directory)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if call(directory, &Request::Ping).is_ok() {
            return daemon;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "synthetic service exited before readiness"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("synthetic service readiness deadline exceeded");
}
fn request(directory: &Path, input: BackfillRequest) -> BackfillReply {
    serde_json::from_value(
        call(
            directory,
            &Request::CaptureBackfill {
                project_id: "synthetic-backfill-process".into(),
                request: input,
            },
        )
        .unwrap(),
    )
    .unwrap()
}
#[test]
fn real_service_selected_output_replay_after_restart_has_one_occurrence_and_searchable_body() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let daemon = start(data.path());
    call(
        data.path(),
        &Request::ConfigureCapture {
            policy: ProjectPolicy {
                enabled: true,
                project_id: "synthetic-backfill-process".into(),
                project_root: root.clone(),
                repository_root: root.clone(),
                worktree_roots: vec![root.clone()],
                capture_subagents: false,
                excluded_paths: vec![],
                excluded_sessions: vec![],
                selected_backfill_sessions: vec![],
            },
        },
    )
    .unwrap();
    let event=serde_json::json!({"hook_event_name":"Stop","session_id":"explicit-synthetic-session","cwd":root,"last_assistant_message":"# Backfill evidence\nPublic selected visible response.","thinking":"NEVER-PERSIST"}).to_string();
    let review = match request(
        data.path(),
        BackfillRequest::Review {
            adapter: Adapter::CodexHook,
            event,
        },
    ) {
        BackfillReply::Reviewed { value } => value,
        _ => panic!("review expected"),
    };
    let enqueue = BackfillRequest::Enqueue {
        adapter: Adapter::CodexHook,
        event: review.canonical_event,
        review_digest: review.review_digest,
        session_id: review.session_id,
    };
    // Deliberately ignore the first success receipt: an uncertain caller must
    // replay this same event instead of synthesizing a new session/turn.
    request(data.path(), enqueue.clone());
    drop(daemon);
    let _restarted = start(data.path());
    assert!(matches!(
        request(data.path(), enqueue),
        BackfillReply::Enqueued {
            outcome: mdreader_capture::EnqueueOutcome::Duplicate { .. }
        }
    ));
    let deadline = Instant::now() + Duration::from_secs(3);
    while call(data.path(), &Request::CaptureStatus).unwrap()["admitted_occurrences"] != 1 {
        assert!(
            Instant::now() < deadline,
            "capture admission deadline exceeded"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let sources = call(data.path(), &Request::ListSources).unwrap();
    let source = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["name"] == "Agent captures")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        data.path(),
        &Request::Index {
            source_id: source.clone(),
        },
    )
    .unwrap();
    let hits = call(
        data.path(),
        &Request::Search {
            query: "Backfill evidence".into(),
            sources: vec![source.clone()],
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
    let path = hits[0]["path"].as_str().unwrap().to_string();
    let document = call(
        data.path(),
        &Request::Read {
            source_id: source,
            path,
            revision: None,
        },
    )
    .unwrap();
    assert!(document["markdown"]
        .as_str()
        .unwrap()
        .ends_with("# Backfill evidence\nPublic selected visible response."));
    assert!(!document["markdown"]
        .as_str()
        .unwrap()
        .contains("NEVER-PERSIST"));
    assert_eq!(
        call(data.path(), &Request::CaptureStatus).unwrap()["admitted_occurrences"],
        1
    );
}
