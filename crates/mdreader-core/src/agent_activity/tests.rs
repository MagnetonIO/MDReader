use super::*;
use crate::ReaderStore;
use std::path::{Path, PathBuf};

fn store() -> (tempfile::TempDir, ReaderStore) {
    let temp = tempfile::tempdir().unwrap();
    let store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    (temp, store)
}

fn observed(agent: Agent, client: &str, at: i64) -> SessionObservation {
    let mut o = SessionObservation::new(agent, client, at);
    o.cwd = Some("/public/project".into());
    o
}

fn file(path: &str, op: FileOp, at: i64) -> FileObservation {
    FileObservation {
        abs_path: PathBuf::from(path),
        op,
        at_ms: at,
    }
}

fn titles(store: &mut ReaderStore) -> Vec<(String, TitleSource)> {
    store
        .agent_activity_page(None, 0, 100, None)
        .unwrap()
        .sessions
        .into_iter()
        .map(|s| (s.title, s.title_source))
        .collect()
}

#[test]
fn titles_follow_precedence_and_never_downgrade() {
    let (_temp, mut store) = store();
    let mut o = observed(Agent::Claude, "s1", 1_000);
    o.files
        .push(file("/public/project/a.md", FileOp::Create, 1_000));
    store.record_agent_activity(&[o], None).unwrap();
    assert_eq!(
        titles(&mut store),
        vec![("project · 1970-01-01".into(), TitleSource::Fallback)]
    );
    let step = |store: &mut ReaderStore, title: &str, source: TitleSource| {
        let mut o = observed(Agent::Claude, "s1", 2_000);
        o.offer_title(title, source);
        store.record_agent_activity(&[o], None).unwrap()
    };
    assert_eq!(
        step(&mut store, "First prompt", TitleSource::Prompt).len(),
        1
    );
    assert!(step(&mut store, "Second prompt", TitleSource::Prompt).is_empty());
    step(&mut store, "AI title", TitleSource::Ai);
    step(&mut store, "AI title, refined", TitleSource::Ai);
    assert_eq!(titles(&mut store)[0].0, "AI title, refined");
    step(&mut store, "renamed in client", TitleSource::Custom);
    assert!(step(&mut store, "Later AI title", TitleSource::Ai).is_empty());
    assert_eq!(
        titles(&mut store)[0],
        ("renamed in client".into(), TitleSource::Custom)
    );
    let id = store
        .agent_session_id(Agent::Claude, "s1")
        .unwrap()
        .unwrap();
    store.rename_agent_session(&id, "  Mine \n").unwrap();
    step(&mut store, "client again", TitleSource::Custom);
    assert_eq!(titles(&mut store)[0], ("Mine".into(), TitleSource::User));
    store.rename_agent_session(&id, "   ").unwrap();
    step(&mut store, "client again", TitleSource::Custom);
    assert_eq!(titles(&mut store)[0].1, TitleSource::Custom);
}

