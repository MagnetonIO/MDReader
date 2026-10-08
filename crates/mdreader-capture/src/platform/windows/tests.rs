use super::*;
use fs2::FileExt;

#[test]
fn sqlite_journal_reopens_and_restores_acknowledged_markdown() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("selected-project");
    fs::create_dir(&project).unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "selected".into(),
        project_root: project.clone(),
        repository_root: project.clone(),
        worktree_roots: vec![project],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    let root = temp.path().join("spool");
    let spool = Spool::open(&root, Limits::default()).unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"synthetic-session","turn_id":"turn","cwd":policy.project_root,"last_assistant_message":"# Exact visible output\nNo imported history."})).unwrap();
    let id = match spool
        .enqueue(Adapter::ClaudeCode, &event, &policy, CaptureOrigin::Live)
        .unwrap()
    {
        EnqueueOutcome::Queued { id } => id,
        other => panic!("{other:?}"),
    };
    drop(spool);
    let spool = Spool::open(&root, Limits::default()).unwrap();
    assert_eq!(spool.pending(8).unwrap(), vec![id.clone()]);
    let projected = spool.project(&id, &policy).unwrap();
    let exact = read_bounded(&projected.path, MAX_MARKDOWN_BYTES).unwrap();
    spool.acknowledge(&id).unwrap();
    fs::remove_file(&projected.path).unwrap();
    drop(spool);
    let spool = Spool::open(&root, Limits::default()).unwrap();
    assert!(spool.pending(8).unwrap().is_empty());
    assert_eq!(
        spool.repair_projections(&[policy.clone()], 2).unwrap(),
        vec![policy.project_id.clone()]
    );
    assert_eq!(
        read_bounded(&projected.path, MAX_MARKDOWN_BYTES).unwrap(),
        exact
    );
    assert!(matches!(
        spool
            .enqueue(Adapter::ClaudeCode, &event, &policy, CaptureOrigin::Live)
            .unwrap(),
        EnqueueOutcome::Duplicate { .. }
    ));
    assert!(matches!(
        sync_directory(&root),
        Err(CaptureError::UnsupportedPlatform)
    ));
}

#[test]
fn private_handles_reject_hardlinks_and_lock_contention_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let private = temp.path().join("private");
    private_directory(&private).unwrap();
    // An inherited DACL is rejected instead of silently tightening an existing object.
    let inherited = private.join("inherited");
    fs::create_dir(&inherited).unwrap();
    assert!(matches!(
        private_directory(&inherited),
        Err(CaptureError::UnsafePath)
    ));
    let original = private.join("original");
    let mut file = private_open(&original, true).unwrap();
    file.write_all(b"synthetic visible output").unwrap();
    file.sync_all().unwrap();
    drop(file);
    fs::hard_link(&original, private.join("alias")).unwrap();
    assert!(matches!(
        read_bounded(&original, 64),
        Err(CaptureError::UnsafePath)
    ));

    let lock = private.join("queue.lock");
    let first = private_open(&lock, false).unwrap();
    let second = private_open(&lock, false).unwrap();
    first.try_lock_exclusive().unwrap();
    let error = second.try_lock_exclusive().unwrap_err();
    assert!(lock_would_block(&error));
    FileExt::unlock(&first).unwrap();
    second.try_lock_exclusive().unwrap();
    FileExt::unlock(&second).unwrap();
}

#[test]
fn pinned_namespace_and_handle_relative_replacement_preserve_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let private = temp.path().join("private");
    private_directory(&private).unwrap();
    let chain = authority::Chain::open(&private, false, true).unwrap();
    assert!(fs::rename(&private, temp.path().join("replacement")).is_err());
    chain.verify().unwrap();
    let temporary = private.join("temporary");
    let destination = private.join("published");
    let mut file = private_open(&temporary, true).unwrap();
    file.write_all(b"# Exact synthetic body\n").unwrap();
    replace(&file, &temporary, &destination).unwrap();
    drop(file);
    assert!(!temporary.exists());
    assert_eq!(
        read_bounded(&destination, 64).unwrap(),
        b"# Exact synthetic body\n"
    );
    assert!(matches!(
        read_bounded(&destination, 2),
        Err(CaptureError::TooLarge)
    ));
}
