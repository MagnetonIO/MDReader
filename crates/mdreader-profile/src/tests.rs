use super::*;
use crate::lifecycle::{
    ActivationOwner, ActivationReceipt, CandidateReceipt, FrozenActivation, Journal, State,
    Transition,
};
use rusqlite::Connection;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;
fn private(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
struct Fixture {
    temp: tempfile::TempDir,
    profile: PathBuf,
    owner: Owner,
}
struct Owner {
    binding: RootBinding,
    db: Connection,
    denied: bool,
    checks: usize,
    fail_at: Option<usize>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .tempdir_in(std::env::temp_dir().canonicalize().unwrap())
            .unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let profile = temp.path().join("reader-v1");
        private(&profile);
        fs::write(
            profile.join("settings.json"),
            br#"{"settings":{"theme":"dark"},"unknown":{"integer":9007199254740993}}"#,
        )
        .unwrap();
        let db = Connection::open(profile.join("library.db")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE unknown_records(id INTEGER PRIMARY KEY,raw BLOB); INSERT INTO unknown_records VALUES(9007199254740993,X'00FF0102');").unwrap();
        let binding = crate::platform::Root::capture(&profile).unwrap().binding;
        Self {
            temp,
            profile,
            owner: Owner {
                binding,
                db,
                denied: false,
                checks: 0,
                fail_at: None,
            },
        }
    }
    fn inventory(&self) -> Inventory {
        let selection = fs::read_dir(&self.profile)
            .unwrap()
            .map(|entry| {
                let name = entry.unwrap().file_name();
                let disposition = match name.to_str().unwrap() {
                    "library.db" => Disposition::Sqlite,
                    "settings.json" => Disposition::Raw,
                    _ => Disposition::Excluded(Exclusion::SqliteRecoverySidecar),
                };
                InventorySelection {
                    relative: PathBuf::from(name),
                    disposition,
                }
            })
            .collect::<Vec<_>>();
        Inventory::capture(&self.profile, &selection, &Limits::default()).unwrap()
    }
    fn stage(&mut self, name: &str) -> VerifiedSnapshot {
        let inventory = self.inventory();
        stage(
            &mut self.owner,
            inventory,
            &self.temp.path().join(name),
            &Limits::default(),
        )
        .unwrap()
    }
}
impl FreezeOwner for Owner {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T> {
        if self.denied || expected != &self.binding {
            return Err(Error::NotQuiescent);
        }
        action(self)
    }
}
impl FrozenProfile for Owner {
    fn binding(&self) -> &RootBinding {
        &self.binding
    }
    fn revalidate(&mut self) -> Result<()> {
        self.checks += 1;
        if self.fail_at == Some(self.checks) {
            return Err(Error::NotQuiescent);
        }
        let path = self.binding.locator.restore().map_err(|_| Error::Invalid)?;
        if crate::platform::Root::capture(&path)?.binding != self.binding {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    fn with_database(
        &mut self,
        path: &Path,
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()> {
        if path != Path::new("library.db") {
            return Err(Error::Invalid);
        }
        action(&self.db)
    }
}
struct Gate {
    candidate: Connection,
    checks: usize,
    fail_at: Option<usize>,
    rollback_allowed: bool,
}
impl ActivationOwner for Gate {
    fn with_frozen<T>(
        &mut self,
        _: &Transition,
        action: impl FnOnce(&mut dyn FrozenActivation) -> Result<T>,
    ) -> Result<T> {
        action(self)
    }
}
impl FrozenActivation for Gate {
    fn revalidate(&mut self) -> Result<()> {
        self.checks += 1;
        if self.fail_at == Some(self.checks) {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
    fn verify_candidate(&mut self, receipt: &CandidateReceipt) -> Result<()> {
        candidate::verify_committed(&self.candidate, receipt)
    }
    fn verify_rollback_preservation(&mut self, _: &ActivationReceipt) -> Result<()> {
        if self.rollback_allowed {
            Ok(())
        } else {
            Err(Error::ReconciliationRequired)
        }
    }
}
fn staged(f: &mut Fixture, control: &mut Connection) -> (Uuid, Gate, VerifiedSnapshot) {
    let inventory = f.inventory();
    let mut journal = Journal::attach(control).unwrap();
    journal.bootstrap(&inventory).unwrap();
    let id = journal.prepare(&inventory, 0).unwrap();
    let snapshot = stage(
        &mut f.owner,
        inventory,
        &f.temp.path().join("archive"),
        &Limits::default(),
    )
    .unwrap();
    journal
        .preserved(id, &snapshot, &Limits::default())
        .unwrap();
    let destination = f.temp.path().join("candidate");
    private(&destination);
    let mut db = Connection::open(destination.join("library.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE converted_user_records(id INTEGER PRIMARY KEY,raw BLOB)")
        .unwrap();
    let plan = candidate::prepare(id, &snapshot, &destination, &Limits::default()).unwrap();
    let tx = db.transaction().unwrap();
    tx.execute("INSERT INTO converted_user_records VALUES(1,X'007FFF')", [])
        .unwrap();
    let receipt = candidate::record(
        &tx,
        &plan,
        1,
        &crate::model::hash(b"exact synthetic conversion"),
    )
    .unwrap();
    assert!(matches!(
        candidate::verify_committed(&tx, &receipt),
        Err(Error::Conflict)
    ));
    tx.commit().unwrap();
    let mut gate = Gate {
        candidate: db,
        checks: 0,
        fail_at: None,
        rollback_allowed: false,
    };
    journal.staged(id, &receipt, &mut gate).unwrap();
    (id, gate, snapshot)
}
#[test]
fn wal_snapshot_preserves_unknown_rows_and_raw_bytes_reopens_and_refuses_tamper() {
    let mut f = Fixture::new();
    let original = fs::read(f.profile.join("settings.json")).unwrap();
    let snapshot = f.stage("archive");
    let manifest = snapshot.manifest();
    let sql = manifest
        .entries
        .iter()
        .find(|e| e.disposition == Disposition::Sqlite)
        .unwrap();
    let raw = manifest
        .entries
        .iter()
        .find(|e| e.disposition == Disposition::Raw)
        .unwrap();
    let db = Connection::open_with_flags(
        f.temp
            .path()
            .join("archive")
            .join(sql.member.as_ref().unwrap()),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let row = db
        .query_row("SELECT id,raw FROM unknown_records", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .unwrap();
    assert_eq!(row, (9_007_199_254_740_993, vec![0, 255, 1, 2]));
    assert_eq!(
        fs::read(
            f.temp
                .path()
                .join("archive")
                .join(raw.member.as_ref().unwrap())
        )
        .unwrap(),
        original
    );
    verify(
        &f.temp.path().join("archive"),
        snapshot.identity(),
        &Limits::default(),
    )
    .unwrap();
    fs::write(
        f.temp
            .path()
            .join("archive")
            .join(raw.member.as_ref().unwrap()),
        b"tampered",
    )
    .unwrap();
    assert!(matches!(
        verify(
            &f.temp.path().join("archive"),
            snapshot.identity(),
            &Limits::default()
        ),
        Err(Error::Integrity)
    ));
    assert_eq!(fs::read(f.profile.join("settings.json")).unwrap(), original);
}
#[test]
fn unclassified_changed_or_unquiesced_inventory_never_publishes() {
    let mut f = Fixture::new();
    let inventory = f.inventory();
    f.owner.denied = true;
    let dest = f.temp.path().join("denied");
    assert!(matches!(
        stage(&mut f.owner, inventory, &dest, &Limits::default()),
        Err(Error::NotQuiescent)
    ));
    assert!(!dest.exists());
    f.owner.denied = false;
    let inventory = f.inventory();
    fs::write(f.profile.join("settings.json"), b"changed").unwrap();
    assert!(matches!(
        stage(&mut f.owner, inventory, &dest, &Limits::default()),
        Err(Error::Conflict)
    ));
    assert!(!dest.exists());
    let mut selections = f
        .inventory()
        .entries()
        .iter()
        .map(|entry| InventorySelection {
            relative: entry.relative.restore().unwrap(),
            disposition: entry.disposition,
        })
        .collect::<Vec<_>>();
    selections[0].disposition = Disposition::Unclassified;
    let inventory = Inventory::capture(&f.profile, &selections, &Limits::default()).unwrap();
    assert!(matches!(
        stage(&mut f.owner, inventory, &dest, &Limits::default()),
        Err(Error::Unclassified)
    ));
    let limits = Limits {
        max_entries: 1,
        ..Limits::default()
    };
    assert!(matches!(
        Inventory::capture(&f.profile, &selections, &limits),
        Err(Error::Bounds)
    ));
    assert!(!dest.exists());
    let inventory = f.inventory();
    f.owner.fail_at = Some(f.owner.checks + 2);
    assert!(matches!(
        stage(&mut f.owner, inventory, &dest, &Limits::default()),
        Err(Error::NotQuiescent)
    ));
    assert!(!dest.exists());
    assert!(fs::read_dir(f.temp.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".profile-stage-")));
}
#[test]
fn candidate_receipt_and_selector_commit_reopen_reconcile_and_preserve_rollback() {
    let mut f = Fixture::new();
    let path = f.temp.path().join("control.sqlite");
    let mut db = Connection::open(&path).unwrap();
    let (id, mut gate, _snapshot) = staged(&mut f, &mut db);
    let original = fs::read(f.profile.join("settings.json")).unwrap();
    let activation = Journal::attach(&mut db)
        .unwrap()
        .activate(id, 0, &mut gate)
        .unwrap();
    drop(db);
    let mut db = Connection::open(&path).unwrap();
    let mut journal = Journal::attach(&mut db).unwrap();
    assert_eq!(journal.selection().unwrap().generation, 1);
    assert_eq!(
        journal.selection().unwrap().mode,
        crate::lifecycle::SelectionMode::ReaderOnly
    );
    let replay = journal.activate(id, 0, &mut gate).unwrap();
    assert_eq!(replay.id, activation.id);
    assert_eq!(journal.recover(None, 16).unwrap()[0].state, State::Active);
    assert!(matches!(
        journal.rollback(id, 1, &mut gate),
        Err(Error::ReconciliationRequired)
    ));
    assert_eq!(journal.selection().unwrap().generation, 1);
    // Private storage fixture only: native production must derive this from a
    // newly verified preservation/reconciliation receipt, not a UI boolean.
    gate.rollback_allowed = true;
    let rollback = journal.rollback(id, 1, &mut gate).unwrap();
    assert_eq!(rollback.to_generation, 2);
    assert_eq!(
        journal.selection().unwrap().mode,
        crate::lifecycle::SelectionMode::LegacyCurrent
    );
    assert!(journal.selection().unwrap().root == f.owner.binding);
    assert_eq!(journal.rollback(id, 1, &mut gate).unwrap().id, rollback.id);
    assert_eq!(fs::read(f.profile.join("settings.json")).unwrap(), original);
    assert_eq!(
        gate.candidate
            .query_row("SELECT COUNT(*) FROM converted_user_records", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn final_owner_fence_or_sql_trigger_rolls_back_selector_and_receipt_together() {
    let mut f = Fixture::new();
    let mut db = Connection::open(f.temp.path().join("control.sqlite")).unwrap();
    let (id, mut gate, _snapshot) = staged(&mut f, &mut db);
    let staged = Journal::attach(&mut db).unwrap().get(id).unwrap();
    db.execute_batch(
        "CREATE TRIGGER late_staged AFTER UPDATE ON profile_transitions_v1 \
         BEGIN UPDATE profile_transitions_v1 SET inventory_hash=printf('%064d',0); END;",
    )
    .unwrap();
    assert!(matches!(
        Journal::attach(&mut db)
            .unwrap()
            .staged(id, staged.candidate.as_ref().unwrap(), &mut gate),
        Err(Error::Conflict)
    ));
    assert_eq!(
        Journal::attach(&mut db)
            .unwrap()
            .get(id)
            .unwrap()
            .inventory_hash,
        staged.inventory_hash
    );
    db.execute_batch("DROP TRIGGER late_staged").unwrap();
    gate.fail_at = Some(gate.checks + 2);
    assert!(matches!(
        Journal::attach(&mut db).unwrap().activate(id, 0, &mut gate),
        Err(Error::NotQuiescent)
    ));
    assert_eq!(
        Journal::attach(&mut db)
            .unwrap()
            .selection()
            .unwrap()
            .generation,
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM profile_activation_receipts_v1",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    gate.fail_at = None;
    db.execute_batch("CREATE TRIGGER late_selector AFTER INSERT ON profile_activation_receipts_v1 BEGIN UPDATE profile_selection_v1 SET generation=generation+1; END;").unwrap();
    assert!(matches!(
        Journal::attach(&mut db).unwrap().activate(id, 0, &mut gate),
        Err(Error::Conflict)
    ));
    assert_eq!(
        Journal::attach(&mut db)
            .unwrap()
            .selection()
            .unwrap()
            .generation,
        0
    );
    db.execute_batch("DROP TRIGGER late_selector").unwrap();
    Journal::attach(&mut db)
        .unwrap()
        .activate(id, 0, &mut gate)
        .unwrap();
    assert_eq!(
        Journal::attach(&mut db)
            .unwrap()
            .selection()
            .unwrap()
            .generation,
        1
    );
}
#[test]
fn future_or_malformed_namespace_and_interrupted_candidate_leave_existing_state_untouched() {
    let mut f = Fixture::new();
    let inventory = f.inventory();
    let mut db = Connection::open(f.temp.path().join("control.sqlite")).unwrap();
    Journal::attach(&mut db)
        .unwrap()
        .bootstrap(&inventory)
        .unwrap();
    db.execute_batch("DROP TABLE profile_transition_schema_v1; CREATE TABLE profile_transition_schema_v1(singleton INTEGER PRIMARY KEY,version INTEGER NOT NULL); INSERT INTO profile_transition_schema_v1 VALUES(1,2);").unwrap();
    assert!(matches!(Journal::attach(&mut db), Err(Error::Invalid)));
    assert_eq!(
        db.query_row(
            "SELECT version FROM profile_transition_schema_v1",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT generation FROM profile_selection_v1", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    let snapshot = f.stage("archive");
    let candidate = f.temp.path().join("candidate");
    private(&candidate);
    let mut connection = Connection::open(candidate.join("library.sqlite")).unwrap();
    connection
        .execute_batch("CREATE TABLE user_rows(id INTEGER PRIMARY KEY)")
        .unwrap();
    let plan =
        candidate::prepare(Uuid::new_v4(), &snapshot, &candidate, &Limits::default()).unwrap();
    {
        let tx = connection.transaction().unwrap();
        tx.execute("INSERT INTO user_rows VALUES(1)", []).unwrap();
        candidate::record(&tx, &plan, 1, &crate::model::hash(b"synthetic")).unwrap();
        // Simulate process interruption before outer owner commit.
    }
    drop(connection);
    let connection = Connection::open(candidate.join("library.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM user_rows", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='profile_import_receipts_v1'",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
}
