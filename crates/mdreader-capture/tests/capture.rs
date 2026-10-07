use mdreader_capture::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

fn fixture() -> (TempDir, Spool, ProjectPolicy) {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "synthetic-project".into(),
        project_root: project.clone(),
        repository_root: project.clone(),
        worktree_roots: vec![project],
        capture_subagents: true,
        excluded_paths: vec!["private".into()],
        excluded_sessions: vec!["excluded-session".into()],
        selected_backfill_sessions: vec!["chosen-session".into()],
    };
    let spool = Spool::open(temp.path().join("capture"), Limits::default()).unwrap();
    (temp, spool, policy)
}
fn event(policy: &ProjectPolicy, session: &str, body: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"hook_event_name":"Stop","session_id":session,
        "turn_id":"turn-1", "prompt_id":"turn-1", "cwd":policy.project_root,"last_assistant_message":body,
        "transcript_path":"/DO-NOT-READ/private-history", "reasoning":"HIDDEN-SECRET",
        "tool_input":{"password":"TOOL-SECRET"}})).unwrap()
}
fn queued(outcome: EnqueueOutcome) -> String {
    match outcome {
        EnqueueOutcome::Queued { id } => id,
        other => panic!("{other:?}"),
    }
}
#[test]
fn durable_restart_projection_and_ack_preserve_markdown_and_provenance() {
    let (temp, spool, policy) = fixture();
    let body = "# A plan\n\nKeep the plan after index rebuild.\n";
    let id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "session-1", body),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    drop(spool);
    let reopened = Spool::open(temp.path().join("capture"), Limits::default()).unwrap();
    assert_eq!(reopened.pending(10).unwrap(), vec![id.clone()]);
    let projected = reopened.project(&id, &policy).unwrap();
    let text = fs::read_to_string(&projected.path).unwrap();
    assert!(text.ends_with(body));
    assert!(text.contains("session-1"));
    assert!(text.contains("turn-1"));
    assert!(!text.contains("HIDDEN-SECRET"));
    assert!(!text.contains("TOOL-SECRET"));
    assert!(!text.contains("DO-NOT-READ"));
    assert_eq!(reopened.pending(10).unwrap().len(), 1);
    reopened.acknowledge(&id).unwrap();
    assert!(reopened.pending(10).unwrap().is_empty());
    assert_eq!(fs::read_to_string(projected.path).unwrap(), text);
    assert!(matches!(
        reopened
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "session-1", body),
                &policy,
                CaptureOrigin::Live
            )
            .unwrap(),
        EnqueueOutcome::Duplicate { .. }
    ));
}
#[test]
fn crash_after_projection_retry_and_distinct_fork_subagent_provenance() {
    let (_temp, spool, policy) = fixture();
    let body = "# Shared visible plan\nDurable body.";
    let id = queued(
        spool
            .enqueue(
                Adapter::CodexHook,
                &event(&policy, "root-session", body),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let first = spool.project(&id, &policy).unwrap();
    let retry = spool.project(&id, &policy).unwrap();
    assert_eq!(first.path, retry.path);
    assert_eq!(
        read_projection(&first.path, &Limits::default())
            .unwrap()
            .occurrences
            .len(),
        1
    );
    spool.acknowledge(&id).unwrap();
    let mut fork: serde_json::Value =
        serde_json::from_slice(&event(&policy, "fork-session", body)).unwrap();
    fork["hook_event_name"] = "SubagentStop".into();
    fork["agent_id"] = "child-7".into();
    fork["agent_type"] = "Plan".into();
    let child_id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &serde_json::to_vec(&fork).unwrap(),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let child = spool.project(&child_id, &policy).unwrap();
    assert_eq!(first.path, child.path);
    let metadata = read_projection(&child.path, &Limits::default()).unwrap();
    assert_eq!(metadata.occurrences.len(), 2);
    assert_eq!(
        metadata.occurrences[1].subagent_id.as_deref(),
        Some("child-7")
    );
    assert_eq!(metadata.occurrences[1].session_id, "fork-session");
    assert_eq!(metadata.content_revision, child.record.content_revision);
    assert_eq!(spool.pending(10).unwrap(), vec![child_id]);
}
#[test]
fn strict_opt_in_scope_exclusions_and_selected_backfill() {
    let (_temp, spool, mut policy) = fixture();
    let input = event(&policy, "session-1", "# Plan");
    policy.enabled = false;
    assert!(matches!(
        spool
            .enqueue(Adapter::ClaudeCode, &input, &policy, CaptureOrigin::Live)
            .unwrap(),
        EnqueueOutcome::Skipped {
            reason: SkipReason::ProjectDisabled
        }
    ));
    policy.enabled = true;
    assert!(matches!(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &input,
                &policy,
                CaptureOrigin::SelectedBackfill
            )
            .unwrap(),
        EnqueueOutcome::Skipped {
            reason: SkipReason::BackfillNotSelected
        }
    ));
    queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "chosen-session", "Selected historical output"),
                &policy,
                CaptureOrigin::SelectedBackfill,
            )
            .unwrap(),
    );
    assert!(matches!(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "excluded-session", "Excluded"),
                &policy,
                CaptureOrigin::Live
            )
            .unwrap(),
        EnqueueOutcome::Skipped {
            reason: SkipReason::SessionExcluded
        }
    ));
    let private = policy.project_root.join("private");
    fs::create_dir(&private).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&input).unwrap();
    value["cwd"] = json!(private);
    assert!(matches!(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &serde_json::to_vec(&value).unwrap(),
                &policy,
                CaptureOrigin::Live
            )
            .unwrap(),
        EnqueueOutcome::Skipped {
            reason: SkipReason::PathExcluded
        }
    ));
    value["cwd"] = json!(policy.project_root.parent().unwrap());
    assert!(matches!(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &serde_json::to_vec(&value).unwrap(),
                &policy,
                CaptureOrigin::Live
            )
            .unwrap(),
        EnqueueOutcome::Skipped {
            reason: SkipReason::OutsideProject
        }
    ));
}
#[test]
fn tool_logs_hidden_reasoning_and_mcp_echoes_are_not_captured() {
    let (_temp, spool, policy) = fixture();
    let mut value: serde_json::Value =
        serde_json::from_slice(&event(&policy, "session", "visible")).unwrap();
    for name in [
        "PostToolUse",
        "PreToolUse",
        "SessionStart",
        "PostCompact",
        "StopFailure",
        "MessageDisplay",
    ] {
        value["hook_event_name"] = name.into();
        assert!(matches!(
            spool
                .enqueue(
                    Adapter::ClaudeCode,
                    &serde_json::to_vec(&value).unwrap(),
                    &policy,
                    CaptureOrigin::Live
                )
                .unwrap(),
            EnqueueOutcome::Skipped {
                reason: SkipReason::UnsupportedEvent
            }
        ));
    }
    for body in [
        mark_retrieval("A retrieved plan"),
        "<think>private reasoning</think>".into(),
        "<!-- mdreader-capture:v1 -->\nA copied projection".into(),
    ] {
        assert!(matches!(
            spool
                .enqueue(
                    Adapter::ClaudeCode,
                    &event(&policy, "session", &body),
                    &policy,
                    CaptureOrigin::Live
                )
                .unwrap(),
            EnqueueOutcome::Skipped { .. }
        ));
    }
    assert!(spool.pending(10).unwrap().is_empty());
}
#[test]
fn queue_bounds_and_revalidation_never_silently_truncate() {
    let (temp, _, mut policy) = fixture();
    let limits = Limits {
        max_pending_events: 1,
        max_event_bytes: 1024,
        ..Limits::default()
    };
    let spool = Spool::open(temp.path().join("bounded"), limits).unwrap();
    let id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "one", "first"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    assert!(matches!(
        spool.enqueue(
            Adapter::ClaudeCode,
            &event(&policy, "two", "second"),
            &policy,
            CaptureOrigin::Live
        ),
        Err(CaptureError::QueueFull)
    ));
    assert!(matches!(
        spool.enqueue(
            Adapter::ClaudeCode,
            &event(&policy, "three", &"x".repeat(2048)),
            &policy,
            CaptureOrigin::Live
        ),
        Err(CaptureError::TooLarge)
    ));
    policy.enabled = false;
    assert!(matches!(
        spool.project(&id, &policy),
        Err(CaptureError::Excluded(SkipReason::ProjectDisabled))
    ));
    assert_eq!(spool.pending(10).unwrap(), vec![id]);
}
#[test]
fn codex_notify_is_visible_output_only_and_missing_turn_is_explicit() {
    let (_temp, spool, policy) = fixture();
    let bytes = serde_json::to_vec(&json!({"type":"agent-turn-complete","thread-id":"notify-session","turn-id":"notify-turn","cwd":policy.project_root,"last-assistant-message":"# Notify plan","input-messages":["PRIVATE PROMPT"]})).unwrap();
    let id = queued(
        spool
            .enqueue(Adapter::CodexNotify, &bytes, &policy, CaptureOrigin::Live)
            .unwrap(),
    );
    let record = spool.read_pending(&id).unwrap();
    assert_eq!(record.provenance.turn_id.as_deref(), Some("notify-turn"));
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("PRIVATE PROMPT"));
    let mut value: serde_json::Value =
        serde_json::from_slice(&event(&policy, "claude", "# Claude")).unwrap();
    value.as_object_mut().unwrap().remove("turn_id");
    value.as_object_mut().unwrap().remove("prompt_id");
    let id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &serde_json::to_vec(&value).unwrap(),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    assert_eq!(spool.read_pending(&id).unwrap().provenance.turn_id, None);
}

