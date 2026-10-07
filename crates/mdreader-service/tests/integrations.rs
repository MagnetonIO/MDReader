#![cfg(unix)]
use mdreader_capture::ProjectPolicy;
use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_service::{
    capture,
    integrations::{self, ClientCapabilities, Kind, Selection},
};
use std::fs;
fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    ReaderStore,
    ProjectPolicy,
    Selection,
) {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Selected project",
            SourceRoot::capture(root.clone()).unwrap(),
        )
        .unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "synthetic-integration".into(),
        project_root: root.clone(),
        repository_root: root.clone(),
        worktree_roots: vec![root.clone()],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    capture::configure(&mut store, data.path(), policy.clone()).unwrap();
    let selection = Selection {
        kind: Kind::CodexHooks,
        project_id: policy.project_id.clone(),
        worktree_root: root,
        executable: None,
        source_ids: vec![source.id],
        capabilities: ClientCapabilities {
            local_commands: true,
            claude_exec_hooks: true,
            codex_lifecycle_hooks: true,
            codex_notify: true,
            client_version: "explicit-synthetic-capability-claim".into(),
        },
    };
    (data, project, store, policy, selection)
}
#[test]
fn durable_review_is_nonmutating_reopens_applies_explicitly_and_uninstalls_only_owned_entries() {
    let (data, _project, mut store, _policy, selection) = fixture();
    let target = selection.worktree_root.join(".codex/hooks.json");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let before = "{\"userSetting\":\"a\\u0020b\"}\n";
    fs::write(&target, before).unwrap();
    let review = integrations::prepare(&mut store, data.path(), selection).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), before);
    assert_eq!(review.before_text.as_deref(), Some(before));
    assert!(!review.capabilities_verified);
    assert!(review.after_text.contains("--mdreader-capture"));
    assert_eq!(review.preparation_id.len(), 36);
    drop(store);
    let mut store = ReaderStore::open(data.path()).unwrap();
    integrations::apply(&mut store, data.path(), &review.preparation_id).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), review.after_text);
    let edited = review.after_text.replacen("{", "{\"userLater\":true,", 1);
    fs::write(&target, &edited).unwrap();
    integrations::uninstall(&mut store, data.path(), &review.preparation_id).unwrap();
    let remaining = fs::read_to_string(&target).unwrap();
    assert!(remaining.contains("\"userLater\":true"));
    assert!(remaining.contains("a\\u0020b"));
    assert!(!remaining.contains("--mdreader-capture"));
}
#[test]
fn stale_policy_or_tampered_service_artifact_never_applies_review() {
    let (data, _project, mut store, mut policy, selection) = fixture();
    let review = integrations::prepare(&mut store, data.path(), selection).unwrap();
    policy.enabled = false;
    capture::configure(&mut store, data.path(), policy).unwrap();
    assert!(integrations::apply(&mut store, data.path(), &review.preparation_id).is_err());
    assert!(!review.config_path.exists());
}

#[test]
fn artifact_tamper_and_changed_review_bytes_are_rejected_without_publication() {
    let (data, _project, mut store, _policy, selection) = fixture();
    let review = integrations::prepare(&mut store, data.path(), selection.clone()).unwrap();
    fs::write(&review.policy_path, b"{}").unwrap();
    assert!(integrations::apply(&mut store, data.path(), &review.preparation_id).is_err());
    assert!(!review.config_path.exists());
    let (data, _project, mut store, _policy, selection) = fixture();
    let review = integrations::prepare(&mut store, data.path(), selection).unwrap();
    fs::create_dir_all(review.config_path.parent().unwrap()).unwrap();
    fs::write(&review.config_path, "{\"userNew\":true}").unwrap();
    assert!(integrations::apply(&mut store, data.path(), &review.preparation_id).is_err());
    assert_eq!(
        fs::read_to_string(&review.config_path).unwrap(),
        "{\"userNew\":true}"
    );
}

