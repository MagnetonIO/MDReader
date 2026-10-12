use mdreader_capture::ProjectPolicy;
use mdreader_integrations::*;
use std::{fs, path::PathBuf};
use tempfile::TempDir;
fn setup(kind: IntegrationKind) -> (TempDir, Installer, InstallRequest, ProjectPolicy) {
    let temp = TempDir::new().unwrap();
    let worktree = temp.path().join("project");
    let data = temp.path().join("private library");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&data).unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "synthetic-project".into(),
        project_root: worktree.clone(),
        repository_root: worktree.clone(),
        worktree_roots: vec![worktree.clone()],
        capture_subagents: true,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    let request = InstallRequest {
        kind,
        worktree_root: worktree,
        executable: PathBuf::from("/Applications/MDReader.app/Contents/MacOS/mdreader-app"),
        policy_path: data.join("captures/policies/service-grant.json"),
        spool_root: data.join("captures"),
        data_dir: data,
        source_ids: vec!["selected-source".into()],
        mcp_grant_id: None,
        capabilities: Capabilities {
            local_commands: true,
            claude_exec_hooks: true,
            codex_lifecycle_hooks: true,
            codex_notify: true,
            client_version: "synthetic-supported-version".into(),
        },
    };
    fs::create_dir_all(request.policy_path.parent().unwrap()).unwrap();
    fs::write(&request.policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let installer = Installer::open(&temp.path().join("private integration state")).unwrap();
    (temp, installer, request, policy)
}
#[test]
fn claude_hook_install_preserves_unrelated_bytes_and_uninstall_restores_original() {
    let (_temp, mut installer, request, policy) = setup(IntegrationKind::ClaudeHooks);
    let config = request.worktree_root.join(".claude/settings.local.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = "{\n  \"env\": {\"KEEP\": \"a\\u0020b\"}, \"number\":1.00e2,\n  \"hooks\": {\"Stop\": [{\"hooks\":[{\"type\":\"command\",\"command\":\"echo existing\"}]}]}\n}\n";
    fs::write(&config, original).unwrap();
    let plan = installer.prepare(&request, &policy).unwrap();
    assert_eq!(
        installer.apply(plan, &policy).unwrap(),
        fs::canonicalize(&config).unwrap()
    );
    let installed = fs::read_to_string(&config).unwrap();
    assert!(installed.contains("\"KEEP\": \"a\\u0020b\""));
    assert!(installed.contains("\"number\":1.00e2"));
    let settings: serde_json::Value = serde_json::from_str(&installed).unwrap();
    assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 2);
    installer.uninstall(&request).unwrap();
    assert_eq!(fs::read_to_string(config).unwrap(), original);
}

#[test]
fn claude_reformatting_keeps_owned_install_valid_and_preserves_later_settings() {
    for kind in [IntegrationKind::ClaudeUserMcp, IntegrationKind::ClaudeHooks] {
        let (_temp, mut installer, request, policy) = setup(kind);
        let target = request.kind.config_path(&request.worktree_root);
        let plan = installer.prepare(&request, &policy).unwrap();
        installer.apply(plan, &policy).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        value["later"] = serde_json::json!({"keep":"untouched"});
        let reformatted = serde_json::to_string_pretty(&value).unwrap();
        fs::write(&target, &reformatted).unwrap();
        installer
            .apply(installer.prepare(&request, &policy).unwrap(), &policy)
            .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), reformatted);
        installer.uninstall(&request).unwrap();
        let remaining: serde_json::Value =
            serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(remaining["later"], value["later"]);
        assert!(!fs::read_to_string(&target).unwrap().contains("--mdreader-"));
    }
}

#[test]
fn launcher_refresh_preserves_binding_and_uninstall_does_not_restore_old_launcher() {
    for kind in [IntegrationKind::ClaudeUserMcp, IntegrationKind::CodexMcp] {
        let (_temp, mut installer, mut request, policy) = setup(kind);
        installer
            .apply(installer.prepare(&request, &policy).unwrap(), &policy)
            .unwrap();
        request.executable =
            PathBuf::from("/Applications/MDReader New.app/Contents/MacOS/mdreader-app");
        let target = installer.refresh_launcher(&request, &policy).unwrap();
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("MDReader New.app")
        );
        installer.uninstall(&request).unwrap();
        assert!(!target.exists() || !fs::read_to_string(&target).unwrap().contains("--mdreader-"));
    }
}

