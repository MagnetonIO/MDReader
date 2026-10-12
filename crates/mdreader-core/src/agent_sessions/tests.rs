use super::*;
fn fixture() -> (tempfile::TempDir, ReaderStore, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("note.md"), "# Public note").unwrap();
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    let source = store
        .add_local_source(
            "Public project",
            crate::SourceRoot::capture(root.canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    (temp, store, source.id)
}
#[test]
fn sessions_and_associations_reopen_without_duplicate_indexing() {
    let (temp, mut store, project) = fixture();
    let connection = uuid::Uuid::new_v4().to_string();
    let a = store
        .register_agent_session(&project, &connection, "client1", "codex", "Public task")
        .unwrap();
    let b = store
        .register_agent_session(
            &project,
            &connection,
            "client1",
            "codex",
            "Public task renamed",
        )
        .unwrap();
    assert_eq!(a.id, b.id);
    store
        .register_agent_document(&connection, &a.id, "note.md")
        .unwrap();
    store
        .register_agent_document(&connection, &a.id, "note.md")
        .unwrap();
    let c = store
        .register_agent_session(&project, &connection, "client2", "claude", "Second session")
        .unwrap();
    store
        .register_agent_document(&connection, &c.id, "note.md")
        .unwrap();
    store.end_agent_session(&connection, &a.id).unwrap();
    drop(store);
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    let sessions = store.agent_sessions(&project).unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions
        .iter()
        .any(|s| s.id == a.id && s.ended_at_ms.is_some()));
    let documents = store.agent_documents(&project, Some(&a.id)).unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].session_ids.len(), 2);
}
#[test]
fn reports_require_existing_markdown_and_current_project_identity() {
    let (temp, mut store, project) = fixture();
    let connection = uuid::Uuid::new_v4().to_string();
    let session = store
        .register_agent_session(&project, &connection, "client", "codex", "Public task")
        .unwrap();
    for path in ["../note.md", "missing.md", "note.txt"] {
        assert!(store
            .register_agent_document(&connection, &session.id, path)
            .is_err());
    }
    store.set_folder_monitoring(&project, false).unwrap();
    assert!(store
        .register_agent_document(&connection, &session.id, "note.md")
        .is_err());
    store.set_folder_monitoring(&project, true).unwrap();
    let root = temp.path().join("project");
    std::fs::rename(&root, temp.path().join("old")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("note.md"), "replacement").unwrap();
    assert!(store
        .register_agent_document(&connection, &session.id, "note.md")
        .is_err());
}
