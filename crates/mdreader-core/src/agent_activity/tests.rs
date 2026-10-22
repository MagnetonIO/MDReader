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
    // The fallback date is the viewer's local date (1 s after midnight UTC
    // is still the previous day at UTC-05:00).
    let west = store
        .agent_activity_page_at(None, 0, 10, None, -300)
        .unwrap();
    assert_eq!(west.sessions[0].title, "project · 1969-12-31");
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
    assert_eq!(
        page.totals,
        Totals {
            claude: 5,
            codex: 0
        }
    );
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
    assert_eq!(by_file.totals.claude, 1);
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
    // The prior namespace exactly as its last version (2) created it.
    store
        .with_library_connection(|db| -> rusqlite::Result<()> {
            db.execute_batch(include_str!("legacy_schema_v2.sql"))?;
            db.execute("INSERT INTO reader_agent_sessions_meta VALUES(1,2)", [])?;
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
fn agent_files_are_indexed_and_opened_exactly_and_private_namespaces_stay_closed() {
    let (temp, mut store) = store();
    let base = temp.path().canonicalize().unwrap();
    let agent_home = base.join(".claude");
    for dir in ["plans", "projects"] {
        std::fs::create_dir_all(agent_home.join(dir)).unwrap();
    }
    std::fs::create_dir_all(base.join("repo")).unwrap();
    std::fs::create_dir_all(base.join("library")).unwrap();
    private(&mut store, &agent_home);
    let library = store
        .add_local_source(
            "Library",
            crate::SourceRoot::capture(base.join("library")).unwrap(),
        )
        .unwrap()
        .id;
    let plan = agent_home.join("plans/public-plan.md");
    let hidden = agent_home.join("projects/private.md");
    let note = base.join("repo/NOTE.md");
    let sibling = base.join("repo/sibling.md");
    let inside = base.join("library/inside.md");
    std::fs::write(&plan, "# Plan\nsyntheticplanword").unwrap();
    std::fs::write(&hidden, "# Private\nsyntheticprivateword").unwrap();
    std::fs::write(&note, "# Note\nsyntheticnoteword").unwrap();
    std::fs::write(&sibling, "# Sibling\nsyntheticsiblingword").unwrap();
    std::fs::write(&inside, "# Inside").unwrap();
    let mut o = observed(Agent::Claude, "s", 1);
    for path in [&plan, &hidden, &note, &inside] {
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
    let at = |store: &mut ReaderStore, path: &Path| {
        store.agent_file_location(path.to_str().unwrap()).unwrap()
    };
    assert_eq!(
        at(&mut store, &hidden),
        None,
        "private files are never admitted"
    );
    assert_eq!(at(&mut store, &base.join("repo/gone.md")), None);
    // A file inside an existing source keeps that source's reference.
    assert_eq!(
        at(&mut store, &inside),
        Some((library.clone(), "inside.md".into()))
    );
    let (root, path) = at(&mut store, &note).unwrap();
    assert_eq!(path, "NOTE.md");
    let (plans_root, _) = at(&mut store, &plan).unwrap();
    // Roots are internal: never listed, chosen as projects or granted.
    let listed: Vec<_> = store
        .list_sources()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(listed, std::slice::from_ref(&library));
    assert!(store.chosen_local_root(&root).is_err());
    assert_eq!(
        store.agent_file_at(&root, "NOTE.md").unwrap().as_deref(),
        note.to_str()
    );
    assert_eq!(store.agent_file_at(&library, "inside.md").unwrap(), None);
    // Only the recorded file is readable or indexed through its root.
    assert!(store.read_document(&root, "sibling.md", None).is_err());
    assert_eq!(
        store
            .read_document(&root, "NOTE.md", None)
            .unwrap()
            .markdown,
        "# Note\nsyntheticnoteword"
    );
    for job in store.pending_index_jobs().unwrap() {
        store.index_source(&job).unwrap();
    }
    let scope = store.agent_file_sources().unwrap();
    assert!(scope.contains(&root) && scope.contains(&plans_root));
    let hits = store.search("syntheticnoteword", &scope, 10).unwrap();
    assert_eq!(
        hits.iter()
            .map(|h| (h.source_id.as_str(), h.path.as_str()))
            .collect::<Vec<_>>(),
        [(root.as_str(), "NOTE.md")]
    );
    assert_eq!(
        store.search("syntheticplanword", &scope, 10).unwrap().len(),
        1
    );
    assert!(store
        .search("syntheticsiblingword", &scope, 10)
        .unwrap()
        .is_empty());
    // A changed file is re-indexed on its next recording.
    std::fs::write(&note, "# Note\nsyntheticrevisedword").unwrap();
    let mut again = observed(Agent::Claude, "s", 2);
    again.files.push(FileObservation {
        abs_path: note.clone(),
        op: FileOp::Edit,
        at_ms: 2,
    });
    store.record_agent_activity(&[again], None).unwrap();
    for job in store.pending_index_jobs().unwrap() {
        store.index_source(&job).unwrap();
    }
    assert_eq!(
        store
            .search("syntheticrevisedword", &scope, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .search("syntheticnoteword", &scope, 10)
        .unwrap()
        .is_empty());
    // References read the original; a stale revision is a conflict.
    let private = store.agent_private_paths().unwrap();
    let reference = file_reference(&private, note.to_str().unwrap(), &root, "NOTE.md").unwrap();
    let read = |revision: &str| {
        read_file(
            &private,
            note.to_str().unwrap(),
            &root,
            "NOTE.md",
            Some(revision),
        )
    };
    assert!(read(&reference.revision).is_ok());
    std::fs::write(&note, "# Edited").unwrap();
    assert!(matches!(
        read(&reference.revision),
        Err(crate::ReaderError::Conflict)
    ));
    // Adding the folder to the library claims the root as an ordinary source.
    let claimed = store
        .add_local_source(
            "Repo",
            crate::SourceRoot::capture(base.join("repo")).unwrap(),
        )
        .unwrap();
    assert_eq!(claimed.id, root);
    assert_eq!(store.agent_file_at(&root, "NOTE.md").unwrap(), None);
    assert!(store.read_document(&root, "sibling.md", None).is_ok());
}

#[test]
fn forgetting_an_agent_retires_roots_left_without_files() {
    let (temp, mut store) = store();
    let repo = temp.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("x.md"), "# X\nsyntheticforgetword").unwrap();
    let mut o = observed(Agent::Codex, "c", 1);
    o.files.push(FileObservation {
        abs_path: repo.join("x.md"),
        op: FileOp::Create,
        at_ms: 1,
    });
    store.record_agent_activity(&[o], None).unwrap();
    for job in store.pending_index_jobs().unwrap() {
        store.index_source(&job).unwrap();
    }
    let scope = store.agent_file_sources().unwrap();
    assert_eq!(
        store
            .search("syntheticforgetword", &scope, 10)
            .unwrap()
            .len(),
        1
    );
    store.forget_agent_activity(Agent::Codex).unwrap();
    assert!(store.agent_file_sources().unwrap().is_empty());
    assert_eq!(
        store
            .agent_file_location(repo.join("x.md").to_str().unwrap())
            .unwrap(),
        None
    );
}

#[cfg(unix)]
#[test]
fn linked_files_named_markdown_are_never_opened_admitted_or_indexed() {
    let (temp, mut store) = store();
    let base = temp.path().canonicalize().unwrap();
    let agent_home = base.join(".claude");
    std::fs::create_dir_all(agent_home.join("plans")).unwrap();
    std::fs::create_dir_all(agent_home.join("projects")).unwrap();
    std::fs::create_dir_all(base.join("tmp")).unwrap();
    private(&mut store, &agent_home);
    let secret = base.join("taxes.txt");
    std::fs::write(&secret, "syntheticsecretword").unwrap();
    let transcript = agent_home.join("projects/session.jsonl");
    std::fs::write(&transcript, "{\"syntheticTranscriptWord\":1}").unwrap();
    let symlinked = base.join("tmp/n.md");
    std::os::unix::fs::symlink(&secret, &symlinked).unwrap();
    // A hard link from a private transcript into the open plans folder.
    let hardlinked = agent_home.join("plans/s.md");
    std::fs::hard_link(&transcript, &hardlinked).unwrap();
    let mut o = observed(Agent::Claude, "s", 1);
    for path in [&symlinked, &hardlinked] {
        o.files.push(FileObservation {
            abs_path: path.clone(),
            op: FileOp::Create,
            at_ms: 1,
        });
    }
    store.record_agent_activity(&[o], None).unwrap();
    let private_paths = store.agent_private_paths().unwrap();
    for path in [&symlinked, &hardlinked] {
        let text = path.to_str().unwrap();
        assert_eq!(store.agent_file_location(text).unwrap(), None, "{text}");
        assert_eq!(
            file_reference(&private_paths, text, "any", "x.md"),
            Err(FileUnavailable::Unreadable),
            "{text}"
        );
        assert!(read_file(&private_paths, text, "any", "x.md", None).is_err());
    }
    // A root admitted for a plain file still refuses a hard-linked sibling
    // that later takes a recorded name.
    let plain = base.join("tmp/plain.md");
    std::fs::write(&plain, "# Plain").unwrap();
    let mut o = observed(Agent::Claude, "s", 2);
    o.files.push(FileObservation {
        abs_path: plain.clone(),
        op: FileOp::Create,
        at_ms: 2,
    });
    store.record_agent_activity(&[o], None).unwrap();
    let (root, path) = store
        .agent_file_location(plain.to_str().unwrap())
        .unwrap()
        .unwrap();
    std::fs::remove_file(&plain).unwrap();
    std::fs::hard_link(&secret, &plain).unwrap();
    assert!(store.read_document(&root, &path, None).is_err());
}

#[cfg(unix)]
#[test]
fn a_fifo_named_markdown_is_refused_without_blocking() {
    let (temp, mut store) = store();
    let base = temp.path().canonicalize().unwrap();
    let fifo = base.join("pipe.md");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let private = store.agent_private_paths().unwrap();
    let path = fifo.to_str().unwrap().to_owned();
    // On a blocking open the reader would wait for a writer forever.
    let (tx, rx) = std::sync::mpsc::channel();
    let probe = path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(file_reference(&private, &probe, "any", "pipe.md"));
    });
    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("opening a FIFO must not block");
    assert!(outcome.is_err());
    let mut o = observed(Agent::Claude, "s", 1);
    o.files.push(FileObservation {
        abs_path: fifo.clone(),
        op: FileOp::Create,
        at_ms: 1,
    });
    store.record_agent_activity(&[o], None).unwrap();
    assert_eq!(store.agent_file_location(&path).unwrap(), None);
    // A recorded file of a root later replaced by a FIFO is refused too.
    let plain = base.join("plain.md");
    std::fs::write(&plain, "# Plain").unwrap();
    let mut o = observed(Agent::Claude, "s", 2);
    o.files.push(FileObservation {
        abs_path: plain.clone(),
        op: FileOp::Create,
        at_ms: 2,
    });
    store.record_agent_activity(&[o], None).unwrap();
    let (root, relative) = store
        .agent_file_location(plain.to_str().unwrap())
        .unwrap()
        .unwrap();
    std::fs::remove_file(&plain).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .arg(&plain)
        .status()
        .unwrap()
        .success());
    let (tx, rx) = std::sync::mpsc::channel();
    let directory = temp.path().join("reader");
    std::thread::spawn(move || {
        let store = ReaderStore::open(&directory).unwrap();
        let _ = tx.send(store.read_document(&root, &relative, None).is_err());
    });
    assert!(rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("reading a FIFO through a root must not block"));
}

#[test]
fn schema_validation_runs_once_per_connection_until_the_schema_changes() {
    let (_temp, mut store) = store();
    let runs = || schema::VALIDATIONS.with(|n| n.get());
    store.agent_activity_revision().unwrap();
    let first = runs();
    for _ in 0..5 {
        store.agent_activity_revision().unwrap();
        store.agent_tracking().unwrap();
    }
    assert_eq!(runs(), first, "unchanged schema is not revalidated");
    store
        .with_library_connection(|db| db.execute_batch("CREATE TABLE unrelated_probe(x)"))
        .unwrap();
    store.agent_activity_revision().unwrap();
    assert_eq!(runs(), first + 1, "any schema change revalidates");
}