#[test]
fn launcher_refresh_refuses_changed_library_grant_or_owned_command() {
    let (_temp, mut installer, mut request, policy) = setup(IntegrationKind::ClaudeUserMcp);
    let target = installer
        .apply(installer.prepare(&request, &policy).unwrap(), &policy)
        .unwrap();
    let installed = fs::read_to_string(&target).unwrap();
    request.source_ids = vec!["different-source".into()];
    assert!(matches!(
        installer.refresh_launcher(&request, &policy),
        Err(IntegrationError::Conflict)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), installed);
    request.source_ids = vec!["selected-source".into()];
    let edited = installed.replace("mdreader-app", "externally-edited");
    fs::write(&target, &edited).unwrap();
    assert!(matches!(
        installer.refresh_launcher(&request, &policy),
        Err(IntegrationError::Conflict)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), edited);
}
#[test]
fn opted_out_project_never_generates_hook_configuration() {
    let (_temp, installer, request, mut policy) = setup(IntegrationKind::CodexHooks);
    policy.enabled = false;
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::OptOut)
    ));
    assert!(!request.worktree_root.join(".codex/hooks.json").exists());
}

#[test]
fn mcp_never_defaults_to_all_library_sources_and_input_sizes_are_bounded() {
    let (_temp, installer, mut request, policy) = setup(IntegrationKind::CodexMcp);
    request.source_ids.clear();
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::Invalid)
    ));
    request.kind = IntegrationKind::CodexHooks;
    assert!(installer.prepare(&request, &policy).is_ok());
    let target = request.kind.config_path(&request.worktree_root);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let oversized = " ".repeat(512 * 1024 + 1);
    fs::write(&target, &oversized).unwrap();
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::Bounds)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), oversized);
}

#[test]
fn each_documented_adapter_installs_reinstalls_and_uninstalls_without_live_files() {
    for kind in [
        IntegrationKind::ClaudeHooks,
        IntegrationKind::ClaudeMcp,
        IntegrationKind::CodexHooks,
        IntegrationKind::CodexMcp,
        IntegrationKind::CodexNotify,
    ] {
        let (_temp, mut installer, request, policy) = setup(kind);
        let plan = installer.prepare(&request, &policy).unwrap();
        let target = plan.config_path().to_owned();
        let policy_path = plan.policy_path().to_owned();
        assert!(!target.exists());
        assert!(policy_path.exists());
        installer.apply(plan, &policy).unwrap();
        assert!(policy_path.is_file());
        let first = fs::read(&target).unwrap();
        let plan = installer.prepare(&request, &policy).unwrap();
        installer.apply(plan, &policy).unwrap();
        assert_eq!(fs::read(&target).unwrap(), first);
        installer.uninstall(&request).unwrap();
        assert!(!target.exists());
        installer.uninstall(&request).unwrap();
        let plan = installer.prepare(&request, &policy).unwrap();
        installer.apply(plan, &policy).unwrap();
        installer.rollback(&request).unwrap();
        assert!(!target.exists());
    }
}

#[test]
fn uninstall_preserves_subsequent_unrelated_edits_and_user_hooks_servers() {
    for kind in [
        IntegrationKind::ClaudeHooks,
        IntegrationKind::ClaudeMcp,
        IntegrationKind::CodexHooks,
        IntegrationKind::CodexMcp,
        IntegrationKind::CodexNotify,
    ] {
        let (_temp, mut installer, request, policy) = setup(kind);
        let target = request.kind.config_path(&request.worktree_root);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let original = if matches!(
            kind,
            IntegrationKind::CodexMcp | IntegrationKind::CodexNotify
        ) {
            "# original\nmodel = 'kept-model'\n[mcp_servers.existing]\ncommand = 'existing-tool' # keep\n"
        } else {
            "{\"env\":{\"KEEP\":\"unchanged\"},\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"existing\"}]}]},\"mcpServers\":{\"existing\":{\"command\":\"existing-tool\"}}}"
        };
        fs::write(&target, original).unwrap();
        let plan = installer.prepare(&request, &policy).unwrap();
        installer.apply(plan, &policy).unwrap();
        let installed = fs::read_to_string(&target).unwrap();
        let edited = if matches!(
            kind,
            IntegrationKind::CodexMcp | IntegrationKind::CodexNotify
        ) {
            format!(
                "# user subsequently added this comment\n{installed}\n[mcp_servers.later]\ncommand = 'later-tool'\n"
            )
        } else {
            installed.replacen("{", "{\"userLater\":\"a\\u0020b\",", 1)
        };
        fs::write(&target, &edited).unwrap();
        let plan = installer.prepare(&request, &policy).unwrap();
        installer.apply(plan, &policy).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), edited);
        installer.uninstall(&request).unwrap();
        let remaining = fs::read_to_string(&target).unwrap();
        assert!(remaining.contains("existing-tool"));
        assert!(remaining.contains(
            if matches!(
                kind,
                IntegrationKind::CodexMcp | IntegrationKind::CodexNotify
            ) {
                "command = 'later-tool'"
            } else {
                "\"userLater\":\"a\\u0020b\""
            }
        ));
        assert!(!remaining.contains("--mdreader-"));
    }
}

