//! Synthetic logs only (`fixtures/agent-logs`); never real transcripts.
use super::*;
use mdreader_core::agent_activity::TitleSource;
use std::sync::Mutex;

const MARKER: &[u8] = b"PRIVATE-BODY-MARKER";

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    homes: Homes,
    store: Mutex<ReaderStore>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap().join("home");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent-logs");
        copy(&source.join("claude"), &home.join(".claude"));
        copy(&source.join("codex"), &home.join(".codex"));
        let store = Mutex::new(ReaderStore::open(&temp.path().join("reader")).unwrap());
        Self {
            homes: Homes::under(&home),
            temp,
            store,
        }
    }
    fn run(&self, request: ActivityRequest) -> Result<Value> {
        execute(
            &|| self.store.lock().map_err(|_| ReaderError::Unavailable),
            &self.homes,
            request,
        )
    }
    fn enable(&self, agent: Agent) {
        self.run(ActivityRequest::SetTracking {
            agent,
            enabled: true,
        })
        .unwrap();
    }
    fn sync(&self) -> Vec<String> {
        let paths: Vec<PathBuf> =
            serde_json::from_value(self.run(ActivityRequest::Discover).unwrap()).unwrap();
        let reply: IngestReply =
            serde_json::from_value(self.run(ActivityRequest::Ingest { paths }).unwrap()).unwrap();
        assert!(reply.pending.is_empty());
        reply.changed
    }
    fn page(&self) -> Page {
        serde_json::from_value(
            self.run(ActivityRequest::List {
                agent: None,
                offset: 0,
                limit: 50,
                filter: None,
                utc_offset_minutes: None,
            })
            .unwrap(),
        )
        .unwrap()
    }
    fn files(&self, title: &str) -> Vec<ActivityFile> {
        let page = self.page();
        let session = page
            .sessions
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| {
                panic!(
                    "{title}: {:?}",
                    page.sessions.iter().map(|s| &s.title).collect::<Vec<_>>()
                )
            });
        serde_json::from_value(
            self.run(ActivityRequest::Files {
                session_id: session.id.clone(),
            })
            .unwrap(),
        )
        .unwrap()
    }
    fn main_log(&self) -> PathBuf {
        self.homes
            .claude_projects
            .join("-public-project/11111111-1111-4111-8111-111111111111.jsonl")
    }
}

fn paths(files: &[ActivityFile]) -> Vec<(&str, FileOp)> {
    let mut out: Vec<_> = files.iter().map(|f| (f.abs_path.as_str(), f.op)).collect();
    out.sort();
    out
}

#[test]
fn claude_titles_files_and_subagents_come_from_synthetic_transcripts() {
    let f = Fixture::new();
    f.enable(Agent::Claude);
    assert_eq!(f.sync().len(), 3);
    let page = f.page();
    let titles: Vec<_> = page
        .sessions
        .iter()
        .map(|s| (s.title.as_str(), s.title_source))
        .collect();
    // The session that only read files is recorded but not listed.
    assert_eq!(
        titles,
        [
            ("Synthetic release checklist", TitleSource::Ai),
            ("Onboarding notes (renamed)", TitleSource::Custom),
        ]
    );
    let first = &page.sessions[0];
    assert_eq!(first.cwd.as_deref(), Some("/public/project"));
    assert_eq!(first.git_branch.as_deref(), Some("feature/checklist"));
    assert_eq!(
        first.started_at_ms,
        mdreader_core::agent_activity::clock::parse_rfc3339_ms("2026-10-20T10:00:00Z").unwrap()
    );
    let files = f.files("Synthetic release checklist");
    assert_eq!(
        paths(&files),
        [
            (
                "/public/home/.claude/plans/synthetic-plan.md",
                FileOp::Create
            ),
            ("/public/project/README.md", FileOp::Edit),
            ("/public/project/docs/checklist.md", FileOp::Create),
            ("/public/project/docs/relative.md", FileOp::Edit),
            ("/public/project/docs/research.md", FileOp::Create),
        ]
    );
    let readme = files
        .iter()
        .find(|f| f.abs_path.ends_with("README.md"))
        .unwrap();
    assert_eq!(readme.display_path, "README.md");
    assert!(!readme.exists);
    assert_eq!(readme.unavailable, Some(FileUnavailable::Missing));
    let plan = files
        .iter()
        .find(|f| f.abs_path.ends_with("synthetic-plan.md"))
        .unwrap();
    assert!(!plan.is_plan, "only this machine's plans folder is tagged");
}

