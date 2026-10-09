//! Combined-candidate regressions: real owner locks, SQLite, selected raster
//! materialization, restart journal and post-activation candidate writes.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::device::{self, assets::AssetRuntime};
use std::{io::Write, time::Instant};

const BODY: &str = "\u{feff}# Selected\r\n\r\n![pixel](pixel.png)\r\nRetained bytes.\r\n";
struct Fixture {
    _temporary: tempfile::TempDir,
    original: PathBuf,
    output: PathBuf,
    selected: super::super::Selection,
    png: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let parent = SourceRoot::capture(temporary.path().canonicalize().unwrap()).unwrap();
        let original = parent
            .create_or_capture_directory_relative(Path::new("original"))
            .unwrap();
        let output = parent
            .create_or_capture_directory_relative(Path::new("exports"))
            .unwrap()
            .path;
        original
            .create_new_relative(Path::new(".mdreader-json-owner.lock"))
            .unwrap();
        let mut store = ReaderStore::open(&original.path.join("reader-v1")).unwrap();
        let selected = device::import(
            &mut store,
            &original.path.join("reader-v1/device-markdown-v1"),
            "selected.md".into(),
            BODY.into(),
        )
        .unwrap();
        device::pin(&mut store, &selected.id, true).unwrap();
        device::position(&mut store, &selected.id, &selected.revision, 3).unwrap();
        device::import(
            &mut store,
            &original.path.join("reader-v1/device-markdown-v1"),
            "archive-only.md".into(),
            "# Not selected\nKept in the original archive.\n".into(),
        )
        .unwrap();
        store
            .with_library_connection(|db| {
                db.execute_batch(
            "CREATE TABLE unknown_owned_record(id INTEGER PRIMARY KEY, body BLOB NOT NULL); \
             INSERT INTO unknown_owned_record VALUES(1,X'00ff0a');"
        )
            })
            .unwrap();
        let runtime = AssetRuntime::default();
        let assets = runtime
            .assets(
                &mut store,
                &original.path.join("reader-v1/device-assets-v1"),
            )
            .unwrap();
        // Same public synthetic 2x3 PNG used by the native asset fixture.
        let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAD0lEQVR4nGNgYPgPRsgUADjcBftBwbf8AAAAAElFTkSuQmCC").unwrap();
        let manifest = assets
            .manifest(&mut store, &selected.id, &selected.revision)
            .unwrap();
        let id = assets
            .begin(
                &mut store,
                &selected.id,
                &selected.revision,
                "pixel.png",
                png.len() as u32,
                &manifest.manifest,
            )
            .unwrap();
        assets
            .write(&mut store, &id, 0, &STANDARD.encode(&png))
            .unwrap();
        let validated = assets
            .prepare_commit(&mut store, &id, runtime.validation_permit().unwrap())
            .unwrap()
            .validate()
            .unwrap();
        assets.publish(&mut store, validated).unwrap();
        drop(store);
        Self {
            _temporary: temporary,
            original: original.path,
            output,
            selected: super::super::Selection {
                document_id: selected.id,
                revision: selected.revision,
            },
            png,
        }
    }
    fn prepare(&self) -> Workspace {
        Workspace::prepare(
            ClosedOwner::acquire(&self.original).unwrap(),
            &self.output,
            std::slice::from_ref(&self.selected),
        )
        .unwrap()
    }
}
fn documents(path: &Path) -> Vec<device::DeviceDocument> {
    let store = ReaderStore::open_durable_readonly(&path.join("reader-v1")).unwrap();
    super::super::conversion::documents(&store).unwrap()
}
fn backup_database(path: &Path, snapshot: &VerifiedSnapshot) -> PathBuf {
    let entry = snapshot
        .manifest()
        .entries
        .iter()
        .find(|entry| entry.relative.restore().unwrap() == Path::new(DATABASE))
        .unwrap();
    path.join(entry.member.as_ref().unwrap())
}