#[test]
fn corrupt_pending_or_receipt_cannot_be_acknowledged_as_duplicate() {
    let (_temp, spool, policy) = fixture();
    let bytes = event(
        &policy,
        "session",
        "# Never claim a truncated capture is durable",
    );
    let id = queued(
        spool
            .enqueue(Adapter::ClaudeCode, &bytes, &policy, CaptureOrigin::Live)
            .unwrap(),
    );
    let pending = spool.root().join("pending").join(format!("{id}.json"));
    let intact = fs::read(&pending).unwrap();
    fs::write(&pending, b"{partial").unwrap();
    assert!(matches!(
        spool.enqueue(Adapter::ClaudeCode, &bytes, &policy, CaptureOrigin::Live),
        Err(CaptureError::Invalid)
    ));
    assert!(spool.acknowledge(&id).is_err());
    fs::write(&pending, intact).unwrap();
    spool.project(&id, &policy).unwrap();
    spool.acknowledge(&id).unwrap();
    fs::write(spool.root().join("receipts").join(&id), b"damaged").unwrap();
    assert!(matches!(
        spool.enqueue(Adapter::ClaudeCode, &bytes, &policy, CaptureOrigin::Live),
        Err(CaptureError::Invalid)
    ));
}
#[test]
fn nonblocking_lock_and_interrupted_write_recovery_keep_ready_events() {
    use fs2::FileExt;
    let (_temp, spool, policy) = fixture();
    let id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "session", "# Ready"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(spool.root().join("queue.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let started = std::time::Instant::now();
    assert!(matches!(
        spool.enqueue(
            Adapter::ClaudeCode,
            &event(&policy, "second", "# Busy"),
            &policy,
            CaptureOrigin::Live
        ),
        Err(CaptureError::Busy)
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    drop(lock);
    fs::write(
        spool.root().join("temporary/interrupted.tmp"),
        b"{incomplete",
    )
    .unwrap();
    assert_eq!(spool.status().unwrap().orphan_temporary_files, 1);
    assert_eq!(spool.recover_temporary().unwrap(), 1);
    assert_eq!(spool.pending(10).unwrap(), vec![id.clone()]);
    assert!(matches!(
        spool.acknowledge(&id),
        Err(CaptureError::NotProjected)
    ));
    spool.project(&id, &policy).unwrap();
    // Simulate a crash after durable receipt creation and before queue unlink.
    let record = spool.read_pending(&id).unwrap();
    fs::write(spool.root().join("receipts").join(&id), record.content_id).unwrap();
    spool.acknowledge(&id).unwrap();
    spool.acknowledge(&id).unwrap();
    assert!(spool.pending(10).unwrap().is_empty());
}
#[cfg(unix)]
#[test]
fn symlink_escape_and_spool_replacement_are_rejected() {
    use std::os::unix::fs::symlink;
    let (temp, spool, policy) = fixture();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, policy.project_root.join("escape")).unwrap();
    let mut input: serde_json::Value =
        serde_json::from_slice(&event(&policy, "session", "# Visible")).unwrap();
    input["cwd"] = json!(policy.project_root.join("escape"));
    assert!(matches!(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &serde_json::to_vec(&input).unwrap(),
                &policy,
                CaptureOrigin::Live
            )
            .unwrap(),
        EnqueueOutcome::Skipped {
            reason: SkipReason::OutsideProject
        }
    ));
    let id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "session", "# Visible"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let record = spool.read_pending(&id).unwrap();
    let victim = outside.join("victim.md");
    fs::write(&victim, "private sentinel").unwrap();
    symlink(
        &victim,
        spool
            .markdown_root()
            .join(format!("{}.md", record.content_id)),
    )
    .unwrap();
    assert!(matches!(
        spool.project(&id, &policy),
        Err(CaptureError::UnsafePath)
    ));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "private sentinel");
    fs::rename(spool.root(), temp.path().join("old-spool")).unwrap();
    fs::create_dir_all(spool.root()).unwrap();
    assert!(spool.pending(10).is_err());
}
#[test]
fn body_revision_changes_and_projects_are_isolated() {
    let (_temp, spool, policy) = fixture();
    let a = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "session", "# Version one"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let b = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "session", "# Version two"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let a = spool.project(&a, &policy).unwrap();
    let b = spool.project(&b, &policy).unwrap();
    assert_ne!(a.path, b.path);
    assert_ne!(a.record.content_revision, b.record.content_revision);
    let mut other = policy.clone();
    other.project_id = "another-project".into();
    assert!(spool.project(&a.id, &other).is_err());
    let id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&other, "session", "# Version one"),
                &other,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    assert_ne!(a.path, spool.project(&id, &other).unwrap().path);
}
#[test]
fn provenance_growth_has_an_explicit_bound_without_erasing_prior_authorship() {
    let (temp, _, policy) = fixture();
    let limits = Limits {
        max_occurrences_per_body: 1,
        ..Limits::default()
    };
    let spool = Spool::open(temp.path().join("provenance-bound"), limits).unwrap();
    let a = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "one", "Same body"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let original = spool.project(&a, &policy).unwrap();
    spool.acknowledge(&a).unwrap();
    let b = queued(
        spool
            .enqueue(
                Adapter::CodexHook,
                &event(&policy, "two", "Same body"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    assert!(matches!(
        spool.project(&b, &policy),
        Err(CaptureError::ProvenanceFull)
    ));
    assert_eq!(
        read_projection(&original.path, spool.limits())
            .unwrap()
            .occurrences[0]
            .session_id,
        "one"
    );
    assert_eq!(spool.pending(10).unwrap(), vec![b]);
}

#[test]
fn hook_and_legacy_notify_dedupe_the_same_codex_authorship() {
    let (_temp, spool, policy) = fixture();
    let hook = event(&policy, "codex-session", "# One Codex turn");
    let id = queued(
        spool
            .enqueue(Adapter::CodexHook, &hook, &policy, CaptureOrigin::Live)
            .unwrap(),
    );
    let notify=serde_json::to_vec(&json!({"type":"agent-turn-complete","thread-id":"codex-session","turn-id":"turn-1","cwd":policy.project_root,"last-assistant-message":"# One Codex turn"})).unwrap();
    assert_eq!(
        spool
            .enqueue(Adapter::CodexNotify, &notify, &policy, CaptureOrigin::Live)
            .unwrap(),
        EnqueueOutcome::Duplicate { id: id.clone() }
    );
    let capture = spool.project(&id, &policy).unwrap();
    assert_eq!(
        read_projection(&capture.path, spool.limits())
            .unwrap()
            .occurrences
            .len(),
        1
    );
}
#[test]
fn queue_byte_bound_and_explicit_subagent_exclusion() {
    let (temp, _, mut policy) = fixture();
    let spool = Spool::open(
        temp.path().join("byte-bound"),
        Limits {
            max_pending_bytes: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        spool.enqueue(
            Adapter::ClaudeCode,
            &event(&policy, "session", "body"),
            &policy,
            CaptureOrigin::Live
        ),
        Err(CaptureError::QueueFull)
    ));
    assert_eq!(spool.status().unwrap().pending_events, 0);
    policy.capture_subagents = false;
    let mut child: serde_json::Value =
        serde_json::from_slice(&event(&policy, "session", "child output")).unwrap();
    child["hook_event_name"] = "SubagentStop".into();
    child["agent_id"] = "child".into();
    assert!(matches!(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &serde_json::to_vec(&child).unwrap(),
                &policy,
                CaptureOrigin::Live
            )
            .unwrap(),
        EnqueueOutcome::Skipped {
            reason: SkipReason::SubagentExcluded
        }
    ));
}

#[test]
fn generated_markdown_is_bounded_by_reader_document_admission() {
    assert!(Limits::default().max_projection_bytes <= 2 * 1024 * 1024);
    let (temp, spool, policy) = fixture();
    let id = queued(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &event(&policy, "one", "# Bounded projection"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    let projection = spool.project(&id, &policy).unwrap();
    let projection_bytes = fs::read(&projection.path).unwrap();
    let pending_bytes = fs::read(spool.root().join("pending").join(format!("{id}.json"))).unwrap();
    spool.acknowledge(&id).unwrap();
    let limits = Limits {
        max_event_bytes: pending_bytes.len() + 64,
        max_projection_bytes: (projection_bytes.len() + 100).max(pending_bytes.len() + 64),
        ..Limits::default()
    };
    drop(spool);
    let spool = Spool::open(temp.path().join("capture"), limits).unwrap();
    let second = queued(
        spool
            .enqueue(
                Adapter::CodexHook,
                &event(&policy, "two", "# Bounded projection"),
                &policy,
                CaptureOrigin::Live,
            )
            .unwrap(),
    );
    assert!(matches!(
        spool.project(&second, &policy),
        Err(CaptureError::TooLarge)
    ));
    assert_eq!(fs::read(projection.path).unwrap(), projection_bytes);
    assert_eq!(spool.pending(10).unwrap(), vec![second]);
}

#[test]
fn durable_receipt_never_masks_missing_authoritative_markdown() {
    let (_temp, spool, policy) = fixture();
    let bytes = event(&policy, "session", "# Authoritative body");
    let id = queued(
        spool
            .enqueue(Adapter::ClaudeCode, &bytes, &policy, CaptureOrigin::Live)
            .unwrap(),
    );
    let projection = spool.project(&id, &policy).unwrap();
    spool.acknowledge(&id).unwrap();
    fs::remove_file(projection.path).unwrap();
    assert!(matches!(
        spool.enqueue(Adapter::ClaudeCode, &bytes, &policy, CaptureOrigin::Live),
        Err(CaptureError::NotProjected)
    ));
}