#[test]
fn listing_requires_a_file_hides_on_request_and_pages_with_filters() {
    let (_temp, mut store) = store();
    let mut quiet = observed(Agent::Codex, "quiet", 50);
    quiet
        .files
        .push(file("/public/project/code.rs", FileOp::Create, 50));
    quiet.files.push(file("relative.md", FileOp::Create, 50));
    let mut sessions = vec![quiet];
    for i in 0..5 {
        let mut o = observed(Agent::Claude, &format!("s{i}"), 100 + i);
        o.offer_title(&format!("Task {i}"), TitleSource::Ai);
        o.files.push(file(
            &format!("/public/project/notes-{i}.md"),
            FileOp::Create,
            100 + i,
        ));
        o.files.push(file(
            &format!("/public/project/notes-{i}.md"),
            FileOp::Edit,
            200 + i,
        ));
        sessions.push(o);
    }
    store.record_agent_activity(&sessions, None).unwrap();
    let page = store.agent_activity_page(None, 0, 2, None).unwrap();
    assert_eq!(
        page.sessions
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        ["Task 4", "Task 3"]
    );
    assert_eq!(page.next_offset, Some(2));
    assert_eq!(page.sessions[0].file_count, 1);
    let last = store.agent_activity_page(None, 4, 2, None).unwrap();
    assert_eq!(last.sessions.len(), 1);
    assert_eq!(last.next_offset, None);
    assert!(store
        .agent_activity_page(Some(Agent::Codex), 0, 10, None)
        .unwrap()
        .sessions
        .is_empty());
    let by_file = store
        .agent_activity_page(None, 0, 10, Some("NOTES-2"))
        .unwrap();
    assert_eq!(by_file.sessions.len(), 1);
    assert_eq!(by_file.sessions[0].title, "Task 2");
    assert!(store
        .agent_activity_page(None, 0, 10, Some("100%_"))
        .unwrap()
        .sessions
        .is_empty());
    let id = by_file.sessions[0].id.clone();
    let (cwd, files) = store.agent_activity_files(&id).unwrap();
    assert_eq!(cwd.as_deref(), Some("/public/project"));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].op, FileOp::Create);
    assert_eq!((files[0].first_at_ms, files[0].last_at_ms), (102, 202));
    store.hide_agent_session(&id).unwrap();
    assert!(store.agent_activity_files(&id).is_err());
    assert_eq!(
        store
            .agent_activity_page(None, 0, 10, None)
            .unwrap()
            .sessions
            .len(),
        4
    );
    let tracking = store.agent_tracking().unwrap();
    assert_eq!(tracking[0].sessions, 4);
    assert!(!tracking[0].enabled);
}

#[test]
fn cursors_commit_with_their_observations_and_forget_is_per_agent() {
    let (temp, mut store) = store();
    let mut o = observed(Agent::Codex, "thread", 10);
    o.files
        .push(file("/public/project/plan.md", FileOp::Create, 10));
    let cursor = Cursor {
        log_path: "/logs/rollout.jsonl".into(),
        agent: Agent::Codex,
        client_session_id: Some("thread".into()),
        byte_offset: 42,
        mtime_ms: 7,
        size: 42,
    };
    let before = store.agent_activity_revision().unwrap();
    store.record_agent_activity(&[o], Some(&cursor)).unwrap();
    assert!(store.agent_activity_revision().unwrap() > before);
    let mut claude = observed(Agent::Claude, "kept", 10);
    claude
        .files
        .push(file("/public/project/kept.md", FileOp::Edit, 10));
    store.record_agent_activity(&[claude], None).unwrap();
    drop(store);
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    assert_eq!(
        store.agent_ingest_cursor("/logs/rollout.jsonl").unwrap(),
        Some(cursor)
    );
    store.forget_agent_activity(Agent::Codex).unwrap();
    assert_eq!(
        store.agent_ingest_cursor("/logs/rollout.jsonl").unwrap(),
        None
    );
    let left = store
        .agent_activity_page(None, 0, 10, None)
        .unwrap()
        .sessions;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].agent, Agent::Claude);
}

#[test]
fn tracking_starts_with_thirty_days_and_load_older_only_widens() {
    let (_temp, mut store) = store();
    let now = 100 * 86_400_000;
    store.set_agent_tracking(Agent::Claude, true, now).unwrap();
    store.extend_agent_history(10, now).unwrap();
    let claude = &store.agent_tracking().unwrap()[0];
    assert!(claude.enabled);
    assert_eq!(claude.since_ms, Some(70 * 86_400_000));
    store.extend_agent_history(90, now).unwrap();
    assert_eq!(
        store.agent_tracking().unwrap()[0].since_ms,
        Some(10 * 86_400_000)
    );
    assert!(store.extend_agent_history(0, now).is_err());
    assert_eq!(store.agent_tracking().unwrap()[1].since_ms, None);
}