#[test]
fn disabled_capture_keeps_explicit_selected_mcp_independent_and_readonly_inspection_private() {
    let (data, _project, mut store, mut policy, mut selection) = fixture();
    policy.enabled = false;
    capture::configure(&mut store, data.path(), policy.clone()).unwrap();
    assert!(integrations::prepare(&mut store, data.path(), selection.clone()).is_err());
    selection.kind = Kind::CodexMcp;
    let review = integrations::prepare(&mut store, data.path(), selection.clone()).unwrap();
    assert!(review.after_text.contains("--source"));
    selection.source_ids.clear();
    assert!(integrations::prepare(&mut store, data.path(), selection).is_err());
    assert!(!review.config_path.exists());
    let entries = integrations::list(&mut store, &policy.project_id).unwrap();
    assert_eq!(entries[0]["preparationId"], review.preparation_id);
    assert_eq!(entries[0]["state"], "prepared");
    assert!(!entries.to_string().contains("afterText"));
    assert!(!entries.to_string().contains("--source"));
    assert!(
        !integrations::capture_policy(&store, &policy.project_id)
            .unwrap()
            .unwrap()
            .enabled
    );
    assert!(integrations::capture_policy(&store, "never-granted")
        .unwrap()
        .is_none());
    integrations::uninstall(&mut store, data.path(), &review.preparation_id).unwrap();
    assert_eq!(
        integrations::list(&mut store, &policy.project_id).unwrap()[0]["state"],
        "removed"
    );
}

#[test]
fn revoked_source_replaced_executable_and_corrupt_saved_record_fail_closed() {
    let (data, _project, mut store, _policy, selection) = fixture();
    let review = integrations::prepare(&mut store, data.path(), selection.clone()).unwrap();
    store
        .set_source_enabled(&selection.source_ids[0], false)
        .unwrap();
    assert!(integrations::apply(&mut store, data.path(), &review.preparation_id).is_err());
    assert!(!review.config_path.exists());
    let (data, project, mut store, _policy, mut selection) = fixture();
    let executable = project.path().join("synthetic app");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    selection.executable = Some(executable.clone());
    let review = integrations::prepare(&mut store, data.path(), selection).unwrap();
    fs::write(&executable, "#!/bin/sh\nexit 42\n").unwrap();
    assert!(integrations::apply(&mut store, data.path(), &review.preparation_id).is_err());
    assert!(!review.config_path.exists());
    let (data, _project, mut store, _policy, selection) = fixture();
    let review = integrations::prepare(&mut store, data.path(), selection).unwrap();
    store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE mdreader_service_integrations_preparations SET record=record||' '",
                [],
            )
        })
        .unwrap();
    assert!(integrations::apply(&mut store, data.path(), &review.preparation_id).is_err());
    assert!(!review.config_path.exists());
}

#[test]
fn utf8_diff_reconstructs_review_and_uninstall_works_after_optout() {
    let (data, _project, mut store, mut policy, selection) = fixture();
    let target = selection.worktree_root.join(".codex/hooks.json");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "{\"note\":\"héllo 📝\"}\n").unwrap();
    let review = integrations::prepare(&mut store, data.path(), selection).unwrap();
    let mut reconstructed = review.before_text.clone().unwrap();
    reconstructed.replace_range(
        review.diff.offset_bytes..review.diff.offset_bytes + review.diff.remove_bytes,
        &review.diff.insert_text,
    );
    assert_eq!(reconstructed, review.after_text);
    integrations::apply(&mut store, data.path(), &review.preparation_id).unwrap();
    integrations::apply(&mut store, data.path(), &review.preparation_id).unwrap();
    policy.enabled = false;
    capture::configure(&mut store, data.path(), policy).unwrap();
    assert!(integrations::apply(&mut store, data.path(), &review.preparation_id).is_err());
    integrations::uninstall(&mut store, data.path(), &review.preparation_id).unwrap();
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        review.before_text.unwrap()
    );
}

#[test]
fn empty_ownership_list_is_readonly_and_does_not_initialize_schema() {
    let data = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(
        integrations::list(&mut store, "synthetic").unwrap(),
        serde_json::json!([])
    );
    assert!(integrations::capture_policy(&store, "synthetic")
        .unwrap()
        .is_none());
    let count:i64=store.with_library_connection(|db| db.query_row("SELECT count(*) FROM sqlite_master WHERE name LIKE 'mdreader_service_integrations_%'",[],|row|row.get(0))).unwrap();
    assert_eq!(count, 0);
    assert!(!data.path().join("integrations").exists());
}

struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(directory: &std::path::Path) -> Daemon {
    use mdreader_service::{call, Request};
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_mdreader-service"))
        .arg("--no-model")
        .arg(directory)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut daemon = Daemon(child);
    for _ in 0..100 {
        if call(directory, &Request::Ping).is_ok() {
            return daemon;
        }
        assert!(daemon.0.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("synthetic service did not become ready");
}
#[test]
fn real_service_transport_persists_review_id_and_routes_readonly_status() {
    use mdreader_service::{call, Request};
    let (data, _project, store, policy, selection) = fixture();
    drop(store);
    let daemon = start(data.path());
    assert_eq!(
        call(
            data.path(),
            &Request::CapturePolicy {
                project_id: policy.project_id.clone()
            }
        )
        .unwrap()["enabled"],
        true
    );
    let wire = call(data.path(), &Request::PrepareIntegration { selection }).unwrap();
    let review: integrations::Review = serde_json::from_value(wire).unwrap();
    assert!(!review.config_path.exists());
    drop(daemon);
    let _daemon = start(data.path());
    assert_eq!(
        call(
            data.path(),
            &Request::ListIntegrations {
                project_id: policy.project_id
            }
        )
        .unwrap()[0]["preparationId"],
        review.preparation_id
    );
    call(
        data.path(),
        &Request::ApplyIntegration {
            preparation_id: review.preparation_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&review.config_path).unwrap(),
        review.after_text
    );
    let edit = review
        .after_text
        .replacen("{", "{\"userAfterRestart\":true,", 1);
    fs::write(&review.config_path, edit).unwrap();
    call(
        data.path(),
        &Request::UninstallIntegration {
            preparation_id: review.preparation_id,
        },
    )
    .unwrap();
    let remaining = fs::read_to_string(&review.config_path).unwrap();
    assert!(remaining.contains("userAfterRestart"));
    assert!(!remaining.contains("--mdreader-capture"));
}

#[test]
fn initially_capture_disabled_mcp_requires_exact_admitted_worktree_and_explicit_capabilities() {
    let (data, project, store, mut policy, mut selection) = fixture();
    drop(store);
    let clean = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(clean.path()).unwrap();
    let source = store
        .add_local_source(
            "Explicit exact worktree",
            SourceRoot::capture(project.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    selection.source_ids = vec![source.id];
    selection.kind = Kind::ClaudeMcp;
    policy.enabled = false;
    capture::configure(&mut store, clean.path(), policy).unwrap();
    selection.capabilities.local_commands = false;
    assert!(integrations::prepare(&mut store, clean.path(), selection.clone()).is_err());
    selection.capabilities.local_commands = true;
    let review = integrations::prepare(&mut store, clean.path(), selection).unwrap();
    assert!(!review.config_path.exists());
    assert!(!review.capabilities_verified);
    assert!(!data.path().join("integrations").exists());
}

#[test]
fn more_than_sixty_four_discarded_reviews_prune_only_closed_snapshots_and_keep_live_ownership() {
    let (data, _project, mut store, policy, selection) = fixture();
    let live = integrations::prepare(&mut store, data.path(), selection.clone()).unwrap();
    integrations::apply(&mut store, data.path(), &live.preparation_id).unwrap();
    let mut first_removed = None;
    for _ in 0..70 {
        let review = integrations::prepare(&mut store, data.path(), selection.clone()).unwrap();
        integrations::uninstall(&mut store, data.path(), &review.preparation_id).unwrap();
        first_removed.get_or_insert(review.preparation_id);
    }
    let entries = integrations::list(&mut store, &policy.project_id).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 9);
    assert_eq!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["state"] == "applied")
            .count(),
        1
    );
    assert!(integrations::apply(&mut store, data.path(), &first_removed.unwrap()).is_err());
    integrations::apply(&mut store, data.path(), &live.preparation_id).unwrap();
    integrations::uninstall(&mut store, data.path(), &live.preparation_id).unwrap();
    assert!(!live.config_path.exists());
}
