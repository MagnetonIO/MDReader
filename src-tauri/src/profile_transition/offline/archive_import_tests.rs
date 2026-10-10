//! Combined migration boundary: real old SQLite, archive, native materialization,
//! ordinary retrieval, relationship navigation and durable explicit matching.
use super::*;
use legacy::{Input, Role};
use rusqlite::{params, Connection};
use std::os::unix::fs::PermissionsExt;
use uuid::Uuid;

struct Fixture {
    _temporary: tempfile::TempDir,
    archive: PathBuf,
    output: PathBuf,
    originals: Vec<(PathBuf, Vec<u8>)>,
    target: String,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let database = root.join("legacy.db");
        let db = Connection::open(&database).unwrap();
        db.execute_batch("CREATE TABLE objects(object_id TEXT PRIMARY KEY,entity TEXT,value TEXT,provenance TEXT);
            CREATE TABLE morphisms(morphism_id TEXT PRIMARY KEY,arrow TEXT,source TEXT,target TEXT,provenance TEXT);
            CREATE TABLE unknown_owned_record(body BLOB); INSERT INTO unknown_owned_record VALUES(x'00ff');
            PRAGMA user_version=10;").unwrap();
        let claim = Uuid::new_v4().to_string();
        let target = Uuid::new_v4().to_string();
        db.execute(
            "INSERT INTO objects VALUES(?1,'kn.Claim',?2,'{}')",
            params![
                claim,
                r#"{"claim_text":"Migration continuity","customAuthoredField":"retained"}"#
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO objects VALUES(?1,'kn.Concept',?2,'{}')",
            params![target, r#"{"concept_name":"Readable recovery"}"#],
        )
        .unwrap();
        db.execute(
            "INSERT INTO morphisms VALUES(?1,'kn.about',?2,?3,'{}')",
            params![Uuid::new_v4().to_string(), claim, target],
        )
        .unwrap();
        drop(db);
        let session = root.join("session.json");
        std::fs::write(
            &session,
            serde_json::to_vec(&serde_json::json!({"session": {
                "version":1,"root":"/synthetic/old", "activeTabId":"old-tab",
                "tabs":[{"id":"old-tab","docRef":{"path":"old.md","name":"Old note"},
                    "history":[{"path":"old.md","name":"Old note"}],"historyIndex":0,
                    "scroll":{"block":5,"offset":17},"pinned":true}],
                "recent":[{"path":"old.md","name":"Old note","root":"/synthetic/old"}],
                "readerHistory":{"version":1,"activeTabId":"canonical-tab","mru":["canonical-tab"],
                    "tabs":[{"id":"canonical-tab","index":0,"pinned":true,"entries":[{
                        "title":"Canonical owned note","heading":"Saved section",
                        "reference":{"kind":"device","documentId":"old-device-id","sourceId":"old-source","revision":"old-exact-revision"},
                        "position":{"revision":"old-exact-revision","line":12,"block":4,"offset":9}}]}],
                    "recent":[{"title":"Canonical provider note","heading":null,"position":null,
                        "reference":{"kind":"provider","sourceId":"old-provider","path":"note.md","revision":"provider-revision"}}]}
            }}))
            .unwrap(),
        )
        .unwrap();
        let settings = root.join("settings.json");
        std::fs::write(&settings, br#"{"settings":{"theme":"dark","readingFont":"sans","token":"synthetic-not-a-real-token","lensSchema":"retired"}}"#).unwrap();
        let inputs = vec![
            Input {
                role: Role::Catalog,
                path: database,
            },
            Input {
                role: Role::Session,
                path: session,
            },
            Input {
                role: Role::Settings,
                path: settings,
            },
        ];
        for input in &inputs {
            std::fs::set_permissions(&input.path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let originals = inputs
            .iter()
            .map(|i| (i.path.clone(), std::fs::read(&i.path).unwrap()))
            .collect();
        let archive = root.join("archive");
        legacy::export(&inputs, &archive, &Default::default()).unwrap();
        let records = std::fs::read_to_string(archive.join("0000.records.jsonl")).unwrap();
        assert!(
            records.lines().any(|line| matches!(
                legacy::catalog::decode_record(line.as_bytes(), &Default::default()).unwrap(),
                Some(legacy::catalog::ArchivedRecord::Object { .. })
            )),
            "Synthetic graph was not decoded: {records}"
        );
        let output = root.join("output");
        std::fs::create_dir(&output).unwrap();
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _temporary: temporary,
            archive,
            output,
            originals,
            target,
        }
    }
}

#[test]
fn verified_legacy_archive_becomes_searchable_linked_copies_and_reopenable_history() {
    let fixture = Fixture::new();
    let mut import = ArchiveImport::preview(&fixture.archive).unwrap();
    let coverage = import.coverage().unwrap();
    assert_eq!(coverage.documents, 2);
    assert_eq!(coverage.relationships, 1);
    assert_eq!(coverage.history_entries, 6);
    let selected = import
        .documents()
        .into_iter()
        .map(|d| Selection {
            document_id: d.document_id,
            revision: d.revision,
        })
        .collect::<Vec<_>>();
    assert_eq!(import.prepare(&fixture.output, &selected).unwrap(), 2);
    let candidate = &import.candidate.as_ref().unwrap().path;
    let mut store = ReaderStore::open(&candidate.join("reader-v1")).unwrap();
    let documents = device::list(&mut store).unwrap();
    assert_eq!(documents.len(), 2);
    assert!(!device::search(&mut store, "continuity").unwrap().is_empty());
    let claim = documents
        .iter()
        .find(|d| d.name.contains("Migration continuity"))
        .unwrap();
    let receipt = device::catalog::receipt(claim);
    let body = device::catalog::read_in(
        &store,
        &candidate.join("reader-v1/device-markdown-v1"),
        &receipt,
    )
    .unwrap();
    assert!(body.markdown.contains("customAuthoredField"));
    let targets = device::archive::resolve(
        &store,
        &candidate.join("reader-v1/device-markdown-v1"),
        &receipt,
        "relative",
        &format!("legacy/0/{}.md", fixture.target),
    )
    .unwrap();
    assert_eq!(targets.len(), 1);
    assert!(targets[0].name.contains("Readable recovery"));
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(candidate.join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings["settings"]["theme"], "dark");
    assert!(settings["settings"].get("token").is_none());
    assert!(settings["settings"].get("lensSchema").is_none());
    let history = legacy::history::entries(
        &candidate.join("legacy-archive"),
        &import.identity,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(history.len(), 6);
    assert_eq!(history[3].context, "tab");
    assert_eq!(history[3].title, "Canonical owned note");
    assert!(history[3].archived_position);
    assert_eq!(history[3].raw["pinned"], true);
    assert_eq!(history[3].raw["index"], 0);
    assert_eq!(history[4].context, "tab_history");
    assert_eq!(history[4].raw["reference"]["documentId"], "old-device-id");
    assert_eq!(
        history[4].raw["reference"]["revision"],
        "old-exact-revision"
    );
    assert_eq!(history[4].raw["position"]["line"], 12);
    assert_eq!(history[4].raw["position"]["offset"], 9);
    assert_eq!(history[5].context, "recent");
    assert_eq!(history[5].kind, "provider");
    assert!(!history[5].archived_position);
    assert_eq!(
        history
            .iter()
            .map(|e| &e.entry_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        6
    );
    assert!(history[0].archived_position);
    assert_eq!(history[0].raw["scroll"]["offset"], 17);
    assert_eq!(history[0].raw["pinned"], true);
    let archive_id = import.identity.export_id.to_string();
    let sha = &import.identity.manifest_sha256;
    let empty = device::archive::history::bindings(&store, &archive_id, sha).unwrap();
    let expected = device::archive::history::digest(&empty).unwrap();
    let reference = device::archive::history::Reference {
        document_id: Some(claim.id.clone()),
        source_id: claim.source_id.clone(),
        path: "document.md".into(),
        revision: claim.revision.clone(),
    };
    device::archive::history::bind(
        &mut store,
        &archive_id,
        sha,
        &history[0].entry_id,
        &expected,
        &reference,
    )
    .unwrap();
    assert!(device::archive::history::bind(
        &mut store,
        &archive_id,
        sha,
        &history[1].entry_id,
        &expected,
        &reference
    )
    .is_err());
    drop(store);
    let reopened = ReaderStore::open(&candidate.join("reader-v1")).unwrap();
    let bindings = device::archive::history::bindings(&reopened, &archive_id, sha).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].reference, reference);
    let reopened_history = legacy::history::entries(
        &candidate.join("legacy-archive"),
        &import.identity,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(reopened_history[4].entry_id, history[4].entry_id);
    assert_eq!(reopened_history[4].raw, history[4].raw);
    assert_eq!(reopened_history[5].raw, history[5].raw);
    assert_eq!(
        device::archive::resolve(
            &reopened,
            &candidate.join("reader-v1/device-markdown-v1"),
            &receipt,
            "relative",
            &format!("legacy/0/{}.md", fixture.target)
        )
        .unwrap()
        .len(),
        1
    );
    legacy::migration::verify(candidate, &Default::default()).unwrap();
    for (index, (path, bytes)) in fixture.originals.iter().enumerate() {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
        assert_eq!(
            &std::fs::read(candidate.join(format!("legacy-archive/{index:04}.raw"))).unwrap(),
            bytes
        );
    }
}

#[test]
fn stale_preview_never_publishes_a_different_revision_or_prepared_candidate() {
    let fixture = Fixture::new();
    let mut import = ArchiveImport::preview(&fixture.archive).unwrap();
    let selected = vec![Selection {
        document_id: import.documents()[0].document_id.clone(),
        revision: "0".repeat(64),
    }];
    assert!(import.prepare(&fixture.output, &selected).is_err());
    assert!(std::fs::read_dir(&fixture.output).unwrap().next().is_none());
    assert!(import.candidate.is_none());
}

#[test]
fn mixed_closed_profile_keeps_true_reader_bytes_assets_pin_and_position_with_legacy_insights() {
    use crate::profile_transition::offline::{legacy_snapshot, owner::ClosedOwner};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use mdreader_core::device::assets::AssetRuntime;
    use mdreader_profile::FrozenProfile;
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let parent = fixture.output.parent().unwrap().canonicalize().unwrap();
    let source = parent.join("mixed-original");
    std::fs::create_dir(&source).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (index, (path, _)) in fixture.originals.iter().enumerate() {
        let name = ["catdb.db", "session.json", "settings.json"][index];
        std::fs::copy(path, source.join(name)).unwrap();
    }
    let root = SourceRoot::capture(source.clone()).unwrap();
    root.create_new_relative(Path::new(".mdreader-json-owner.lock"))
        .unwrap();
    let original = "\u{feff}# Real reader body\r\n![pixel](pixel.png)\r\nSaved position here.\r\n";
    let mut store = ReaderStore::open(&source.join("reader-v1")).unwrap();
    let owned = device::import(
        &mut store,
        &source.join("reader-v1/device-markdown-v1"),
        "owned.md".into(),
        original.into(),
    )
    .unwrap();
    device::pin(&mut store, &owned.id, true).unwrap();
    device::position(&mut store, &owned.id, &owned.revision, 3).unwrap();
    let runtime = AssetRuntime::default();
    let assets = runtime
        .assets(&mut store, &source.join("reader-v1/device-assets-v1"))
        .unwrap();
    let png=STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAD0lEQVR4nGNgYPgPRsgUADjcBftBwbf8AAAAAElFTkSuQmCC").unwrap();
    let manifest = assets
        .manifest(&mut store, &owned.id, &owned.revision)
        .unwrap();
    let asset = assets
        .begin(
            &mut store,
            &owned.id,
            &owned.revision,
            "pixel.png",
            png.len() as u32,
            &manifest.manifest,
        )
        .unwrap();
    assets
        .write(&mut store, &asset, 0, &STANDARD.encode(&png))
        .unwrap();
    let validated = assets
        .prepare_commit(&mut store, &asset, runtime.validation_permit().unwrap())
        .unwrap()
        .validate()
        .unwrap();
    assets.publish(&mut store, validated).unwrap();
    let recovered = recovered_fixture(&mut store, &source, &parent);
    drop(store);
    let mut closed = ClosedOwner::acquire(&source).unwrap();
    let archive = legacy_snapshot::capture(&root, || closed.revalidate()).unwrap();
    legacy_snapshot::verify_originals(&root, &archive).unwrap();
    let mut import = ArchiveImport::preview(&archive).unwrap();
    assert_eq!(import.documents().len(), 9);
    let selected = import
        .documents()
        .into_iter()
        .map(|d| Selection {
            document_id: d.document_id,
            revision: d.revision,
        })
        .collect::<Vec<_>>();
    assert_eq!(import.prepare(&fixture.output, &selected).unwrap(), 9);
    let candidate = import.candidate.as_ref().unwrap().path.clone();
    legacy_snapshot::verify_originals(&root, &archive).unwrap();
    let mut reopened = ReaderStore::open(&candidate.join("reader-v1")).unwrap();
    let copies = device::list(&mut reopened).unwrap();
    let copy = copies.iter().find(|d| d.name == "owned.md").unwrap();
    assert_ne!(copy.id, owned.id);
    assert_eq!(copy.revision, owned.revision);
    assert!(copy.pinned);
    assert_eq!(copy.line, 3);
    assert_eq!(
        device::catalog::read_in(
            &reopened,
            &candidate.join("reader-v1/device-markdown-v1"),
            &device::catalog::receipt(copy)
        )
        .unwrap()
        .markdown
        .as_bytes(),
        original.as_bytes()
    );
    let copied_runtime = AssetRuntime::default();
    let copied_assets = copied_runtime
        .existing_assets(&mut reopened, &candidate.join("reader-v1/device-assets-v1"))
        .unwrap()
        .unwrap();
    let payload = device::publication::prepare_archive(
        &mut reopened,
        &copy.id,
        &copied_assets,
        &copied_runtime,
    )
    .unwrap()
    .read()
    .unwrap();
    assert_eq!(payload.markdown(), original.as_bytes());
    assert_eq!(payload.assets().len(), 1);
    assert_eq!(payload.assets()[0].target, "pixel.png");
    assert_eq!(payload.assets()[0].bytes().unwrap(), png);
    assert!(!device::search(&mut reopened, "continuity")
        .unwrap()
        .is_empty());
    assert!(!device::search(&mut reopened, "Real reader body")
        .unwrap()
        .is_empty());
    for (name, body, line) in recovered {
        let copy = copies.iter().find(|d| d.name == name).unwrap();
        let read = device::catalog::read(&reopened, &device::catalog::receipt(copy)).unwrap();
        assert_eq!(read.markdown, body);
        assert_eq!(copy.line, line);
        if name == "Recovered inactive.md" {
            assert!(copy.pinned);
        }
    }
    let identity = legacy::migration::archive::identity(&archive, &Default::default()).unwrap();
    let bindings = device::archive::history::bindings(
        &reopened,
        &identity.export_id.to_string(),
        &identity.manifest_sha256,
    )
    .unwrap();
    assert_eq!(bindings.len(), 3);
    assert_eq!(reopened.capture_count().unwrap(), 0);
    reopened.with_library_connection(|db| {
        let mutations:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='mdreader_provider_mutations')",[],|r|r.get(0)).unwrap();assert!(!mutations);
    });
    assert!(reopened
        .list_sources()
        .unwrap()
        .iter()
        .all(|s| !s.id.starts_with("provider:")));
    legacy_snapshot::verify_originals(&root, &archive).unwrap();
}

// One combined mixed-profile fixture uses actual capture projection, provider
// admission/journal transitions and current owned readers; no mocked authority.
fn recovered_fixture(
    store: &mut ReaderStore,
    profile: &Path,
    parent: &Path,
) -> Vec<(String, String, u32)> {
    use mdreader_capture::{Adapter, CaptureOrigin, EnqueueOutcome, ProjectPolicy, Spool};
    use mdreader_core::ProviderProjection;
    use mdreader_provider::{
        journal, AccountBinding, DisplayText, DocumentBody, DocumentVersion, LibraryId,
        MarkdownBytes, Metadata, OpaqueValue, RecordKey, SourceBinding, SourceId,
    };
    let inactive = device::import(
        store,
        &profile.join("reader-v1/device-markdown-v1"),
        "inactive.md".into(),
        "# Inactive retained original\nSaved.\n".into(),
    )
    .unwrap();
    device::pin(store, &inactive.id, true).unwrap();
    device::position(store, &inactive.id, &inactive.revision, 2).unwrap();
    store
        .set_source_enabled(&inactive.source_id, false)
        .unwrap();
    let project = parent.join("capture-project");
    std::fs::create_dir(&project).unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "migration-capture".into(),
        project_root: project.clone(),
        repository_root: project.clone(),
        worktree_roots: vec![project.clone()],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    let spool = Spool::open(profile.join("reader-v1/captures"), Default::default()).unwrap();
    let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"migration-session","turn_id":"turn-1","prompt_id":"turn-1","cwd":project,"last_assistant_message":"# Captured retained output\nExact visible body.\n"})).unwrap();
    let id = match spool
        .enqueue(Adapter::ClaudeCode, &event, &policy, CaptureOrigin::Live)
        .unwrap()
    {
        EnqueueOutcome::Queued { id } => id,
        other => panic!("unexpected {other:?}"),
    };
    let projected = spool.project(&id, &policy).unwrap();
    let source = store
        .add_local_source(
            "Agent output",
            SourceRoot::capture(spool.markdown_root()).unwrap(),
        )
        .unwrap();
    let path = projected
        .path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    store
        .commit_capture(
            &id,
            &source.id,
            &path,
            &serde_json::to_string(&projected.record.provenance).unwrap(),
        )
        .unwrap();
    spool.acknowledge(&id).unwrap();
    let capture = store.read_document(&source.id, &path, None).unwrap();
    let capture_line = capture.markdown.lines().count() as u32;
    drop(spool);
    let library = LibraryId::new("migration-provider-library").unwrap();
    let binding = SourceBinding::new(
        library.clone(),
        SourceId::new("migration-provider").unwrap(),
        AccountBinding::new("synthetic-account").unwrap(),
        Uuid::new_v4(),
        "1".repeat(64),
    )
    .unwrap();
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("provider-version-1").unwrap(),
        content_hash: None,
        conditional_write_token: None,
    };
    let key = RecordKey::new("opaque-selected-record").unwrap();
    let body = DocumentBody {
        key: key.clone(),
        version: version.clone(),
        body: MarkdownBytes::new(b"# Cached retained body\nSaved exact position.\n".to_vec())
            .unwrap(),
        observed_at_ms: 1,
    };
    let metadata = Metadata {
        membership_ancestors: None,
        key: key.clone(),
        name: DisplayText::new("Provider note").unwrap(),
        display_path: DisplayText::new("Selected provider note").unwrap(),
        media_type: None,
        modified_at_ms: Some(1),
        size_bytes: Some(body.body.as_bytes().len() as u64),
        version: version.clone(),
    };
    let projection =
        ProviderProjection::open(&profile.join("reader-v1/provider-cache"), library).unwrap();
    let receipt = store.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        let receipt = projection.admit(&tx, &binding, &metadata, &body).unwrap();
        tx.commit().unwrap();
        receipt
    });
    store.with_library_connection(|db| {
        journal::prepare(
            db,
            &binding,
            &key,
            &version,
            &MarkdownBytes::new(b"# Prepared preserved draft\nNever send this.\n".to_vec())
                .unwrap(),
        )
        .unwrap();
        let op = journal::prepare(
            db,
            &binding,
            &key,
            &version,
            &MarkdownBytes::new(b"# Intended conflicting draft\nKeep this too.\n".to_vec())
                .unwrap(),
        )
        .unwrap();
        drop(journal::begin_dispatch(db, &binding, op).unwrap());
        let remote = DocumentBody {
            key: key.clone(),
            version: version.clone(),
            body: MarkdownBytes::new(
                b"# Conflicting retained remote\nBoth versions preserved.\n".to_vec(),
            )
            .unwrap(),
            observed_at_ms: 2,
        };
        assert!(matches!(
            journal::reconcile_read(db, &binding, op, &remote),
            Err(mdreader_provider::ProviderError::Conflict)
        ));
    });
    let session_path = profile.join("session.json");
    let mut session: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&session_path).unwrap()).unwrap();
    let session = session.get_mut("session").unwrap();
    let recent = session["readerHistory"]["recent"].as_array_mut().unwrap();
    for (kind, source_id, path, revision, line, document_id) in [
        (
            "capture",
            source.id.clone(),
            path,
            capture.revision.clone(),
            capture_line,
            None,
        ),
        (
            "provider",
            receipt.reader_source_id.clone(),
            receipt.path.clone(),
            receipt.revision.clone(),
            2,
            None,
        ),
        (
            "device",
            inactive.source_id.clone(),
            "document.md".into(),
            inactive.revision.clone(),
            2,
            Some(inactive.id.clone()),
        ),
    ] {
        // Capture canonical references use local kind with verified sourceKind at
        // read time; source/revision, not display kind, fences the association.
        let mut reference = serde_json::json!({"kind":if kind=="capture" {"local"} else {kind},"sourceId":source_id,"path":path,"revision":revision});
        if let Some(id) = document_id {
            reference["documentId"] = id.into();
        }
        recent.push(serde_json::json!({"title":"Recovered exact history","heading":null,"reference":reference,"position":{"revision":revision,"line":line,"block":null,"offset":null}}));
    }
    // Write the real wrapped session while its synthetic owner is still active.
    let wrapped = serde_json::json!({"session":session});
    std::fs::write(&session_path, serde_json::to_vec(&wrapped).unwrap()).unwrap();
    vec![
        (
            "Recovered inactive.md".into(),
            "# Inactive retained original\nSaved.\n".into(),
            2,
        ),
        ("Agent output.md".into(), capture.markdown, capture_line),
        (
            "Cached Provider note.md".into(),
            String::from_utf8(body.body.as_bytes().to_vec()).unwrap(),
            2,
        ),
        (
            "Recovered provider draft (prepared).md".into(),
            "# Prepared preserved draft\nNever send this.\n".into(),
            1,
        ),
        (
            "Recovered provider draft (conflict).md".into(),
            "# Intended conflicting draft\nKeep this too.\n".into(),
            1,
        ),
        (
            "Recovered conflicting provider copy.md".into(),
            "# Conflicting retained remote\nBoth versions preserved.\n".into(),
            1,
        ),
    ]
}
