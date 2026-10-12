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
    grants::set(&mut store, &id, true, &[source.clone()]).unwrap();
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
        &[source.clone()],
        &[],
        &[source.clone()],
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
