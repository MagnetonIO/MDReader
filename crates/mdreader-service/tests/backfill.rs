#![cfg(unix)]
use mdreader_capture::{Adapter, CaptureOrigin, EnqueueOutcome, Limits, ProjectPolicy, Spool};
use mdreader_core::ReaderStore;
use mdreader_service::capture::{
    self,
    backfill::{BackfillReply, BackfillRequest, BackfillReview},
};
use serde_json::json;
struct Fixture {
    _project: tempfile::TempDir,
    data: tempfile::TempDir,
    root: std::path::PathBuf,
    store: ReaderStore,
    event: String,
}
impl Fixture {
    fn new() -> Self {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let root = project.path().join("worktree");
        std::fs::create_dir(&root).unwrap();
        let policy = ProjectPolicy {
            enabled: true,
            project_id: "selected-test".into(),
            project_root: root.clone(),
            repository_root: root.clone(),
            worktree_roots: vec![root.clone()],
            capture_subagents: false,
            excluded_paths: vec![],
            excluded_sessions: vec![],
            selected_backfill_sessions: vec![],
        };
        let mut store = ReaderStore::open(data.path()).unwrap();
        capture::configure(&mut store, data.path(), policy).unwrap();
        let event=json!({"hook_event_name":"Stop","session_id":"selected-session","cwd":root,"last_assistant_message":"# Public synthetic plan\nExact visible output.","reasoning":"NEVER-PERSIST","transcript_path":"/not-read"}).to_string();
        Self {
            _project: project,
            data,
            root,
            store,
            event,
        }
    }
    fn call(&mut self, request: BackfillRequest) -> mdreader_core::Result<BackfillReply> {
        capture::backfill::execute(&mut self.store, self.data.path(), "selected-test", request)
    }
    fn review(&mut self) -> BackfillReview {
        let request = BackfillRequest::Review {
            adapter: Adapter::ClaudeCode,
            event: self.event.clone(),
        };
        match self.call(request).unwrap() {
            BackfillReply::Reviewed { value } => value,
            _ => panic!("review expected"),
        }
    }
    fn enqueue(&mut self, review: &BackfillReview) -> mdreader_core::Result<BackfillReply> {
        self.call(BackfillRequest::Enqueue {
            adapter: Adapter::ClaudeCode,
            event: review.canonical_event.clone(),
            review_digest: review.review_digest.clone(),
            session_id: review.session_id.clone(),
        })
    }
    fn spool(&self) -> Spool {
        Spool::open(self.data.path().join("captures"), Limits::default()).unwrap()
    }
}
#[test]
fn selected_output_queue_replays_across_restart_and_admits_exact_provenance() {
    let mut fixture = Fixture::new();
    let reviewed = fixture.review();
    assert_eq!(reviewed.turn_id, None);
    assert!(!reviewed.selected);
    assert!(!reviewed.canonical_event.contains("NEVER-PERSIST"));
    assert!(fixture.spool().pending(10).unwrap().is_empty());
    assert!(matches!(
        fixture.enqueue(&reviewed).unwrap(),
        BackfillReply::Enqueued {
            outcome: EnqueueOutcome::Queued { .. }
        }
    ));
    drop(fixture.store);
    fixture.store = ReaderStore::open(fixture.data.path()).unwrap();
    assert!(matches!(
        fixture.enqueue(&reviewed).unwrap(),
        BackfillReply::Enqueued {
            outcome: EnqueueOutcome::Duplicate { .. }
        }
    ));
    let spool = fixture.spool();
    let pending = spool.pending(10).unwrap();
    assert_eq!(pending.len(), 1);
    let record = spool.read_pending(&pending[0]).unwrap();
    assert_eq!(record.body, reviewed.body);
    assert_eq!(record.provenance.origin, CaptureOrigin::SelectedBackfill);
    assert_eq!(record.provenance.turn_id, None);
    assert_eq!(record.provenance.session_id, "selected-session");
    assert_eq!(
        capture::drain(&mut fixture.store, fixture.data.path())
            .unwrap()
            .admitted,
        1
    );
    assert_eq!(fixture.store.capture_count().unwrap(), 1);
    let projected = std::fs::read_to_string(
        spool
            .markdown_root()
            .join(format!("{}.md", record.content_id)),
    )
    .unwrap();
    assert!(projected.ends_with(&record.body));
    assert!(!projected.contains("NEVER-PERSIST"));
    assert!(matches!(
        fixture.enqueue(&reviewed).unwrap(),
        BackfillReply::Enqueued {
            outcome: EnqueueOutcome::Duplicate { .. }
        }
    ));
}
#[test]
fn wrong_review_policy_change_replaced_root_and_revoked_source_never_regrant() {
    let mut fixture = Fixture::new();
    let mut reviewed = fixture.review();
    let original = fixture.store.capture_policies().unwrap();
    assert!(!format!(
        "{:?}",
        BackfillRequest::Review {
            adapter: Adapter::ClaudeCode,
            event: fixture.event.clone()
        }
    )
    .contains("NEVER-PERSIST"));
    reviewed.review_digest = "0".repeat(64);
    assert!(fixture.enqueue(&reviewed).is_err());
    reviewed = fixture.review();
    let policy_path = capture::policy_path(fixture.data.path(), "selected-test");
    let mut changed_policy = ProjectPolicy::load(&policy_path).unwrap();
    changed_policy.capture_subagents = true;
    capture::configure(
        &mut fixture.store,
        fixture.data.path(),
        changed_policy.clone(),
    )
    .unwrap();
    assert!(fixture.enqueue(&reviewed).is_err());
    changed_policy.capture_subagents = false;
    capture::configure(&mut fixture.store, fixture.data.path(), changed_policy).unwrap();
    reviewed = fixture.review();
    let source = fixture.store.list_sources().unwrap().remove(0);
    fixture.store.set_source_enabled(&source.id, false).unwrap();
    assert!(fixture.enqueue(&reviewed).is_err());
    fixture.store.set_source_enabled(&source.id, true).unwrap();
    std::fs::rename(&fixture.root, fixture._project.path().join("old")).unwrap();
    std::fs::create_dir(&fixture.root).unwrap();
    assert!(fixture.enqueue(&reviewed).is_err());
    assert_eq!(fixture.store.capture_policies().unwrap(), original);
    assert!(fixture.spool().pending(10).unwrap().is_empty());
}
#[test]
fn deselection_remains_possible_when_root_missing_and_retains_pending_recovery() {
    let mut fixture = Fixture::new();
    let reviewed = fixture.review();
    fixture.enqueue(&reviewed).unwrap();
    let original: serde_json::Value =
        serde_json::from_str(&fixture.store.capture_policies().unwrap()[0].1).unwrap();
    std::fs::rename(&fixture.root, fixture._project.path().join("old")).unwrap();
    assert!(matches!(
        fixture
            .call(BackfillRequest::Deselect {
                session_id: reviewed.session_id
            })
            .unwrap(),
        BackfillReply::Deselected { .. }
    ));
    let changed: serde_json::Value =
        serde_json::from_str(&fixture.store.capture_policies().unwrap()[0].1).unwrap();
    assert_eq!(original["roots"], changed["roots"]);
    assert_eq!(original["projection_root"], changed["projection_root"]);
    assert_eq!(changed["policy"]["selected_backfill_sessions"], json!([]));
    std::fs::rename(fixture._project.path().join("old"), &fixture.root).unwrap();
    assert_eq!(
        capture::drain(&mut fixture.store, fixture.data.path())
            .unwrap()
            .admitted,
        0
    );
    assert_eq!(fixture.spool().pending(10).unwrap().len(), 1);
    assert_eq!(fixture.store.capture_count().unwrap(), 0);
}