#[test]
fn an_incomplete_last_line_waits_and_offsets_resume_without_rereading() {
    let f = Fixture::new();
    f.enable(Agent::Claude);
    f.sync();
    assert!(!paths(&f.files("Synthetic release checklist"))
        .iter()
        .any(|(p, _)| p.ends_with("truncated.md")));
    let key = f.main_log();
    let cursor = f
        .store
        .lock()
        .unwrap()
        .agent_ingest_cursor(key.to_str().unwrap())
        .unwrap()
        .unwrap();
    assert!(cursor.byte_offset < cursor.size);
    // Nothing new: no re-read and no change.
    assert!(f.sync().is_empty());
    use std::io::Write;
    let mut log = std::fs::OpenOptions::new().append(true).open(&key).unwrap();
    log.write_all(b"\"}}]}}\n").unwrap();
    writeln!(log, r#"{{"type":"assistant","sessionId":"x","timestamp":"2026-10-20T11:00:00.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t9","name":"Write","input":{{"file_path":"/public/project/docs/later.md","content":"PRIVATE-BODY-MARKER"}}}}]}}}}"#).unwrap();
    drop(log);
    assert_eq!(f.sync().len(), 1);
    let files = paths(&f.files("Synthetic release checklist"))
        .into_iter()
        .map(|(p, _)| p.to_owned())
        .collect::<Vec<_>>();
    assert!(files.iter().any(|p| p.ends_with("truncated.md")));
    assert!(files.iter().any(|p| p.ends_with("later.md")));
    let after = f
        .store
        .lock()
        .unwrap()
        .agent_ingest_cursor(key.to_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(after.byte_offset, after.size);
}

#[test]
fn a_rewritten_log_is_rescanned_from_the_start() {
    let f = Fixture::new();
    f.enable(Agent::Claude);
    f.sync();
    let key = f.main_log();
    std::fs::write(&key, concat!(
        r#"{"type":"assistant","timestamp":"2026-10-21T08:00:00.000Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"r1","name":"Write","input":{"file_path":"/public/project/docs/rotated.md","content":""}}]}}"#,
        "\n"
    )).unwrap();
    f.sync();
    assert!(paths(&f.files("Synthetic release checklist"))
        .iter()
        .any(|(p, _)| p.ends_with("rotated.md")));
}

#[test]
fn codex_file_changes_patches_and_thread_names_are_recorded() {
    let f = Fixture::new();
    f.enable(Agent::Codex);
    f.sync();
    let page = f.page();
    let titles: Vec<_> = page
        .sessions
        .iter()
        .map(|s| (s.title.as_str(), s.title_source))
        .collect();
    assert_eq!(
        titles,
        [
            ("Reader web sharing", TitleSource::Custom),
            ("Codex · 2025-06-01", TitleSource::Fallback)
        ]
    );
    assert_eq!(page.sessions[0].git_branch.as_deref(), Some("feat/sharing"));
    assert_eq!(
        paths(&f.files("Reader web sharing")),
        [
            ("/public/web/docs/new-name.md", FileOp::Edit),
            ("/public/web/docs/relative-notes.md", FileOp::Create),
            ("/public/web/docs/sharing.md", FileOp::Create),
        ]
    );
    assert_eq!(
        paths(&f.files("Codex · 2025-06-01")),
        [("/public/legacy/LEGACY.md", FileOp::Create)]
    );
    // Claude logs are not read while only Codex is tracked.
    assert!(f
        .store
        .lock()
        .unwrap()
        .agent_ingest_cursor(f.main_log().to_str().unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn only_known_log_locations_of_enabled_agents_are_read() {
    let f = Fixture::new();
    let outside = f.temp.path().join("elsewhere.jsonl");
    std::fs::copy(f.main_log(), &outside).unwrap();
    let ingest = |paths: Vec<PathBuf>| f.run(ActivityRequest::Ingest { paths }).unwrap();
    ingest(vec![f.main_log()]);
    assert!(f.page().sessions.is_empty(), "tracking is off by default");
    f.enable(Agent::Claude);
    ingest(vec![
        outside.clone(),
        f.homes
            .claude_projects
            .join("../projects/-public-project/../x.jsonl"),
    ]);
    assert!(f.page().sessions.is_empty());
    for bad in [
        outside,
        PathBuf::from("relative.jsonl"),
        f.homes.claude_projects.join("a/b/c.jsonl"),
    ] {
        assert_eq!(ingest::classify(&f.homes, &bad), None, "{bad:?}");
    }
    assert!(f
        .run(ActivityRequest::Ingest {
            paths: vec![PathBuf::from("/x"); MAX_INGEST_PATHS + 1]
        })
        .is_err());
}

#[test]
fn agent_files_open_through_exact_references_and_the_store_keeps_no_message_text() {
    let f = Fixture::new();
    let repo = f.temp.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let note = repo.join("design.md");
    std::fs::write(&note, "# Public design").unwrap();
    std::fs::write(
        repo.join("unrecorded.md"),
        "# Sibling\nsyntheticunrecordedword",
    )
    .unwrap();
    let log = f
        .homes
        .claude_projects
        .join("-public-project/66666666-6666-4666-8666-666666666666.jsonl");
    std::fs::write(&log, format!(
        "{}\n",
        serde_json::json!({"type":"assistant","cwd":repo,"timestamp":"2026-10-21T08:00:00.000Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"w","name":"Write","input":{"file_path":note,"content":"PRIVATE-BODY-MARKER"}}]}})
    )).unwrap();
    f.enable(Agent::Claude);
    f.enable(Agent::Codex);
    let before: i64 = serde_json::from_value(f.run(ActivityRequest::Revision).unwrap()).unwrap();
    f.sync();
    let after: i64 = serde_json::from_value(f.run(ActivityRequest::Revision).unwrap()).unwrap();
    assert!(after > before);
    let title = format!(
        "repo · {}",
        mdreader_core::agent_activity::clock::utc_date(
            mdreader_core::agent_activity::clock::parse_rfc3339_ms("2026-10-21T08:00:00Z").unwrap()
        )
    );
    let files = f.files(&title);
    assert_eq!(files[0].display_path, "design.md");
    let reference = files[0].reference.clone().unwrap();
    let read = |revision: Option<String>| {
        f.run(ActivityRequest::ReadExact {
            source_id: reference.source_id.clone(),
            path: reference.path.clone(),
            revision,
        })
    };
    assert_eq!(
        read(Some(reference.revision.clone())).unwrap()["markdown"],
        "# Public design"
    );
    // The file outside every source is searchable; its unrecorded sibling is not.
    {
        let mut store = f.store.lock().unwrap();
        for job in store.pending_index_jobs().unwrap() {
            store.index_source(&job).unwrap();
        }
        let scope = store.agent_file_sources().unwrap();
        let hits = store.search("design", &scope, 10).unwrap();
        assert_eq!(
            hits.iter()
                .map(|h| (h.source_id.clone(), h.path.clone(), h.revision.clone()))
                .collect::<Vec<_>>(),
            [(
                reference.source_id.clone(),
                reference.path.clone(),
                reference.revision.clone()
            )]
        );
        assert!(store
            .search("syntheticunrecordedword", &scope, 10)
            .unwrap()
            .is_empty());
    }
    std::fs::write(&note, "# Edited").unwrap();
    assert!(matches!(
        read(Some(reference.revision.clone())),
        Err(ReaderError::Conflict)
    ));
    assert_eq!(
        f.run(ActivityRequest::ReadExact {
            source_id: "f".repeat(64),
            path: "design.md".into(),
            revision: None
        })
        .unwrap(),
        Value::Null
    );
    // Privacy: no message, thinking, tool body or later prompt text persisted.
    drop(f.store.lock().unwrap());
    for entry in std::fs::read_dir(f.temp.path().join("reader")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes.windows(MARKER.len()).any(|w| w == MARKER),
                "{path:?}"
            );
            assert!(
                !bytes.windows(13).any(|w| w == b"A later promp"),
                "{path:?}"
            );
        }
    }
}

#[test]
fn forget_and_hide_and_rename_round_trip() {
    let f = Fixture::new();
    f.enable(Agent::Claude);
    f.enable(Agent::Codex);
    f.sync();
    let page = f.page();
    assert_eq!(page.sessions.len(), 4);
    let id = page.sessions[0].id.clone();
    f.run(ActivityRequest::Rename {
        session_id: id.clone(),
        title: "Mine".into(),
    })
    .unwrap();
    assert_eq!(f.page().sessions[0].title, "Mine");
    f.run(ActivityRequest::Hide { session_id: id }).unwrap();
    assert_eq!(f.page().sessions.len(), 3);
    let state: Vec<TrackingState> = serde_json::from_value(
        f.run(ActivityRequest::Forget {
            agent: Agent::Codex,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(state.iter().all(|s| s.available && s.enabled));
    assert_eq!(
        state
            .iter()
            .find(|s| s.agent == Agent::Codex)
            .unwrap()
            .sessions,
        0
    );
    assert!(f.page().sessions.iter().all(|s| s.agent == Agent::Claude));
}

#[cfg(unix)]
#[test]
fn a_fifo_named_like_a_log_does_not_block_ingest() {
    let f = Fixture::new();
    f.enable(Agent::Claude);
    let fifo = f
        .homes
        .claude_projects
        .join("-public-project/88888888-8888-4888-8888-888888888888.jsonl");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let (tx, rx) = std::sync::mpsc::channel();
    let homes = f.homes.clone();
    let temp = f.temp.path().to_owned();
    std::thread::spawn(move || {
        let store = Mutex::new(ReaderStore::open(&temp.join("reader")).unwrap());
        let reply = execute(
            &|| store.lock().map_err(|_| ReaderError::Unavailable),
            &homes,
            ActivityRequest::Ingest { paths: vec![fifo] },
        );
        let _ = tx.send(reply.is_ok());
    });
    assert!(rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("ingesting a FIFO must not block"));
}

#[test]
fn a_read_racing_remove_tracked_sessions_writes_nothing_back() {
    let f = Fixture::new();
    f.enable(Agent::Claude);
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let log = f.main_log();
    // The batch reads "enabled" and the cursor, parses, and only then takes
    // the lock to commit. The user turns tracking off and removes the
    // sessions just before that commit.
    let lock = || -> Result<std::sync::MutexGuard<'_, ReaderStore>> {
        let mut store = f.store.lock().map_err(|_| ReaderError::Unavailable)?;
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
            store.set_agent_tracking(Agent::Claude, false, 0)?;
            store.forget_agent_activity(Agent::Claude)?;
        }
        Ok(store)
    };
    execute(
        &lock,
        &f.homes,
        ActivityRequest::Ingest {
            paths: vec![log.clone()],
        },
    )
    .unwrap();
    let mut store = f.store.lock().unwrap();
    assert!(store
        .agent_activity_page(None, 0, 10, None)
        .unwrap()
        .sessions
        .is_empty());
    assert_eq!(
        store.agent_ingest_cursor(log.to_str().unwrap()).unwrap(),
        None
    );
}
