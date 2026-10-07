use mdreader_capture::{Adapter, CaptureOrigin, EnqueueOutcome, Limits, ProjectPolicy, Spool};
use mdreader_core::ReaderStore;
use mdreader_service::capture::{configure, drain};

fn policy(root: &std::path::Path) -> ProjectPolicy {
    ProjectPolicy {
        enabled: true,
        project_id: "synthetic-project".into(),
        project_root: root.into(),
        repository_root: root.into(),
        worktree_roots: vec![root.into()],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    }
}
#[test]
#[cfg(unix)]
fn receipt_follows_durable_admission_and_restart_recovers_pending_events() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let grant = policy(&root);
    let mut store = ReaderStore::open(data.path()).unwrap();
    configure(&mut store, data.path(), grant.clone()).unwrap();
    let spool = Spool::open(data.path().join("captures"), Limits::default()).unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"synthetic-session","cwd":root,"last_assistant_message":"# Approved-looking plan\nCapture is not approval."})).unwrap();
    assert!(matches!(
        spool
            .enqueue(Adapter::ClaudeCode, &event, &grant, CaptureOrigin::Live)
            .unwrap(),
        EnqueueOutcome::Queued { .. }
    ));
    drop(store);
    let mut store = ReaderStore::open(data.path()).unwrap();
    let result = drain(&mut store, data.path()).unwrap();
    assert_eq!(result.admitted, 1);
    assert_eq!(store.capture_count().unwrap(), 1);
    assert!(spool.pending(10).unwrap().is_empty());
    let source = store
        .list_sources()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "Agent captures")
        .unwrap();
    assert_eq!(
        store.index_job(&source.id).unwrap().unwrap().phase,
        "queued"
    );
    store.index_source(&source.id).unwrap();
    assert!(!store
        .search("approval", &[source.id], 10)
        .unwrap()
        .is_empty());
    assert_eq!(drain(&mut store, data.path()).unwrap().admitted, 0);
}
#[test]
#[cfg(unix)]
fn replacing_a_project_root_or_revoking_capture_source_leaves_event_unacknowledged() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().join("worktree");
    std::fs::create_dir(&root).unwrap();
    let grant = policy(&root);
    let mut store = ReaderStore::open(data.path()).unwrap();
    configure(&mut store, data.path(), grant.clone()).unwrap();
    let spool = Spool::open(data.path().join("captures"), Limits::default()).unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"s","cwd":root,"last_assistant_message":"# Plan\nDo this"})).unwrap();
    spool
        .enqueue(Adapter::ClaudeCode, &event, &grant, CaptureOrigin::Live)
        .unwrap();
    std::fs::rename(&root, project.path().join("old")).unwrap();
    std::fs::create_dir(&root).unwrap();
    assert_eq!(drain(&mut store, data.path()).unwrap().admitted, 0);
    assert_eq!(store.capture_count().unwrap(), 0);
    assert_eq!(spool.pending(10).unwrap().len(), 1);
    std::fs::remove_dir(&root).unwrap();
    std::fs::rename(project.path().join("old"), &root).unwrap();
    let source = store.list_sources().unwrap().remove(0);
    store.set_source_enabled(&source.id, false).unwrap();
    assert_eq!(drain(&mut store, data.path()).unwrap().admitted, 0);
    assert_eq!(store.capture_count().unwrap(), 0);
    assert_eq!(spool.pending(10).unwrap().len(), 1);
}

#[test]
#[cfg(unix)]
fn policy_artifact_tracks_opt_out_and_tampering_never_grants_capture() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let grant = policy(&project.path().canonicalize().unwrap());
    let mut store = ReaderStore::open(data.path()).unwrap();
    configure(&mut store, data.path(), grant.clone()).unwrap();
    let path = mdreader_service::capture::policy_path(data.path(), &grant.project_id);
    assert!(ProjectPolicy::load(&path).unwrap().enabled);
    let spool = Spool::open(data.path().join("captures"), Limits::default()).unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"s","cwd":grant.project_root,"last_assistant_message":"# Plan\nNot approval"})).unwrap();
    spool
        .enqueue(Adapter::ClaudeCode, &event, &grant, CaptureOrigin::Live)
        .unwrap();
    let mut changed = grant.clone();
    changed.capture_subagents = true;
    std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert_eq!(drain(&mut store, data.path()).unwrap().admitted, 0);
    assert_eq!(spool.pending(10).unwrap().len(), 1);
    let mut disabled = grant;
    disabled.enabled = false;
    configure(&mut store, data.path(), disabled).unwrap();
    assert!(!ProjectPolicy::load(&path).unwrap().enabled);
    assert_eq!(drain(&mut store, data.path()).unwrap().admitted, 0);
    assert_eq!(spool.pending(10).unwrap().len(), 1);
}