#[test]
fn exact_review_replay_recovers_only_its_candidate_policy_artifact_after_interrupted_publication() {
    let mut fixture = Fixture::new();
    let reviewed = fixture.review();
    let original = fixture.store.capture_policies().unwrap();
    let path = capture::policy_path(fixture.data.path(), "selected-test");
    let policy = ProjectPolicy::load(&path).unwrap();
    // An unrelated artifact mismatch must never be overwritten by a replay.
    let mut unrelated = policy.clone();
    unrelated
        .selected_backfill_sessions
        .push("another-session".into());
    std::fs::write(&path, serde_json::to_vec(&unrelated).unwrap()).unwrap();
    assert!(fixture.enqueue(&reviewed).is_err());
    assert_eq!(fixture.store.capture_policies().unwrap(), original);
    // Simulate publication durable on disk, before its SQLite selection save.
    let mut candidate = policy;
    candidate
        .selected_backfill_sessions
        .push(reviewed.session_id.clone());
    std::fs::write(&path, serde_json::to_vec(&candidate).unwrap()).unwrap();
    assert!(fixture
        .call(BackfillRequest::Review {
            adapter: Adapter::ClaudeCode,
            event: fixture.event.clone()
        })
        .is_err());
    assert!(matches!(
        fixture.enqueue(&reviewed).unwrap(),
        BackfillReply::Enqueued {
            outcome: EnqueueOutcome::Queued { .. }
        }
    ));
    let saved: serde_json::Value =
        serde_json::from_str(&fixture.store.capture_policies().unwrap()[0].1).unwrap();
    let prior: serde_json::Value = serde_json::from_str(&original[0].1).unwrap();
    assert_eq!(saved["roots"], prior["roots"]);
    assert_eq!(saved["projection_root"], prior["projection_root"]);
    assert_eq!(saved["source_id"], prior["source_id"]);
    assert_eq!(
        saved["policy"]["selected_backfill_sessions"],
        json!(["selected-session"])
    );
    assert_eq!(fixture.spool().pending(10).unwrap().len(), 1);
}

#[test]
fn stale_settings_cannot_restore_deselected_session_or_recapture_a_replaced_root() {
    let mut fixture = Fixture::new();
    let reviewed = fixture.review();
    fixture.enqueue(&reviewed).unwrap();
    let path = capture::policy_path(fixture.data.path(), "selected-test");
    // Simulate native settings reading its policy before the user deselects.
    let mut stale_settings = ProjectPolicy::load(&path).unwrap();
    fixture
        .call(BackfillRequest::Deselect {
            session_id: reviewed.session_id,
        })
        .unwrap();
    let current = fixture.store.capture_policies().unwrap();
    let artifact = std::fs::read(&path).unwrap();
    let sources = serde_json::to_value(fixture.store.list_sources().unwrap()).unwrap();
    // Replacing the directory additionally proves rejection precedes fresh
    // authority capture, rather than merely blocking projection afterward.
    std::fs::rename(&fixture.root, fixture._project.path().join("original")).unwrap();
    std::fs::create_dir(&fixture.root).unwrap();
    stale_settings.capture_subagents = true;
    assert!(matches!(
        capture::configure(&mut fixture.store, fixture.data.path(), stale_settings),
        Err(mdreader_core::ReaderError::Conflict)
    ));
    assert_eq!(fixture.store.capture_policies().unwrap(), current);
    assert_eq!(std::fs::read(&path).unwrap(), artifact);
    assert_eq!(
        serde_json::to_value(fixture.store.list_sources().unwrap()).unwrap(),
        sources
    );
    assert_eq!(fixture.spool().pending(10).unwrap().len(), 1);
    assert_eq!(fixture.store.capture_count().unwrap(), 0);
}
