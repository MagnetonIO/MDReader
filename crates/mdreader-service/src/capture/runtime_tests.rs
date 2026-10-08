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
    let scope: crate::catalog::ReaderOwnedSources =
        serde_json::from_value(runtime.execute(Request::ReaderOwnedSources).unwrap()).unwrap();
    assert_eq!(scope.captures.len(), 1);
    let source_id = scope.captures[0].id.clone();
    let spool = mdreader_capture::Spool::open(
        directory.path().join("captures"),
        mdreader_capture::Limits::default(),
    )
    .unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"reader-scope","cwd":policy.project_root,"last_assistant_message":"# Captured plan\nquartz scope evidence"})).unwrap();
    spool
        .enqueue(
            mdreader_capture::Adapter::ClaudeCode,
            &event,
            &policy,
            mdreader_capture::CaptureOrigin::Live,
        )
        .unwrap();
    {
        let mut store = owner.lock().unwrap();
        crate::capture::drain(&mut store, directory.path()).unwrap();
        assert_eq!(store.capture_count().unwrap(), 1);
        store.index_source(&source_id).unwrap();
    }
    let listed = runtime
        .execute(Request::Catalog {
            request: crate::catalog::CatalogRequest::List {
                local_sources: vec![],
            },
        })
        .unwrap();
    assert!(listed["result"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["kind"] == "capture"));
    let page = runtime
        .execute(Request::Catalog {
            request: crate::catalog::CatalogRequest::Page {
                catalog_id: format!("capture:{source_id}"),
                cursor: None,
                limit: 100,
            },
        })
        .unwrap();
    assert_eq!(page["result"]["documents"].as_array().unwrap().len(), 1);
    let found = runtime
        .execute(Request::SearchRanked {
            query: "quartz".into(),
            sources: vec![source_id.clone()],
            limit: 10,
            mode: crate::SearchMode::Exact,
        })
        .unwrap();
    assert_eq!(found["hits"].as_array().unwrap().len(), 1);
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
        .execute(Request::ConfigureCapture {
            policy: policy.clone(),
        })
        .unwrap();
    let mut disabled = policy;
    disabled.enabled = false;
    runtime
        .execute(Request::ConfigureCapture { policy: disabled })
        .unwrap();
    let scope: crate::catalog::ReaderOwnedSources =
        serde_json::from_value(runtime.execute(Request::ReaderOwnedSources).unwrap()).unwrap();
    assert!(scope.captures.is_empty());
    let page = runtime
        .execute(Request::Catalog {
            request: crate::catalog::CatalogRequest::Page {
                catalog_id: format!("capture:{source_id}"),
                cursor: None,
                limit: 100,
            },
        })
        .unwrap();
    assert_eq!(page["error"], "unauthorized");
}
