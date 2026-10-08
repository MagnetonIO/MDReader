use mdreader_capture::{Adapter, CaptureError, ProjectPolicy, backfill::inspect};
use serde_json::json;
fn policy(root: &std::path::Path) -> ProjectPolicy {
    ProjectPolicy {
        enabled: true,
        project_id: "selected-output-test".into(),
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
fn review_returns_only_exact_visible_fields_and_never_invents_missing_turns() {
    let project = tempfile::tempdir().unwrap();
    let policy = policy(&project.path().canonicalize().unwrap());
    for (adapter, event) in [
        (
            Adapter::ClaudeCode,
            json!({"hook_event_name":"Stop","session_id":"chosen","cwd":policy.project_root,
        "last_assistant_message":"# Visible\nExact **body**.","thinking":"HIDDEN-SECRET","tools":["TOOL-SECRET"],"transcript_path":"/unavailable/never-read"}),
        ),
        (
            Adapter::CodexNotify,
            json!({"type":"agent-turn-complete","thread-id":"chosen","cwd":policy.project_root,
        "last-assistant-message":"# Visible\nExact **body**.","reasoning":"HIDDEN-SECRET","turn-id":"real-turn"}),
        ),
    ] {
        let output = inspect(adapter, &serde_json::to_vec(&event).unwrap(), &policy).unwrap();
        assert_eq!(output.body, "# Visible\nExact **body**.");
        assert_eq!(output.session_id, "chosen");
        assert_eq!(
            output.turn_id,
            if adapter == Adapter::CodexNotify {
                Some("real-turn".into())
            } else {
                None
            }
        );
        assert!(!output.canonical_event.contains("SECRET"));
        assert!(!output.canonical_event.contains("transcript"));
        let second = inspect(adapter, output.canonical_event.as_bytes(), &policy).unwrap();
        assert_eq!(output.canonical_event, second.canonical_event);
        assert!(policy.selected_backfill_sessions.is_empty());
    }
}
#[test]
fn review_rejects_nonfinal_hidden_out_of_scope_disabled_and_oversized_inputs() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut policy = policy(&project.path().canonicalize().unwrap());
    let event = |cwd: &std::path::Path, kind: &str, body: &str| {
        serde_json::to_vec(&json!({"hook_event_name":kind,"session_id":"chosen","cwd":cwd,"last_assistant_message":body})).unwrap()
    };
    assert!(
        inspect(
            Adapter::CodexHook,
            &event(outside.path(), "Stop", "visible"),
            &policy
        )
        .is_err()
    );
    assert!(
        inspect(
            Adapter::CodexHook,
            &event(project.path(), "ToolEnd", "visible"),
            &policy
        )
        .is_err()
    );
    assert!(
        inspect(
            Adapter::CodexHook,
            &event(project.path(), "Stop", "<analysis>hidden</analysis>"),
            &policy
        )
        .is_err()
    );
    assert!(inspect(Adapter::ClaudeCode, &vec![b' '; 256 * 1024 + 1], &policy).is_err());
    policy.enabled = false;
    assert!(matches!(
        inspect(
            Adapter::ClaudeCode,
            &event(project.path(), "Stop", "visible"),
            &policy
        ),
        Err(CaptureError::Excluded(_))
    ));
}
