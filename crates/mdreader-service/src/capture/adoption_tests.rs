use super::*;
use std::fs;
fn fixture() -> (tempfile::TempDir, tempfile::TempDir, String) {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "upgrade-capture".into(),
        project_root: root.clone(),
        repository_root: root.clone(),
        worktree_roots: vec![root],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    let mut store = ReaderStore::open(data.path()).unwrap();
    configure(&mut store, data.path(), policy.clone()).unwrap();
    let spool = Spool::open(data.path().join("captures"), Limits::default()).unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"upgrade-session","cwd":policy.project_root,"last_assistant_message":"# Captured\nupgradecapturecedar"})).unwrap();
    spool
        .enqueue(
            mdreader_capture::Adapter::ClaudeCode,
            &event,
            &policy,
            mdreader_capture::CaptureOrigin::Live,
        )
        .unwrap();
    drain(&mut store, data.path()).unwrap();
    let source = reader::sources(&store, data.path()).unwrap()[0].id.clone();
    store.index_source(&source).unwrap();
    // A prior client has real capture grant/artifact/index but no explicit
    // ownership table entry. Initialization must not infer exemption itself.
    store
        .with_library_connection(|db| db.execute("DELETE FROM reader_local_owned_v1", []))
        .unwrap();
    drop(store);
    (data, project, source)
}
#[test]
fn prior_capture_owner_adoption_requires_verified_grant_artifact_and_spool() {
    let (data, _project, source) = fixture();
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert!(store
        .search("upgradecapturecedar", std::slice::from_ref(&source), 10)
        .is_err());
    adopt_verified_sources(&mut store, data.path()).unwrap();
    assert_eq!(
        store
            .search("upgradecapturecedar", std::slice::from_ref(&source), 10)
            .unwrap()
            .len(),
        1
    );
    drop(store);
    let store = ReaderStore::open_readonly(data.path()).unwrap();
    assert_eq!(
        store
            .search("upgradecapturecedar", &[source], 10)
            .unwrap()
            .len(),
        1
    );
    let (data, _project, source) = fixture();
    let mut store = ReaderStore::open(data.path()).unwrap();
    fs::write(policy_path(data.path(), "upgrade-capture"), "{}").unwrap();
    adopt_verified_sources(&mut store, data.path()).unwrap();
    assert!(store
        .search("upgradecapturecedar", &[source], 10)
        .is_err());
}
