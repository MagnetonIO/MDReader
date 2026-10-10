//! Real Windows process/byte-lock/raw-archive regression, isolated from the
//! caller's APPDATA. This is a synthetic OS participant, not GUI acceptance.
use super::super::*;
use crate::profile_transition::offline::windows_workspace::legacy as native_legacy;
use mdreader_profile::legacy_windows::{ClosedOriginal, LegacyApplicationFence};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, Instant},
};
const DRIVER: &str = "MDREADER_WINDOWS_LEGACY_DRIVER";
const TEST: &str = "profile_transition::offline::windows_workspace::tests::completed_legacy_candidate_recovers_and_live_retry_keeps_exact_selection";
type Objects = BTreeMap<PathBuf, (mdreader_capture::WindowsObjectStamp, Vec<u8>)>;
fn objects(path: &Path) -> Objects {
    fn walk(root: &Path, path: &Path, found: &mut Objects) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &entry.path(), found);
            } else {
                assert!(entry.file_type().unwrap().is_file());
                let mut file = fs::File::open(entry.path()).unwrap();
                let stamp = mdreader_capture::observe_object(&file, false, false).unwrap();
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).unwrap();
                assert_eq!(
                    stamp,
                    mdreader_capture::observe_object(&file, false, false).unwrap()
                );
                found.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    (stamp, bytes),
                );
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(path, path, &mut found);
    found
}
struct OriginalProcess {
    child: Child,
    input: Option<ChildStdin>,
}
impl OriginalProcess {
    fn start(binary: &Path) -> Self {
        // Zero arguments exercises the production standard-profile GUI shape.
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        Self { child, input }
    }
    fn quit(&mut self) {
        if let Some(mut input) = self.input.take() {
            input.write_all(b"quit\n").unwrap();
            input.flush().unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(
                Instant::now() < deadline,
                "synthetic participant did not exit normally"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for OriginalProcess {
    fn drop(&mut self) {
        // Ordinary cooperative exit even during fixture failure; never kill.
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(b"quit\n");
            let _ = input.flush();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if self.child.try_wait().is_ok_and(|status| status.is_some()) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
fn closed(root: &SourceRoot, binary: &Path) -> ClosedOriginal {
    let mut process = OriginalProcess::start(binary);
    let mut observer = LegacyApplicationFence::capture(root, binary).unwrap();
    assert!(matches!(observer.verify_closed(), Err(Error::NotQuiescent)));
    process.quit();
    observer.verify_closed().unwrap();
    let mut closed = ClosedOriginal::acquire(observer, Limits::default()).expect(
        "actual legacy fixture admission after observed normal process exit; inspect static admission phase diagnostics above",
    );
    closed.revalidate().unwrap();
    closed
}
fn preserved(draft: &mut native_legacy::Draft) {
    let operation = draft.operation;
    draft
        .control
        .as_mut()
        .unwrap()
        .with_library_connection(|db| {
            let tx = db.transaction().unwrap();
            let (transition, _) = observe(&tx, operation).unwrap();
            assert_eq!(transition.state, State::Preserved);
            assert!(transition.candidate.is_none());
            assert!(!marker::present(&tx).unwrap());
        });
}
fn fixture(root: &Path) {
    let catalog = root.join("workspaces").join(Uuid::new_v4().to_string());
    fs::create_dir_all(&catalog).unwrap();
    let db = rusqlite::Connection::open(catalog.join("catdb.db")).unwrap();
    db.execute_batch(
        "PRAGMA journal_mode=DELETE;
        CREATE TABLE objects(object_id TEXT PRIMARY KEY,entity TEXT,value TEXT,provenance TEXT);
        CREATE TABLE future_history(value BLOB);
        INSERT INTO future_history VALUES(x'00ff0102');",
    )
    .unwrap();
    for title in ["First legacy decision", "Second legacy decision"] {
        db.execute(
            "INSERT INTO objects VALUES(?1,'kn.Claim',?2,'{}')",
            rusqlite::params![
                Uuid::new_v4().to_string(),
                serde_json::json!({"claim_text":title}).to_string()
            ],
        )
        .unwrap();
    }
    drop(db);
    fs::write(root.join("opaque 雪 # 100% %3F.bin"), [0, 255, 1, 2, 3]).unwrap();
    fs::write(
        root.join("settings.json"),
        br#"{"settings":{"theme":"dark"},"future":{"retained":true}}"#,
    )
    .unwrap();
}
pub(in crate::profile_transition::offline::windows_workspace) fn run() {
    if std::env::var_os(DRIVER).is_none() {
        let temporary = tempfile::tempdir().unwrap();
        let appdata = temporary.path().join("synthetic appdata");
        fs::create_dir(&appdata).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(DRIVER, "1")
            .env("APPDATA", appdata.canonicalize().unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let appdata = PathBuf::from(std::env::var_os("APPDATA").unwrap());
    assert_eq!(appdata.file_name().unwrap(), "synthetic appdata");
    let original_path = appdata.join("dev.mdreader.app");
    fs::create_dir(&original_path).unwrap();
    fixture(&original_path);
    let root = SourceRoot::capture(original_path.clone()).unwrap();
    let binary = PathBuf::from(
        std::env::var_os("MDREADER_WINDOWS_LEGACY_FIXTURE")
            .expect("compile fixtures/windows-legacy-owner.rs as a synthetic MDReader.exe"),
    );
    let before = objects(&original_path);
    let mut original = closed(&root, &binary);
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(original_path.join("settings.json"))
        .is_err());
    let mut draft = native_legacy::Draft::begin(&mut original, &appdata, Uuid::new_v4()).unwrap();
    let choices = draft.documents().unwrap();
    assert_eq!(choices.len(), 2);
    let selected = [Selection {
        document_id: choices[0].document_id.clone(),
        revision: choices[0].revision.clone(),
    }];
    let changed = [Selection {
        document_id: choices[1].document_id.clone(),
        revision: choices[1].revision.clone(),
    }];
    native_legacy::tests::interrupt_next();
    assert!(matches!(
        draft.finish(&mut original, &selected),
        Err(Error::ReconciliationRequired)
    ));
    preserved(&mut draft);
    let candidate_path = draft.root.path.join("candidate");
    let candidate_before = objects(&candidate_path);
    let archive_before = objects(draft.archive.path());
    let raw_before = objects(&draft.root.path.join("original"));
    assert!(matches!(
        draft.finish(&mut original, &changed),
        Err(Error::Conflict)
    ));
    preserved(&mut draft);
    assert_eq!(objects(&candidate_path), candidate_before);
    let mut prepared = draft.finish(&mut original, &selected).unwrap();
    assert_eq!(prepared.imported().unwrap(), 1);
    assert_eq!(prepared.state().unwrap(), "prepared");
    let store = prepared.candidate.as_mut().unwrap().store().unwrap();
    let imported = crate::profile_transition::offline::conversion::documents(&store).unwrap();
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].name, choices[0].name);
    let body = mdreader_core::device::catalog::read(
        &store,
        &mdreader_core::device::catalog::receipt(&imported[0]),
    )
    .unwrap();
    assert!(body
        .markdown
        .contains(choices[0].name.strip_suffix(".md").unwrap()));
    assert!(body.markdown.contains("Archived kn.Claim"));
    drop(store);

    assert_eq!(objects(&candidate_path), candidate_before);
    assert_eq!(objects(draft.archive.path()), archive_before);
    assert_eq!(objects(&draft.root.path.join("original")), raw_before);
    assert_eq!(objects(&original_path), before);
    drop(prepared);
    drop(draft);

    // A second independently prepared operation loses its actual controller
    // lifetime before journal staging. Resume reobserves a NEW running original
    // and its ordinary exit; neither absent PID nor saved metadata grants closure.
    let mut draft = native_legacy::Draft::begin(&mut original, &appdata, Uuid::new_v4()).unwrap();
    let choices = draft.documents().unwrap();
    let selected = [Selection {
        document_id: choices[0].document_id.clone(),
        revision: choices[0].revision.clone(),
    }];
    native_legacy::tests::interrupt_next();
    assert!(matches!(
        draft.finish(&mut original, &selected),
        Err(Error::ReconciliationRequired)
    ));
    preserved(&mut draft);
    let saved = draft.root.path.clone();
    let candidate_before = objects(&saved.join("candidate"));
    let archive_before = objects(draft.archive.path());
    let archive_path = draft.archive.path().to_owned();
    let raw_before = objects(&saved.join("original"));
    let native_operation = draft.native_operation;
    drop(draft);
    drop(original);
    assert!(native_legacy::observation(&saved).unwrap().is_some());
    let mut original = closed(&root, &binary);
    let native_legacy::Recovery::Prepared(mut prepared) =
        native_legacy::Draft::resume(&saved, &mut original).unwrap()
    else {
        panic!("completed candidate was not reconciled");
    };
    assert_eq!(prepared.native_operation(), native_operation);
    assert_eq!(prepared.imported().unwrap(), 1);
    assert_eq!(prepared.state().unwrap(), "prepared");
    assert_eq!(objects(&saved.join("candidate")), candidate_before);
    assert_eq!(objects(&archive_path), archive_before);
    assert_eq!(objects(&saved.join("original")), raw_before);
    assert_eq!(objects(&original_path), before);
    original.revalidate().unwrap();
}
