//! Actual interrupted Windows journals, not a simulated recovery status.
use super::*;
use mdreader_capture::{
    Adapter, CaptureOrigin, EnqueueOutcome, Limits, PrivateDirectory, ProjectPolicy, Spool,
};
use std::{io::Read, path::Path, process::Command};

const CHILD_ROOT: &str = "MDREADER_SYNTHETIC_JOURNAL_CHILD";
const CRASH: i32 = 72;
const HOT_HEADER: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

fn journal_header(root: &Path, name: &str) -> [u8; 8] {
    let private = PrivateDirectory::open(root).unwrap();
    let mut journal = private
        .database_file(Path::new(name), false)
        .unwrap()
        .unwrap();
    let mut header = [0; 8];
    journal.read_exact(&mut header).unwrap();
    header
}

#[test]
fn interrupted_provider_and_capture_transactions_recover_with_pinned_journals() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        // The only paths/keys supplied to this child belong to the parent's
        // disposable fixture. Exit bypasses Connection/Transaction destructors.
        let root = std::path::PathBuf::from(root);
        let cache = PrivateDirectory::open(&root.join("cache")).unwrap();
        let spool = PrivateDirectory::open(&root.join("spool")).unwrap();
        let _provider_pin = cache
            .database_file(Path::new("provider-bodies-v1.sqlite-journal"), false)
            .unwrap()
            .unwrap();
        let _capture_pin = spool
            .database_file(Path::new("capture-v1.sqlite-journal"), false)
            .unwrap()
            .unwrap();
        let provider =
            rusqlite::Connection::open(root.join("cache/provider-bodies-v1.sqlite")).unwrap();
        let capture = rusqlite::Connection::open(root.join("spool/capture-v1.sqlite")).unwrap();
        for connection in [&provider, &capture] {
            connection.execute_batch("PRAGMA journal_mode=PERSIST; PRAGMA synchronous=EXTRA; PRAGMA cache_size=2; PRAGMA cache_spill=ON; BEGIN IMMEDIATE;").unwrap();
        }
        provider
            .execute("UPDATE provider_bodies SET body=zeroblob(1048576)", [])
            .unwrap();
        let orphan = format!("{0}/{0}/{0}.md", "0".repeat(64));
        provider
            .execute(
                "INSERT INTO provider_bodies VALUES(?1,?2,zeroblob(1048576))",
                rusqlite::params![orphan, "0".repeat(64)],
            )
            .unwrap();
        capture
            .execute(
                "UPDATE capture_bodies SET projection=zeroblob(1048576),bytes=1048576",
                [],
            )
            .unwrap();
        // Prove each journal is genuinely hot before terminating the process.
        assert_eq!(
            journal_header(&root.join("cache"), "provider-bodies-v1.sqlite-journal"),
            HOT_HEADER
        );
        assert_eq!(
            journal_header(&root.join("spool"), "capture-v1.sqlite-journal"),
            HOT_HEADER
        );
        std::process::exit(CRASH);
    }
    let (temp, mut store, projection, binding) = setup();
    let (metadata, document) = body(
        "committed-key",
        "committed-revision",
        "# Original body\nExact provider bytes.\n",
    );
    let receipt = store.with_library_connection(|c| {
        let tx = c.transaction().unwrap();
        let receipt = projection
            .admit(&tx, &binding, &metadata, &document)
            .unwrap();
        tx.commit().unwrap();
        receipt
    });
    let project = temp.path().join("selected-project");
    std::fs::create_dir(&project).unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "selected".into(),
        project_root: project.clone(),
        repository_root: project.clone(),
        worktree_roots: vec![project],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    let spool = Spool::open(temp.path().join("spool"), Limits::default()).unwrap();
    let event = serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"synthetic","turn_id":"turn","cwd":policy.project_root,"last_assistant_message":"# Exact visible capture\nPreserved output."})).unwrap();
    let id = match spool
        .enqueue(Adapter::ClaudeCode, &event, &policy, CaptureOrigin::Live)
        .unwrap()
    {
        EnqueueOutcome::Queued { id } => id,
        other => panic!("{other:?}"),
    };
    let projected = spool.project(&id, &policy).unwrap();
    let exact_capture = std::fs::read(&projected.path).unwrap();
    spool.acknowledge(&id).unwrap();
    drop(spool);
    drop(projection);
    drop(store);
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "windows_recovery::interrupted_provider_and_capture_transactions_recover_with_pinned_journals", "--test-threads=1"])
        .env(CHILD_ROOT, temp.path()).status().unwrap();
    assert_eq!(status.code(), Some(CRASH));
    assert_eq!(
        journal_header(
            &temp.path().join("cache"),
            "provider-bodies-v1.sqlite-journal"
        ),
        HOT_HEADER
    );
    assert_eq!(
        journal_header(&temp.path().join("spool"), "capture-v1.sqlite-journal"),
        HOT_HEADER
    );

    let projection =
        ProviderProjection::open(&temp.path().join("cache"), binding.library_id().clone()).unwrap();
    let store = ReaderStore::open(&temp.path().join("data")).unwrap();
    // The retained writer must release EXCLUSIVE: this uses a sibling readonly
    // SQLite connection, validates the current exact receipt and hashes bytes.
    let read = store
        .read_document(
            &receipt.reader_source_id,
            &receipt.path,
            Some(&receipt.revision),
        )
        .unwrap();
    assert_eq!(read.markdown.as_bytes(), document.body.as_bytes());
    assert!(store
        .read_document(
            &receipt.reader_source_id,
            &receipt.path,
            Some(&"0".repeat(64))
        )
        .is_err());
    let cache = rusqlite::Connection::open_with_flags(
        temp.path().join("cache/provider-bodies-v1.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        cache
            .query_row("SELECT count(*) FROM provider_bodies", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(store.list_sources().unwrap().len(), 1);
    let spool = Spool::open(temp.path().join("spool"), Limits::default()).unwrap();
    assert!(spool.pending(8).unwrap().is_empty());
    std::fs::remove_file(&projected.path).unwrap();
    spool.repair_projections(&[policy], 2).unwrap();
    assert_eq!(std::fs::read(&projected.path).unwrap(), exact_capture);
    assert_ne!(
        journal_header(
            &temp.path().join("cache"),
            "provider-bodies-v1.sqlite-journal"
        ),
        HOT_HEADER
    );
    assert_ne!(
        journal_header(&temp.path().join("spool"), "capture-v1.sqlite-journal"),
        HOT_HEADER
    );
    drop(projection);
}

#[test]
fn retained_cache_rejects_changed_library_version_and_namespace_before_publication() {
    for mutation in [
        "UPDATE provider_meta SET version=2",
        "UPDATE provider_meta SET library_id='other-library'",
        "CREATE TABLE foreign_namespace(value TEXT)",
    ] {
        let (temp, mut store, projection, binding) = setup();
        let cache = rusqlite::Connection::open(temp.path().join("cache/provider-bodies-v1.sqlite"))
            .unwrap();
        cache
            .pragma_update(None, "journal_mode", "PERSIST")
            .unwrap();
        cache.execute_batch(mutation).unwrap();
        let (metadata, document) = body("new-key", "new-revision", "# Never admitted\n");
        assert!(store
            .with_library_connection(|c| {
                let tx = c.transaction().unwrap();
                projection.admit(&tx, &binding, &metadata, &document)
            })
            .is_err());
        assert!(store.list_sources().unwrap().is_empty());
        assert!(store.pending_index_jobs().unwrap().is_empty());
        assert_eq!(
            cache
                .query_row("SELECT count(*) FROM provider_bodies", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        drop(projection);
        assert!(
            ProviderProjection::open(&temp.path().join("cache"), binding.library_id().clone())
                .is_err()
        );
    }
}
