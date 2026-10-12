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