#[test]
fn concurrent_prepared_changes_and_opt_out_before_apply_fail_without_clobbering() {
    let (_temp, mut installer, request, mut policy) = setup(IntegrationKind::CodexHooks);
    let plan = installer.prepare(&request, &policy).unwrap();
    let target = plan.config_path().to_owned();
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let user = "{\"userConcurrent\":true}";
    fs::write(&target, user).unwrap();
    assert!(matches!(
        installer.apply(plan, &policy),
        Err(IntegrationError::Conflict)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), user);
    let plan = installer.prepare(&request, &policy).unwrap();
    policy.enabled = false;
    assert!(matches!(
        installer.apply(plan, &policy),
        Err(IntegrationError::OptOut)
    ));
    assert_eq!(fs::read_to_string(target).unwrap(), user);
}

#[test]
fn service_artifact_is_consumer_only_and_revalidated_before_configuration_change() {
    let (_temp, mut installer, request, mut policy) = setup(IntegrationKind::CodexHooks);
    fs::write(
        &request.policy_path,
        serde_json::to_vec_pretty(&policy).unwrap(),
    )
    .unwrap();
    let original = fs::read(&request.policy_path).unwrap();
    let plan = installer.prepare(&request, &policy).unwrap();
    policy.excluded_sessions.push("revoked-session".into());
    fs::write(
        &request.policy_path,
        serde_json::to_vec_pretty(&policy).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        installer.apply(plan, &policy),
        Err(IntegrationError::Conflict)
    ));
    let latest = fs::read(&request.policy_path).unwrap();
    assert_ne!(latest, original);
    let plan = installer.prepare(&request, &policy).unwrap();
    installer.apply(plan, &policy).unwrap();
    assert_eq!(fs::read(&request.policy_path).unwrap(), latest);
    policy.enabled = false;
    fs::write(
        &request.policy_path,
        serde_json::to_vec_pretty(&policy).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::OptOut)
    ));
    installer.uninstall(&request).unwrap();
    assert!(
        !serde_json::from_slice::<serde_json::Value>(&fs::read(&request.policy_path).unwrap())
            .unwrap()["enabled"]
            .as_bool()
            .unwrap()
    );
    let mut mcp = request.clone();
    mcp.kind = IntegrationKind::CodexMcp;
    let plan = installer.prepare(&mcp, &policy).unwrap();
    installer.apply(plan, &policy).unwrap();
    installer.uninstall(&mcp).unwrap();
}

#[test]
fn separately_prepared_installs_share_receipt_lock_and_never_adopt_each_others_entries() {
    let (temp, mut first, request, policy) = setup(IntegrationKind::CodexHooks);
    let mut second = Installer::open(&temp.path().join("private integration state")).unwrap();
    let one = first.prepare(&request, &policy).unwrap();
    let two = second.prepare(&request, &policy).unwrap();
    let target = first.apply(one, &policy).unwrap();
    let installed = fs::read(&target).unwrap();
    assert!(matches!(
        second.apply(two, &policy),
        Err(IntegrationError::Conflict)
    ));
    assert_eq!(fs::read(target).unwrap(), installed);
}