fn legacy_store() -> (tempfile::TempDir, ReaderStore, PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("project")).unwrap();
    let root = temp.path().join("project").canonicalize().unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/note.md"), "# Public note").unwrap();
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    let source = store
        .add_local_source(
            "Public project",
            crate::SourceRoot::capture(root.clone()).unwrap(),
        )
        .unwrap();
    // Create the prior namespace through its own owner, then seed rows.
    store.agent_sessions(&source.id).unwrap();
    store
        .with_library_connection(|db| -> rusqlite::Result<()> {
            db.execute("INSERT INTO reader_agent_sessions VALUES('old-1',?1,'conn','client-a','claude','Claude Code session',5,9,NULL)", [&source.id])?;
            db.execute("INSERT INTO reader_agent_sessions VALUES('old-2',?1,'conn','client-b','codex','Release notes',6,8,8)", [&source.id])?;
            db.execute("INSERT INTO reader_agent_documents VALUES('old-1','docs/note.md',7)", [])?;
            db.execute("INSERT INTO reader_agent_documents VALUES('old-2','../escape.md',7)", [])?;
            db.execute("INSERT INTO reader_agent_source_documents VALUES('old-2',?1,'docs/note.md',8)", [&source.id])?;
            Ok(())
        })
        .unwrap();
    (temp, store, root, source.id)
}

