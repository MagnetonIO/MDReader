use super::*;
use crate::capture::backfill::BackfillRequest;
use std::time::Instant;
use uuid::Uuid;

#[test]
fn actual_runtime_capture_and_integration_paths_are_denied_while_frozen() {
    let directory = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(ReaderStore::open(directory.path()).unwrap()));
    let runtime = Runtime::new(
        owner.clone(),
        directory.path().join("model"),
        crate::ServiceOptions {
            disable_model: true,
        },
    )
    .unwrap();
    let root = project.path().canonicalize().unwrap();
    let policy = mdreader_capture::ProjectPolicy {
        enabled: true,
        project_id: "freeze-test".into(),
        project_root: root.clone(),
        repository_root: root.clone(),
        worktree_roots: vec![root],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    runtime
        .execute(Request::ConfigureCapture {
            policy: policy.clone(),
        })
        .unwrap();
    let admission = runtime.capture_participant().unwrap();
    let operation = Uuid::new_v4();
    let epoch = Uuid::new_v4();
    let frozen = admission
        .freeze(operation, epoch, Instant::now() + Duration::from_secs(2))
        .unwrap();
    // Reader/index scheduling is a separate participant and may still briefly
    // hold this mutex. This capability only denies capture/integration effects.
    let before =
        std::fs::read(crate::capture::policy_path(directory.path(), "freeze-test")).unwrap();
    let mut disabled = policy.clone();
    disabled.enabled = false;
    assert!(runtime
        .execute(Request::ConfigureCapture { policy: disabled })
        .is_err());
    assert!(runtime
        .execute(Request::CaptureBackfill {
            project_id: "freeze-test".into(),
            request: BackfillRequest::Deselect {
                session_id: "unselected".into()
            }
        })
        .is_err());
    assert!(runtime
        .execute(Request::UninstallIntegration {
            preparation_id: Uuid::new_v4().to_string()
        })
        .is_err());
    assert_eq!(
        std::fs::read(crate::capture::policy_path(directory.path(), "freeze-test")).unwrap(),
        before
    );
    frozen
        .revalidate(admission, &owner, operation, epoch)
        .unwrap();
    drop(frozen);
    runtime
        .execute(Request::ConfigureCapture { policy })
        .unwrap();
}
