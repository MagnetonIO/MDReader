use super::*;
use projects::AgentRequest;
fn fixture() -> (tempfile::TempDir, ReaderStore, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("public.md"), "# Public fixture").unwrap();
    let root = SourceRoot::capture(root.canonicalize().unwrap()).unwrap();
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    let source = store
        .add_local_source("Public project", root.clone())
        .unwrap();
    let selection = Selection {
        agent: Agent::Codex,
        scope: Scope::Project,
        root: root.path.clone(),
        identity: root.identity,
    };
    let id = selection.id().unwrap();
    grants::save_install(
        &mut store,
        &id,
        &Record {
            selection,
            executable: temp.path().join("synthetic-cli"),
            client_version: "codex-cli 0.162.1".into(),
            phase: "installed".into(),
            skill_conflict: false,
            skill_owned: Some(true),
        },
    )
    .unwrap();
    (temp, store, source.id, id)
}
#[test]
fn existing_read_grants_do_not_become_reporting_or_edit_grants() {
    let (_temp, mut store, source, id) = fixture();
    grants::set(&mut store, &id, true, std::slice::from_ref(&source)).unwrap();
    assert_eq!(sources(&mut store, &id).unwrap(), vec![source.clone()]);
    assert!(projects::report(
        &mut store,
        &id,
        AgentRequest::RegisterSession {
            project_id: source.clone(),
            client_session_id: "public-session".into(),
            title: "Public task".into()
        }
    )
    .is_err());
    assert!(documents::fence(&mut store, &id, &source).is_err());
    assert!(projects::report(&mut store, &id, AgentRequest::Handshake).is_ok());
    assert!(permissions::get(&mut store, &id)
        .unwrap()
        .last_seen_ms
        .is_some());
}
#[test]
fn reporting_uses_connection_identity_and_revocation_fences_writes() {
    let (_temp, mut store, source, id) = fixture();
    permissions::set(
        &mut store,
        &id,
        true,
        std::slice::from_ref(&source),
        &[],
        std::slice::from_ref(&source),
    )
    .unwrap();
    let session = projects::report(
        &mut store,
        &id,
        AgentRequest::RegisterSession {
            project_id: source.clone(),
            client_session_id: "public-session".into(),
            title: "Public task".into(),
        },
    )
    .unwrap();
    assert_eq!(session["agent"], "codex");
    let file = projects::report(
        &mut store,
        &id,
        AgentRequest::RegisterDocument {
            session_id: session["id"].as_str().unwrap().into(),
            relative_path: "public.md".into(),
        },
    )
    .unwrap();
    assert_eq!(file["registered"], true);
    let fence = documents::fence(&mut store, &id, &source).unwrap();
    permissions::set(&mut store, &id, false, &[], &[], &[]).unwrap();
    assert!(documents::check(&mut store, &fence).is_err());
    assert!(projects::report(&mut store, &id, AgentRequest::Handshake).is_err());
}
#[test]
fn hook_and_mcp_reports_write_activity_sessions_without_a_project() {
    let (temp, mut store, project, id) = fixture();
    let outside = temp.path().canonicalize().unwrap().join("elsewhere");
    let report = |store: &mut ReaderStore, request| projects::report(store, &id, request);
    permissions::set(
        &mut store,
        &id,
        true,
        std::slice::from_ref(&project),
        &[],
        &[],
    )
    .unwrap();
    // A hook event from a directory no project contains is still recorded.
    let recorded = report(
        &mut store,
        AgentRequest::RecordActivity {
            client_session_id: "public-hook-session".into(),
            cwd: outside.clone(),
            files: vec![activity::ReportedFile {
                path: outside.join("NOTES.md"),
                op: mdreader_core::agent_activity::FileOp::Create,
            }],
            ended: false,
            title: Some("Hook-named session".into()),
        },
    )
    .unwrap();
    let page = store.agent_activity_page(None, 0, 10, None).unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].title, "Hook-named session");
    assert_eq!(page.sessions[0].id, recorded["session_id"]);
    assert_eq!(
        page.sessions[0].agent,
        mdreader_core::agent_activity::Agent::Codex
    );
    assert_eq!(page.sessions[0].cwd.as_deref(), outside.to_str());
    assert!(report(
        &mut store,
        AgentRequest::RecordActivity {
            client_session_id: "x".into(),
            cwd: "relative".into(),
            files: vec![],
            ended: false,
            title: None,
        },
    )
    .is_err());
    // MCP registration still needs a granted project and writes the same rows.
    let session = report(
        &mut store,
        AgentRequest::RegisterSession {
            project_id: project.clone(),
            client_session_id: "public-mcp-session".into(),
            title: "Public task".into(),
        },
    )
    .unwrap();
    let session_id = session["id"].as_str().unwrap().to_owned();
    assert!(report(
        &mut store,
        AgentRequest::RegisterDocument {
            session_id: session_id.clone(),
            relative_path: "missing.md".into(),
        },
    )
    .is_err());
    report(
        &mut store,
        AgentRequest::RegisterDocument {
            session_id: session_id.clone(),
            relative_path: "public.md".into(),
        },
    )
    .unwrap();
    report(
        &mut store,
        AgentRequest::EndSession {
            session_id: session_id.clone(),
        },
    )
    .unwrap();
    let page = store.agent_activity_page(None, 0, 10, None).unwrap();
    let mcp = page.sessions.iter().find(|s| s.id == session_id).unwrap();
    assert_eq!(mcp.title, "Public task");
    assert!(mcp.ended_at_ms.is_some());
    let (_, files) = store.agent_activity_files(&session_id).unwrap();
    assert!(files[0].abs_path.ends_with("project/public.md"));
    // The prior project tables are no longer written.
    assert!(store.agent_sessions(&project).unwrap().is_empty());
}
#[test]
fn operation_ids_are_durable_and_bound_to_exact_text_and_connection() {
    let (temp, mut store, source, id) = fixture();
    let op = uuid::Uuid::new_v4().to_string();
    let request = AgentRequest::CreateDocument {
        source_id: source,
        path: "new.md".into(),
        operation_id: op.clone(),
        markdown: "# Public intended text".into(),
    };
    ledger_prepare(&mut store, &id, &op, &request);
    let mut changed = request.clone();
    if let AgentRequest::CreateDocument { markdown, .. } = &mut changed {
        *markdown = "different".into();
    }
    assert!(matches!(
        operation_store::prepare(&mut store, &id, &op, &changed),
        Err(ReaderError::Conflict)
    ));
    drop(store);
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    assert!(operation_store::get(&mut store, &id, &op)
        .unwrap()
        .is_some());
    assert!(
        operation_store::get(&mut store, &uuid::Uuid::new_v4().to_string(), &op)
            .unwrap()
            .is_none()
    );
}
fn ledger_prepare(store: &mut ReaderStore, id: &str, op: &str, request: &AgentRequest) {
    operation_store::prepare(store, id, op, request).unwrap();
}
#[test]
fn root_cwd_child_paths_and_other_project_survive_a_stale_selected_root() {
    let (temp, mut store, first, id) = fixture();
    let root = temp.path().join("second");
    std::fs::create_dir_all(root.join("child")).unwrap();
    let root = SourceRoot::capture(root.canonicalize().unwrap()).unwrap();
    let second = store.add_local_source("Second", root.clone()).unwrap().id;
    permissions::set(
        &mut store,
        &id,
        true,
        &[first.clone(), second.clone()],
        &[],
        &[],
    )
    .unwrap();
    store.set_source_enabled(&first, false).unwrap();
    let found = projects::report(
        &mut store,
        &id,
        AgentRequest::FindProject {
            path: root.path.clone(),
        },
    )
    .unwrap();
    assert_eq!(found["project_id"], second);
    assert_eq!(found["relative_path"], "");
    let found = projects::report(
        &mut store,
        &id,
        AgentRequest::FindProject {
            path: root.path.join("child"),
        },
    )
    .unwrap();
    assert_eq!(found["project_id"], second);
    assert_eq!(found["relative_path"], "child");
    assert!(projects::report(
        &mut store,
        &id,
        AgentRequest::RegisterSession {
            project_id: first,
            client_session_id: "stale".into(),
            title: "Stale".into()
        }
    )
    .is_err());
}
#[test]
fn public_skill_without_completed_receipt_cannot_claim_installed() {
    let (temp, mut store, _, id) = fixture();
    let mut record = grants::record(&mut store, &id).unwrap().unwrap();
    assets::install(&record.selection, true).unwrap();
    record.phase = "installing".into();
    grants::save_install(&mut store, &id, &record).unwrap();
    assert!(
        !state(&mut store, &temp.path().join("reader"), &record.selection)
            .unwrap()
            .installed
    );
    assert_eq!(
        grants::record(&mut store, &id).unwrap().unwrap().phase,
        "installing"
    );
}
#[cfg(feature = "sync-native")]
#[path = "device_tests.rs"]
mod device_tests;
#[path = "provider_tests.rs"]
mod provider_tests;
#[test]
fn reporting_only_write_refusal_does_not_create_an_intent() {
    let (temp, mut store, source, id) = fixture();
    permissions::set(
        &mut store,
        &id,
        true,
        std::slice::from_ref(&source),
        &[],
        &[],
    )
    .unwrap();
    let runtime = crate::runtime::Runtime::new(
        std::sync::Arc::new(std::sync::Mutex::new(store)),
        temp.path().join("reader/models"),
        crate::ServiceOptions {
            disable_model: true,
        },
    )
    .unwrap();
    assert!(runtime
        .execute(crate::Request::AgentCall {
            grant_id: id.clone(),
            request: AgentRequest::CreateDocument {
                source_id: source,
                path: "refused.md".into(),
                operation_id: uuid::Uuid::new_v4().to_string(),
                markdown: "# Must not be retained".into()
            }
        })
        .is_err());
    let mut owner = runtime.store().unwrap();
    let count:u64=owner.with_library_connection(|db|db.query_row("SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_schema WHERE name='mdreader_agent_operations_v1') THEN 1 ELSE 0 END",[],|r|r.get(0))).unwrap();
    let intents: u64 = if count == 0 {
        0
    } else {
        owner
            .with_library_connection(|db| {
                db.query_row(
                    "SELECT count(*) FROM mdreader_agent_operations_v1",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap()
    };
    assert_eq!(
        intents, 0,
        "authorization fails before any body intent is written"
    );
}
#[cfg(unix)]
#[test]
fn a_selection_with_a_renumbered_folder_identity_resolves_to_the_installed_grant() {
    let (temp, mut store, _source, id) = fixture();
    let installed = grants::record(&mut store, &id).unwrap().unwrap().selection;
    let SourceRootIdentity::Unix { device, inode } = installed.identity.clone() else {
        unreachable!()
    };
    let renumbered = Selection {
        identity: SourceRootIdentity::Unix {
            device: device.wrapping_add(1),
            inode: inode.wrapping_add(2),
        },
        ..installed.clone()
    };
    assert_ne!(renumbered.id().unwrap(), id);
    assert_eq!(
        installed_selection(&mut store, renumbered)
            .unwrap()
            .id()
            .unwrap(),
        id
    );
    // Another folder or agent is never matched to this grant.
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let other = Selection {
        root: elsewhere.canonicalize().unwrap(),
        ..installed.clone()
    };
    assert_ne!(
        installed_selection(&mut store, other)
            .unwrap()
            .id()
            .unwrap(),
        id
    );
    let claude = Selection {
        agent: Agent::Claude,
        ..installed
    };
    assert_ne!(
        installed_selection(&mut store, claude)
            .unwrap()
            .id()
            .unwrap(),
        id
    );
}
#[test]
fn concurrent_client_config_rewrites_are_retried_and_a_persistent_conflict_is_reported() {
    let (temp, mut store, _source, id) = fixture();
    let record = grants::record(&mut store, &id).unwrap().unwrap();
    let directory = temp.path().join("profile");
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.canonicalize().unwrap();
    crate::capture::configure(&mut store, &directory, policy(&record.selection).unwrap()).unwrap();
    let req = request(&directory, &record).unwrap();
    let policy = policy(&record.selection).unwrap();
    // Simulate the agent rewriting its config between our read and our write.
    let mut rewrites = 0;
    install_config_observed(
        &mut installer(&directory).unwrap(),
        &req,
        &policy,
        &mut |path| {
            if rewrites < 2 {
                rewrites += 1;
                let mut text = std::fs::read_to_string(path).unwrap_or_default();
                text.push_str(&format!("\n# concurrent client write {rewrites}\n"));
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(rewrites, 2);
    let config = std::fs::read_to_string(req.kind.config_path(&req.worktree_root)).unwrap();
    assert!(
        config.contains("# concurrent client write 2"),
        "agent edits are kept"
    );
    assert!(config.contains(&id), "the MDReader entry is installed");
    assert!(installer(&directory)
        .unwrap()
        .verify_installed(&req, &policy)
        .unwrap());

    // A conflict on every attempt is reported, not retried forever.
    let fresh = tempfile::tempdir().unwrap();
    let other = fresh.path().canonicalize().unwrap();
    let mut changing = Record {
        selection: Selection {
            root: other.clone(),
            identity: SourceRoot::capture(other.clone()).unwrap().identity,
            ..record.selection.clone()
        },
        ..record
    };
    changing.phase = "installing".into();
    crate::capture::configure(&mut store, &directory, policy_for(&changing)).unwrap();
    let req = request(&directory, &changing).unwrap();
    let mut attempts = 0;
    let refused = install_config_observed(
        &mut installer(&directory).unwrap(),
        &req,
        &policy_for(&changing),
        &mut |path| {
            attempts += 1;
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("# rewrite {attempts}\n")).unwrap();
        },
    );
    assert!(refused.is_err());
    assert_eq!(attempts, CONFIG_ATTEMPTS);
}
fn policy_for(record: &Record) -> mdreader_capture::ProjectPolicy {
    policy(&record.selection).unwrap()
}
#[test]
fn an_all_sources_read_grant_resolves_live_and_includes_sources_added_later() {
    let (temp, mut store, project, id) = fixture();
    permissions::set(&mut store, &id, true, &[], &[ALL_SOURCES.to_string()], &[]).unwrap();
    assert_eq!(sources(&mut store, &id).unwrap(), vec![project.clone()]);
    let later = temp.path().join("later");
    std::fs::create_dir(&later).unwrap();
    let later = store
        .add_local_source(
            "Later",
            SourceRoot::capture(later.canonicalize().unwrap()).unwrap(),
        )
        .unwrap()
        .id;
    let granted = sources(&mut store, &id).unwrap();
    assert!(granted.contains(&project) && granted.contains(&later));
    // Disabled sources drop out of the resolved grant.
    store.set_source_enabled(&later, false).unwrap();
    assert_eq!(sources(&mut store, &id).unwrap(), vec![project]);
}
#[test]
fn explicit_read_lists_stay_bounded_unless_they_name_every_enabled_source() {
    let current = State {
        installed: true,
        enabled: true,
        source_ids: vec![],
        all_sources: false,
        sources: (0..40)
            .map(|i| mdreader_core::Source {
                id: format!("source-{i:02}"),
                name: format!("Source {i}"),
                enabled: true,
            })
            .collect(),
        reporting_project_ids: vec![],
        edit_source_ids: vec![],
        editable_source_ids: vec![],
        connected: false,
        last_seen_ms: None,
    };
    let ids = |n: usize| (0..n).map(|i| format!("source-{i:02}")).collect::<Vec<_>>();
    assert_eq!(read_grant(&current, &ids(3)).unwrap(), ids(3));
    assert!(
        read_grant(&current, &ids(33)).is_err(),
        "explicit lists stay bounded"
    );
    assert_eq!(read_grant(&current, &ids(40)).unwrap(), [ALL_SOURCES]);
    assert_eq!(
        read_grant(&current, &[ALL_SOURCES.into()]).unwrap(),
        [ALL_SOURCES]
    );
    assert!(read_grant(&current, &["unknown".into()]).is_err());
    assert!(read_grant(&current, &[ids(1)[0].clone(), ids(1)[0].clone()]).is_err());
}
