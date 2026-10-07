use mdreader_capture::{Adapter, CaptureOrigin, Limits, ProjectPolicy, Spool};
use serde_json::json;
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};
use tempfile::TempDir;

fn setup() -> (
    TempDir,
    ProjectPolicy,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "process-test".into(),
        project_root: root.clone(),
        repository_root: root.clone(),
        worktree_roots: vec![root],
        capture_subagents: true,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec!["selected".into()],
    };
    let path = temp.path().join("policy.json");
    fs::write(&path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let spool = temp.path().join("capture");
    (temp, policy, path, spool)
}
fn run(
    mode: &str,
    adapter: &str,
    policy: &std::path::Path,
    spool: &std::path::Path,
    input: &[u8],
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mdreader-capture"))
        .args([mode, adapter])
        .arg(policy)
        .arg(spool)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A hostile over-limit producer may observe the consumer close its pipe early.
    let _ = child.stdin.take().unwrap().write_all(input);
    child.wait_with_output().unwrap()
}
#[test]
fn real_hook_processes_enqueue_both_agents_without_reader_window_or_model() {
    let (_temp, policy, path, spool) = setup();
    for (adapter, session) in [("claude", "claude-process"), ("codex", "codex-process")] {
        let bytes = serde_json::to_vec(&json!({"hook_event_name":"Stop","cwd":policy.project_root,"session_id":session,"turn_id":"turn","prompt_id":"prompt","last_assistant_message":"# Durable plan\nVisible final output only."})).unwrap();
        let output = run("hook", adapter, &path, &spool, &bytes);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
    }
    let queue = Spool::open(&spool, Limits::default()).unwrap();
    assert_eq!(queue.pending(10).unwrap().len(), 2);
    let ids = queue.pending(10).unwrap();
    let a = queue.project(&ids[0], &policy).unwrap();
    let b = queue.project(&ids[1], &policy).unwrap();
    assert_eq!(a.path, b.path);
    assert_eq!(
        mdreader_capture::read_projection(&a.path, &Limits::default())
            .unwrap()
            .occurrences
            .len(),
        2
    );
}
#[cfg(unix)]
#[test]
fn hook_survives_brief_daemon_lock_contention_and_bounds_permanent_contention() {
    use fs2::FileExt;
    use std::time::{Duration, Instant};
    let (_temp, policy, path, spool) = setup();
    Spool::open(&spool, Limits::default()).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(spool.join("queue.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let bytes = serde_json::to_vec(&json!({"hook_event_name":"Stop","cwd":policy.project_root,"session_id":"contention","last_assistant_message":"# Visible plan"})).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let policy_path = path.clone();
    let spool_path = spool.clone();
    let input = bytes.clone();
    let hook = std::thread::spawn(move || {
        sender
            .send(run("hook", "claude", &policy_path, &spool_path, &input))
            .unwrap();
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        receiver.try_recv().is_err(),
        "contention must not immediately discard a visible plan"
    );
    FileExt::unlock(&lock).unwrap();
    let output = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    hook.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
    assert_eq!(
        Spool::open(&spool, Limits::default())
            .unwrap()
            .pending(10)
            .unwrap()
            .len(),
        1
    );
    lock.lock_exclusive().unwrap();
    let started = Instant::now();
    let output = run("hook", "claude", &path, &spool, &bytes);
    FileExt::unlock(&lock).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_ne!(output.status.code(), Some(2));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
}
#[test]
fn failed_hook_never_requests_continuation_or_reports_durable_success() {
    let (_temp, policy, path, spool) = setup();
    let invalid = run("hook", "codex", &path, &spool, b"not json");
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&invalid.stdout).trim(), "{}");
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("not json"));
    let output = run(
        "hook",
        "claude",
        &path,
        &spool,
        &vec![b'x'; Limits::default().max_event_bytes + 1],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds"));
    let queue = Spool::open(spool, Limits::default()).unwrap();
    assert!(queue.pending(1).unwrap().is_empty());
    let _ = policy;
}
#[test]
fn historical_backfill_requires_selected_session_and_receipts_survive_process_exit() {
    let (_temp, policy, path, spool) = setup();
    let input = |session: &str| {
        serde_json::to_vec(&json!({"hook_event_name":"Stop","cwd":policy.project_root,"session_id":session,"last_assistant_message":"# Explicit selected output"})).unwrap()
    };
    assert!(
        run("backfill", "claude", &path, &spool, &input("unselected"))
            .status
            .success()
    );
    let queue = Spool::open(&spool, Limits::default()).unwrap();
    assert!(queue.pending(10).unwrap().is_empty());
    assert!(run("backfill", "claude", &path, &spool, &input("selected"))
        .status
        .success());
    let ids = queue.pending(10).unwrap();
    assert_eq!(ids.len(), 1);
    queue.project(&ids[0], &policy).unwrap();
    queue.acknowledge(&ids[0]).unwrap();
    assert!(run("backfill", "claude", &path, &spool, &input("selected"))
        .status
        .success());
    assert!(queue.pending(10).unwrap().is_empty());
    assert!(matches!(
        queue
            .enqueue(
                Adapter::ClaudeCode,
                &input("selected"),
                &policy,
                CaptureOrigin::SelectedBackfill
            )
            .unwrap(),
        mdreader_capture::EnqueueOutcome::Duplicate { .. }
    ));
}

#[test]
fn crash_projection_worker() {
    use std::io::Write;
    let Some(base) = std::env::var_os("MDREADER_SYNTHETIC_CRASH_TEST") else {
        return;
    };
    let base = std::path::PathBuf::from(base);
    let policy = ProjectPolicy::load(&base.join("policy.json")).unwrap();
    let spool = Spool::open(base.join("capture"), Limits::default()).unwrap();
    let bytes=serde_json::to_vec(&json!({"hook_event_name":"Stop","cwd":policy.project_root,"session_id":"killed-session","turn_id":"killed-turn","last_assistant_message":"# Durable after SIGKILL\nPlan body."})).unwrap();
    let id = match spool
        .enqueue(Adapter::CodexHook, &bytes, &policy, CaptureOrigin::Live)
        .unwrap()
    {
        mdreader_capture::EnqueueOutcome::Queued { id } => id,
        other => panic!("{other:?}"),
    };
    spool.project(&id, &policy).unwrap();
    println!("projected:{id}");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn killed_service_after_projection_replays_without_losing_body_or_provenance() {
    use std::io::BufRead;
    let (temp, policy, _, spool_path) = setup();
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_projection_worker", "--nocapture"])
        .env("MDREADER_SYNTHETIC_CRASH_TEST", temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(worker.stdout.take().unwrap());
    let mut id = None;
    for _ in 0..16 {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if let Some((_, value)) = line.split_once("projected:") {
            id = Some(value.trim().to_owned());
            break;
        }
    }
    let id = id.expect("worker must publish its durable projection before it is killed");
    worker.kill().unwrap();
    assert!(!worker.wait().unwrap().success());
    let spool = Spool::open(&spool_path, Limits::default()).unwrap();
    assert_eq!(spool.pending(10).unwrap(), vec![id.clone()]);
    let projection = spool.project(&id, &policy).unwrap();
    assert!(fs::read_to_string(&projection.path)
        .unwrap()
        .ends_with("# Durable after SIGKILL\nPlan body."));
    assert_eq!(
        mdreader_capture::read_projection(&projection.path, spool.limits())
            .unwrap()
            .occurrences
            .len(),
        1
    );
    spool.acknowledge(&id).unwrap();
    assert!(spool.pending(10).unwrap().is_empty());
}
#[test]
fn legacy_codex_notification_receives_only_the_single_supported_event_argument() {
    let (_temp, policy, path, spool_path) = setup();
    let payload=json!({"type":"agent-turn-complete","thread-id":"notify-process","turn-id":"turn-9","cwd":policy.project_root,"last-assistant-message":"# Notify output","input-messages":["EXCLUDED INPUT PROMPT"]}).to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_mdreader-capture"))
        .args(["hook", "codex-notify"])
        .arg(&path)
        .arg(&spool_path)
        .arg(payload)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "{}");
    let spool = Spool::open(&spool_path, Limits::default()).unwrap();
    let ids = spool.pending(10).unwrap();
    assert_eq!(ids.len(), 1);
    let record = spool.read_pending(&ids[0]).unwrap();
    assert_eq!(record.provenance.session_id, "notify-process");
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("EXCLUDED INPUT PROMPT"));
}