#[test]
fn unrecorded_identical_entries_and_edited_owned_entries_remain_unmanaged() {
    for kind in [
        IntegrationKind::ClaudeHooks,
        IntegrationKind::ClaudeMcp,
        IntegrationKind::CodexHooks,
        IntegrationKind::CodexMcp,
        IntegrationKind::CodexNotify,
    ] {
        let (_temp, mut installer, request, policy) = setup(kind);
        let plan = installer.prepare(&request, &policy).unwrap();
        let target = plan.config_path().to_owned();
        let after = plan.after_bytes().to_owned();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, &after).unwrap();
        assert!(matches!(
            installer.prepare(&request, &policy),
            Err(IntegrationError::Conflict)
        ));
        assert!(matches!(
            installer.uninstall(&request),
            Err(IntegrationError::Conflict)
        ));
        assert_eq!(fs::read(&target).unwrap(), after);
        fs::remove_file(&target).unwrap();
        let plan = installer.prepare(&request, &policy).unwrap();
        installer.apply(plan, &policy).unwrap();
        let edited = fs::read_to_string(&target)
            .unwrap()
            .replace("mdreader-app", "user-modified-app");
        fs::write(&target, &edited).unwrap();
        assert!(matches!(
            installer.uninstall(&request),
            Err(IntegrationError::Conflict)
        ));
        assert_eq!(fs::read_to_string(&target).unwrap(), edited);
    }
}

#[test]
fn malformed_duplicate_unknown_target_schema_and_unsupported_capabilities_fail_closed() {
    for (kind, bad) in [
        (IntegrationKind::ClaudeHooks, "{\"hooks\":[]}"),
        (
            IntegrationKind::ClaudeHooks,
            "{\"hooks\":{\"Stop\":[\"unknown-hook-group\"]}}",
        ),
        (
            IntegrationKind::CodexHooks,
            "{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"future-handler-kind\"}]}]}}",
        ),
        (IntegrationKind::CodexHooks, "{\"hooks\":{\"Stop\":{}}}"),
        (IntegrationKind::ClaudeMcp, "{\"mcpServers\":false}"),
        (IntegrationKind::ClaudeHooks, "{\"keep\":1,\"keep\":2}"),
        (
            IntegrationKind::ClaudeHooks,
            "{\"hooks\":{\"Stop\":[{\"hooks\":[]}],}}",
        ),
        (IntegrationKind::CodexMcp, "mcp_servers = 3\n"),
        (
            IntegrationKind::CodexNotify,
            "notify = 'unsupported-shape'\n",
        ),
        (
            IntegrationKind::CodexMcp,
            "model = 'a'\nmodel = 'duplicate'\n",
        ),
    ] {
        let (_temp, installer, request, policy) = setup(kind);
        let target = request.kind.config_path(&request.worktree_root);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, bad).unwrap();
        assert!(
            installer.prepare(&request, &policy).is_err(),
            "{kind:?}: {bad}"
        );
        assert_eq!(fs::read_to_string(target).unwrap(), bad);
    }
    let (_temp, installer, mut request, policy) = setup(IntegrationKind::CodexHooks);
    request.capabilities.codex_lifecycle_hooks = false;
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::Unsupported)
    ));
    request.kind = IntegrationKind::ClaudeHooks;
    request.capabilities.claude_exec_hooks = false;
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::Unsupported)
    ));
}

#[test]
fn shell_quotes_and_exec_vectors_deliver_special_paths_as_exact_arguments() {
    let (temp, mut installer, mut request, mut policy) = setup(IntegrationKind::CodexHooks);
    let executable = temp
        .path()
        .join("binary with 'quotes' $dollar `tick` ;semi");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$MDREADER_TEST_ARGV\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    }
    request.executable = executable.clone();
    policy.capture_subagents = false;
    fs::write(&request.policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let plan = installer.prepare(&request, &policy).unwrap();
    let policy_path = plan.policy_path().to_owned();
    let target = installer.apply(plan, &policy).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
    assert!(value["hooks"].get("SubagentStop").is_none());
    let command = value["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let argv = temp.path().join("argv.txt");
    assert!(
        std::process::Command::new("/bin/sh")
            .args(["-c", command])
            .env("MDREADER_TEST_ARGV", &argv)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read_to_string(&argv)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec![
            "--mdreader-capture",
            "hook",
            "codex",
            policy_path.to_str().unwrap(),
            request.spool_root.to_str().unwrap()
        ]
    );
    request.kind = IntegrationKind::ClaudeHooks;
    let plan = installer.prepare(&request, &policy).unwrap();
    let value: serde_json::Value = serde_json::from_slice(plan.after_bytes()).unwrap();
    let hook = &value["hooks"]["Stop"][0]["hooks"][0];
    assert_eq!(hook["command"], executable.to_str().unwrap());
    assert_eq!(hook["args"][0], "--mdreader-capture");
    assert_eq!(hook["args"][2], "claude");
}

#[cfg(unix)]
#[test]
fn symlink_boundaries_and_replaced_state_root_are_rejected() {
    use std::os::unix::fs::symlink;
    let (temp, installer, request, policy) = setup(IntegrationKind::CodexHooks);
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, request.worktree_root.join(".codex")).unwrap();
    assert!(installer.prepare(&request, &policy).is_err());
    assert!(!outside.join("hooks.json").exists());
    fs::remove_file(request.worktree_root.join(".codex")).unwrap();
    let root = temp.path().join("private integration state");
    let moved = temp.path().join("displaced state");
    fs::rename(&root, &moved).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::Conflict)
    ));
}

