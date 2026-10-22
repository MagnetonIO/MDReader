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
fn prior_rows_stay_readable_by_project_and_session() {
    let (temp, mut store, project) = fixture();
    store.agent_sessions(&project).unwrap();
    store
        .with_library_connection(|db| -> rusqlite::Result<()> {
            db.execute("INSERT INTO reader_agent_sessions VALUES('a',?1,'c','client1','codex','Public task',1,3,2)", [&project])?;
            db.execute("INSERT INTO reader_agent_sessions VALUES('b',?1,'c','client2','claude','Second',1,2,NULL)", [&project])?;
            db.execute("INSERT INTO reader_agent_documents VALUES('a','note.md',1)", [])?;
            db.execute("INSERT INTO reader_agent_documents VALUES('b','note.md',1)", [])?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    let sessions = store.agent_sessions(&project).unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions
        .iter()
        .any(|s| s.id == "a" && s.ended_at_ms.is_some()));
    let documents = store.agent_documents(&project, Some("a")).unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].session_ids, ["a", "b"]);
    assert!(store.agent_documents(&project, Some("missing")).is_err());
}
#[test]
fn version_one_namespace_upgrades_in_place_and_keeps_sessions() {
    let (_temp, mut store, project) = fixture();
    let v1 = include_str!("schema.sql")
        .lines()
        .filter(|line| !line.contains("reader_agent_source_documents"))
        .collect::<Vec<_>>()
        .join("\n");
    store
        .with_library_connection(|db| -> rusqlite::Result<()> {
            db.execute_batch(&v1)?;
            db.execute("INSERT INTO reader_agent_sessions_meta VALUES(1,1)", [])?;
            db.execute("INSERT INTO reader_agent_sessions VALUES('public-session',?1,'c','client','codex','Public task',1,1,NULL)", [&project])?;
            Ok(())
        })
        .unwrap();
    let sessions = store.agent_sessions(&project).unwrap();
    assert_eq!(sessions.len(), 1);
    let version: i64 = store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT version FROM reader_agent_sessions_meta WHERE id=1",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(version, 2);
}