#[test]
fn selected_offline_profile_roundtrip_preserves_assets_unknown_records_and_candidate_writes() {
    let fixture = Fixture::new();
    let picked =
        super::super::commands::picked_root(fixture._temporary.path().join("original")).unwrap();
    assert_eq!(
        picked.path, fixture.original,
        "OS aliases preserve the admitted directory"
    );
    let alias = fixture._temporary.path().join("leaf-link");
    std::os::unix::fs::symlink(&fixture.original, &alias).unwrap();
    assert!(
        super::super::commands::picked_root(alias).is_err(),
        "leaf symlinks remain forbidden"
    );
    std::fs::remove_file(fixture.original.join("reader-v1/index.sqlite")).unwrap();
    let root = SourceRoot::capture(fixture.original.clone()).unwrap();
    let live_gui = root
        .open_lock_relative(Path::new(".mdreader-json-owner.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&live_gui).unwrap();
    assert!(matches!(
        ClosedOwner::acquire(&fixture.original),
        Err(Error::NotQuiescent)
    ));
    fs2::FileExt::unlock(&live_gui).unwrap();
    drop(live_gui);

    let mut prepared = fixture.prepare();
    let directory = prepared.launch_directory();
    let candidate = directory.join("candidate");
    let operation = prepared.operation;
    let receipt = Journal::attach(&mut prepared.journal)
        .unwrap()
        .get(operation)
        .unwrap()
        .candidate
        .unwrap();
    let snapshot = mdreader_profile::verify(
        &directory.join("original"),
        &receipt.snapshot,
        &Limits::default(),
    )
    .unwrap();
    let backup = Connection::open_with_flags(
        backup_database(&directory.join("original"), &snapshot),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        backup
            .query_row(
                "SELECT body FROM unknown_owned_record WHERE id=1",
                [],
                |row| row.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
        [0, 255, 10]
    );
    assert_eq!(
        backup
            .query_row("SELECT count(*) FROM device_documents_v1", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        2
    );
    drop(backup);
    assert_eq!(prepared.imported().unwrap(), 1);
    assert!(
        !fixture.original.join("reader-v1/index.sqlite").exists(),
        "import cannot create a disposable source index"
    );
    assert!(
        verify_prepared(&candidate).is_err(),
        "staged is not activated"
    );
    drop(prepared);
    let mut resumed = Workspace::resume(&directory).unwrap();
    assert_eq!(resumed.operation, operation);
    resumed.activate().unwrap();
    drop(resumed);
    assert_eq!(selected_for_launch(&directory).unwrap().path, candidate);
    let imported = documents(&candidate);
    assert_eq!(imported.len(), 1);
    assert_ne!(imported[0].id, fixture.selected.document_id);
    assert!(imported[0].pinned);
    assert_eq!(imported[0].line, 3);
    assert_eq!(imported[0].revision, fixture.selected.revision);
    let mut store = ReaderStore::open(&candidate.join("reader-v1")).unwrap();
    let read = device::catalog::read(&store, &device::catalog::receipt(&imported[0])).unwrap();
    assert_eq!(read.markdown.as_bytes(), BODY.as_bytes());
    let runtime = AssetRuntime::default();
    let assets = runtime
        .existing_assets(&mut store, &candidate.join("reader-v1/device-assets-v1"))
        .unwrap()
        .unwrap();
    let manifest = assets
        .manifest(&mut store, &imported[0].id, &imported[0].revision)
        .unwrap();
    assert_eq!(manifest.assets.len(), 1);
    let raster = assets
        .prepare_open(
            &mut store,
            &imported[0].id,
            &imported[0].revision,
            &manifest.assets[0].asset_id,
            &manifest.manifest,
            runtime.read_permit(Instant::now()).unwrap(),
        )
        .unwrap()
        .verify()
        .unwrap();
    let (session, _) = runtime
        .install(&assets, &mut store, raster, Instant::now())
        .unwrap();
    let bytes = runtime
        .chunk(&assets, &mut store, &session, 0, Instant::now())
        .unwrap();
    assert_eq!(STANDARD.decode(bytes.base64).unwrap(), fixture.png);
    runtime.close(&session).unwrap();
    let after = device::import(
        &mut store,
        &candidate.join("reader-v1/device-markdown-v1"),
        "after.md".into(),
        "# Written after activation\n".into(),
    )
    .unwrap();
    device::pin(&mut store, &after.id, true).unwrap();
    drop(store);

    let mut rollback = Workspace::resume(&directory).unwrap();
    rollback.rollback().unwrap();
    drop(rollback);
    assert_eq!(
        selected_for_launch(&directory).unwrap().path,
        fixture.original
    );
    assert!(
        verify_prepared(&candidate).is_err(),
        "rolled-back selector cannot open stale candidate as active"
    );
    assert_eq!(documents(&fixture.original).len(), 2);
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("rollback.json")).unwrap()).unwrap();
    let identity = serde_json::from_value(record["snapshot"].clone()).unwrap();
    let archive = directory.join(format!(
        "rollback-{}",
        record["directory"].as_str().unwrap()
    ));
    let snapshot = mdreader_profile::verify(&archive, &identity, &Limits::default()).unwrap();
    let backup = Connection::open_with_flags(
        backup_database(&archive, &snapshot),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        backup
            .query_row(
                "SELECT pinned FROM device_documents_v1 WHERE id=?1",
                [&after.id],
                |row| row.get::<_, bool>(0)
            )
            .unwrap(),
        true
    );
    assert_eq!(
        std::fs::read(
            fixture
                .original
                .join("reader-v1/device-markdown-v1")
                .join(&fixture.selected.document_id)
                .join("document.md")
        )
        .unwrap(),
        BODY.as_bytes()
    );
}

#[test]
fn resumed_preparation_rejects_original_changes_and_stale_document_revision() {
    let fixture = Fixture::new();
    let index = fixture.original.join("reader-v1/index.sqlite");
    let old_index = Connection::open(&index).unwrap();
    old_index.execute_batch("DROP INDEX documents_parser_version; ALTER TABLE documents DROP COLUMN parser_version; PRAGMA user_version=5;").unwrap();
    drop(old_index);
    let original_index = std::fs::read(&index).unwrap();
    let mut wrong = fixture.selected.clone();
    wrong.revision = "0".repeat(64);
    assert!(matches!(
        Workspace::prepare(
            ClosedOwner::acquire(&fixture.original).unwrap(),
            &fixture.output,
            &[wrong]
        ),
        Err(Error::Conflict)
    ));
    let prepared = fixture.prepare();
    assert_eq!(
        std::fs::read(&index).unwrap(),
        original_index,
        "conversion cannot migrate a v5 source index"
    );
    let directory = prepared.launch_directory();
    drop(prepared);
    let mut source = ReaderStore::open(&fixture.original.join("reader-v1")).unwrap();
    device::pin(&mut source, &fixture.selected.document_id, false).unwrap();
    drop(source);
    let mut resumed = Workspace::resume(&directory).unwrap();
    assert!(matches!(resumed.activate(), Err(Error::Conflict)));
    assert_eq!(
        Journal::attach(&mut resumed.journal)
            .unwrap()
            .selection()
            .unwrap()
            .generation,
        0
    );
    assert!(
        !documents(&fixture.original)
            .iter()
            .find(|doc| doc.id == fixture.selected.document_id)
            .unwrap()
            .pinned
    );
}

#[test]
fn uncertain_rollback_archive_cannot_discard_later_candidate_changes() {
    let fixture = Fixture::new();
    let mut prepared = fixture.prepare();
    let directory = prepared.launch_directory();
    prepared.activate().unwrap();
    let candidate = directory.join("candidate");
    // Simulate restart after archive/intent persistence but before selector CAS.
    // The actual closed owner creates the real snapshot; no fake freeze boolean.
    let generation = Journal::attach(&mut prepared.journal)
        .unwrap()
        .selection()
        .unwrap()
        .generation;
    let id = Uuid::new_v4();
    let archive = directory.join(format!("rollback-{id}"));
    let choices =
        super::super::super::backup_inventory::selections(&candidate, &Limits::default()).unwrap();
    let inventory = Inventory::capture(&candidate, &choices, &Limits::default()).unwrap();
    let snapshot = mdreader_profile::stage(
        &mut super::super::super::BorrowedFreeze {
            frozen: &mut prepared.candidate,
        },
        inventory,
        &archive,
        &Limits::default(),
    )
    .unwrap();
    let record = serde_json::json!({"version":1,"operation":prepared.operation,"generation":generation,"directory":id,"snapshot":snapshot.identity()});
    let mut file = prepared
        .root
        .create_new_relative(Path::new("rollback.json"))
        .unwrap();
    file.write_all(&serde_json::to_vec(&record).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    drop(file);
    drop(prepared);
    let mut candidate_owner = ReaderStore::open(&candidate.join("reader-v1")).unwrap();
    let changed = device::import(
        &mut candidate_owner,
        &candidate.join("reader-v1/device-markdown-v1"),
        "must-keep.md".into(),
        "# Later durable write\n".into(),
    )
    .unwrap();
    drop(candidate_owner);
    let mut resumed = Workspace::resume(&directory).unwrap();
    assert!(matches!(resumed.rollback(), Err(Error::Conflict)));
    assert!(
        Journal::attach(&mut resumed.journal)
            .unwrap()
            .selection()
            .unwrap()
            .root
            == *resumed.candidate.binding()
    );
    assert!(documents(&candidate)
        .iter()
        .any(|document| document.id == changed.id));
}

#[test]
fn service_handoff_retains_lost_begin_identity_and_never_shutdowns_quarantined_owner() {
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        time::Duration,
    };
    let temporary = tempfile::tempdir().unwrap();
    let root = SourceRoot::capture(temporary.path().canonicalize().unwrap()).unwrap();
    root.create_or_capture_directory_relative(Path::new("reader-v1"))
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let boot = Uuid::new_v4();
    let endpoint = serde_json::json!({"protocol":1,"port":port,"token":"a".repeat(64),"pid":1});
    root.create_new_relative(Path::new("reader-v1/reader-endpoint.json"))
        .unwrap()
        .write_all(&serde_json::to_vec(&endpoint).unwrap())
        .unwrap();
    let retained = root
        .open_lock_relative(Path::new("reader-v1/reader.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&retained).unwrap();
    let contender = root
        .open_lock_relative(Path::new("reader-v1/reader.lock"))
        .unwrap();
    let server = std::thread::spawn(move || {
        let mut first_scope = None;
        for index in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut input = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut input)
                .unwrap();
            let envelope: serde_json::Value = serde_json::from_str(&input).unwrap();
            let request = &envelope["request"]["params"]["request"];
            assert_eq!(envelope["request"]["method"], "preservation");
            let result = if index == 0 {
                assert_eq!(request["operation"], "inspect");
                serde_json::json!({"state":"hello","boot":boot})
            } else {
                assert_eq!(request["operation"], "begin");
                if index == 1 {
                    first_scope = Some(request["scope"].clone());
                    // The first dispatched Begin loses its reply. It must not
                    // be replaced by a fresh operation on the next UI Retry.
                    continue;
                }
                assert_eq!(Some(&request["scope"]), first_scope.as_ref());
                serde_json::json!({"state":"status","status":{
                    "phase":"quarantined","remaining_ms":0,"lease":null,"failure":"deadline"}})
            };
            writeln!(
                stream,
                "{}",
                serde_json::json!({"result":result,"error":null})
            )
            .unwrap();
        }
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok(_) => panic!("failed/quarantined Begin must not issue Revalidate or Shutdown"),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("unexpected synthetic listener error: {error}"),
            }
        }
    });
    let mut handoff = super::super::shutdown_handoff::ShutdownHandoff::default();
    for _ in 0..2 {
        assert!(matches!(
            handoff.close(&root, &contender),
            Err(Error::ReconciliationRequired)
        ));
        assert_eq!(handoff.failure_kind(), Some("service_handoff_unconfirmed"));
        assert!(fs2::FileExt::try_lock_exclusive(&contender).is_err());
    }
    server.join().unwrap();
    fs2::FileExt::unlock(&retained).unwrap();
}