#[test]
fn prior_project_sessions_migrate_with_absolute_paths_and_reopen_once() {
    let (temp, mut store, root, _source) = legacy_store();
    let page = store.agent_activity_page(None, 0, 10, None).unwrap();
    assert_eq!(page.sessions.len(), 2);
    let codex = page
        .sessions
        .iter()
        .find(|s| s.agent == Agent::Codex)
        .unwrap();
    assert_eq!(codex.title, "Release notes");
    assert_eq!(codex.title_source, TitleSource::Custom);
    assert_eq!(codex.ended_at_ms, Some(8));
    let claude = page
        .sessions
        .iter()
        .find(|s| s.agent == Agent::Claude)
        .unwrap();
    assert_eq!(claude.title_source, TitleSource::Fallback);
    assert_eq!(claude.cwd.as_deref(), root.to_str());
    let note = root.join("docs/note.md");
    for session in &page.sessions {
        let (_, files) = store.agent_activity_files(&session.id).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|f| f.abs_path.as_str())
                .collect::<Vec<_>>(),
            [note.to_str().unwrap()]
        );
    }
    drop(store);
    let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    assert_eq!(
        store
            .agent_activity_page(None, 0, 10, None)
            .unwrap()
            .sessions
            .len(),
        2
    );
    // The old tables remain readable and unchanged.
    let old: i64 = store
        .with_library_connection(|db| {
            db.query_row("SELECT count(*) FROM reader_agent_sessions", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(old, 2);
}

#[test]
fn an_interrupted_migration_leaves_prior_rows_and_replays() {
    let (_temp, mut store, _root, _source) = legacy_store();
    // A foreign temp object fails validation after the import ran inside the
    // transition transaction, standing in for a crash before commit.
    store
        .with_library_connection(|db| {
            db.execute_batch("CREATE TEMP TABLE reader_agent_activity_shadow(x)")
        })
        .unwrap();
    assert!(store.agent_activity_page(None, 0, 10, None).is_err());
    let partial: bool = store
        .with_library_connection(|db| db.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE name GLOB 'reader_agent_activity_*')", [], |r| r.get(0)))
        .unwrap();
    assert!(!partial);
    store
        .with_library_connection(|db| {
            db.execute_batch("DROP TABLE temp.reader_agent_activity_shadow")
        })
        .unwrap();
    assert_eq!(
        store
            .agent_activity_page(None, 0, 10, None)
            .unwrap()
            .sessions
            .len(),
        2
    );
}

#[test]
fn a_newer_schema_is_rejected_without_reset() {
    let (_temp, mut store) = store();
    store.agent_activity_revision().unwrap();
    store
        .with_library_connection(|db| {
            db.execute("UPDATE reader_agent_activity_meta SET version=99", [])
        })
        .unwrap();
    assert!(matches!(
        store.agent_activity_revision(),
        Err(crate::ReaderError::NewerDatabase(99))
    ));
    let version: i64 = store
        .with_library_connection(|db| {
            db.query_row("SELECT version FROM reader_agent_activity_meta", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(version, 99);
}

fn private(store: &mut ReaderStore, path: &Path) {
    store
        .with_library_connection(|db| {
            db.execute(
                "INSERT OR IGNORE INTO reader_local_private_v1 VALUES(?1)",
                [crate::store::encode_path(path)],
            )
        })
        .unwrap();
}

#[test]
fn agent_files_open_exactly_and_respect_private_namespaces() {
    let (temp, mut store) = store();
    let base = temp.path().canonicalize().unwrap();
    let agent_home = base.join(".claude");
    std::fs::create_dir_all(agent_home.join("plans")).unwrap();
    std::fs::create_dir_all(agent_home.join("projects")).unwrap();
    std::fs::create_dir_all(base.join("repo")).unwrap();
    private(&mut store, &agent_home);
    let plan = agent_home.join("plans/public-plan.md");
    let hidden = agent_home.join("projects/private.md");
    let note = base.join("repo/NOTE.md");
    let sibling = base.join("repo/sibling.md");
    for path in [&plan, &hidden, &note, &sibling] {
        std::fs::write(path, "# Public").unwrap();
    }
    let mut o = observed(Agent::Claude, "s", 1);
    for path in [&plan, &hidden, &note] {
        o.files.push(FileObservation {
            abs_path: path.clone(),
            op: FileOp::Edit,
            at_ms: 1,
        });
    }
    o.files.push(file(
        base.join("repo/gone.md").to_str().unwrap(),
        FileOp::Create,
        1,
    ));
    store.record_agent_activity(&[o], None).unwrap();
    let id = store.agent_session_id(Agent::Claude, "s").unwrap().unwrap();
    let (_, files) = store.agent_activity_files(&id).unwrap();
    let private = store.agent_private_paths().unwrap();
    let mut refs = std::collections::HashMap::new();
    for f in &files {
        refs.insert(
            f.abs_path.clone(),
            file_reference(&private, &f.abs_path, &f.exact_id),
        );
    }
    assert_eq!(
        refs[hidden.to_str().unwrap()],
        Err(FileUnavailable::Private)
    );
    assert_eq!(
        refs[base.join("repo/gone.md").to_str().unwrap()],
        Err(FileUnavailable::Missing)
    );
    let plan_ref = refs[plan.to_str().unwrap()].clone().unwrap();
    assert_eq!(plan_ref.path, "public-plan.md");
    let note_ref = refs[note.to_str().unwrap()].clone().unwrap();
    let recorded = store.agent_file_path(&note_ref.source_id).unwrap().unwrap();
    assert_eq!(recorded, note.to_str().unwrap());
    let read = |path: &str, revision: Option<&str>| {
        read_file(&private, &recorded, &note_ref.source_id, path, revision)
    };
    assert_eq!(
        read(&note_ref.path, Some(&note_ref.revision))
            .unwrap()
            .markdown,
        "# Public"
    );
    // Only the recorded file resolves through its id, never a sibling.
    assert!(read("sibling.md", None).is_err());
    assert_eq!(store.agent_file_path(&"0".repeat(64)).unwrap(), None);
    assert_eq!(store.agent_file_path("not-hex").unwrap(), None);
    std::fs::write(&note, "# Changed").unwrap();
    assert!(matches!(
        read(&note_ref.path, Some(&note_ref.revision)),
        Err(crate::ReaderError::Conflict)
    ));
}