#[cfg(unix)]
#[test]
fn cooperative_lock_is_nonblocking_and_replaced_worktree_cannot_inherit_a_prepared_grant() {
    use fs2::FileExt;
    let (temp, mut installer, request, policy) = setup(IntegrationKind::CodexHooks);
    let plan = installer.prepare(&request, &policy).unwrap();
    let state = temp.path().join("private integration state");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.join("installer.lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::Busy)
    ));
    FileExt::unlock(&lock).unwrap();
    fs::rename(
        &request.worktree_root,
        temp.path().join("displaced worktree"),
    )
    .unwrap();
    fs::create_dir(&request.worktree_root).unwrap();
    assert!(matches!(
        installer.apply(plan, &policy),
        Err(IntegrationError::Conflict)
    ));
    assert!(!request.kind.config_path(&request.worktree_root).exists());
}

#[test]
fn large_claude_user_config_installs_reopens_and_preserves_later_client_state() {
    let (temp, mut installer, request, policy) = setup(IntegrationKind::ClaudeUserMcp);
    let target = request.kind.config_path(&request.worktree_root);
    let original = format!(
        "{{\n  \"clientState\": \"{}\",\n  \"mcpServers\": {{\"existing\": {{\"command\": \"existing-tool\"}}}}\n}}\n",
        "x".repeat(600 * 1024)
    );
    fs::write(&target, &original).unwrap();
    installer
        .apply(installer.prepare(&request, &policy).unwrap(), &policy)
        .unwrap();
    drop(installer);
    let mut installer = Installer::open(&temp.path().join("private integration state")).unwrap();
    installer
        .apply(installer.prepare(&request, &policy).unwrap(), &policy)
        .unwrap();
    let installed = fs::read_to_string(&target).unwrap();
    let edited = installed.replacen("{", "{\"newClientState\":true,", 1);
    fs::write(&target, &edited).unwrap();
    installer.uninstall(&request).unwrap();
    let after = fs::read_to_string(&target).unwrap();
    assert!(after.contains(&format!("\"clientState\": \"{}\"", "x".repeat(600 * 1024))));
    let config: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(config["newClientState"], true);
    assert_eq!(config["mcpServers"]["existing"]["command"], "existing-tool");
    assert!(config["mcpServers"].get("mdreader").is_none());
}

#[test]
fn oversized_claude_user_config_is_refused_without_changing_original() {
    let (_temp, installer, request, policy) = setup(IntegrationKind::ClaudeUserMcp);
    let target = request.kind.config_path(&request.worktree_root);
    let original = " ".repeat(1024 * 1024 + 1);
    fs::write(&target, &original).unwrap();
    assert!(matches!(
        installer.prepare(&request, &policy),
        Err(IntegrationError::Bounds)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), original);
}

#[test]
fn quote_dense_near_limit_claude_user_config_has_complete_removal_capacity() {
    let (temp, mut installer, request, policy) = setup(IntegrationKind::ClaudeUserMcp);
    let target = request.kind.config_path(&request.worktree_root);
    let original = format!(
        "{{\"clientState\":[{}]}}",
        vec!["\"\""; 320 * 1024].join(",")
    );
    assert!(original.len() > 900 * 1024 && original.len() < 1024 * 1024);
    fs::write(&target, &original).unwrap();
    installer
        .apply(installer.prepare(&request, &policy).unwrap(), &policy)
        .unwrap();
    drop(installer);
    let mut installer = Installer::open(&temp.path().join("private integration state")).unwrap();
    installer.uninstall(&request).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), original);
    drop(installer);
    let mut installer = Installer::open(&temp.path().join("private integration state")).unwrap();
    installer.uninstall(&request).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), original);
}
