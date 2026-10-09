//! Separate-profile archive, conversion receipt and durable activation journal.
use super::{owner::ClosedOwner, *};
use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_profile::{candidate, lifecycle::*, Inventory, Limits, VerifiedSnapshot};
use rusqlite::{Connection, OpenFlags};
use std::io::Write;
use uuid::Uuid;

#[path = "original_fence.rs"]
mod original_fence;
#[path = "recovery.rs"]
mod recovery;
#[path = "rollback.rs"]
mod rollback;
const DATABASE: &str = "reader-v1/library.sqlite";
const MARKER: &str = "native-reader-profile.json";
pub(super) struct Workspace {
    root: SourceRoot,
    original: ClosedOwner,
    candidate: ClosedOwner,
    journal: Connection,
    operation: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    receipt: CandidateReceipt,
}

pub(super) fn private_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .prefix(prefix)
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .map(tempfile::TempDir::keep)
        .map_err(|_| Error::Storage)
}
fn connection(root: &SourceRoot, name: &str) -> Result<Connection> {
    // Create privately before SQLite opens; never adopt a preexisting file.
    root.create_new_relative(Path::new(name))
        .map_err(|_| Error::Storage)?
        .sync_all()
        .map_err(|_| Error::Storage)?;
    let db = Connection::open(root.path.join(name)).map_err(|_| Error::Storage)?;
    db.pragma_update(None, "journal_mode", "DELETE")
        .map_err(|_| Error::Storage)?;
    db.pragma_update(None, "synchronous", "EXTRA")
        .map_err(|_| Error::Storage)?;
    Ok(db)
}
pub(super) fn export(original: &mut ClosedOwner, parent: &Path) -> Result<VerifiedSnapshot> {
    let limits = Limits::default();
    let choices = super::super::backup_inventory::selections(original.path(), &limits)?;
    let inventory = Inventory::capture(original.path(), &choices, &limits)?;
    let directory = private_directory(parent, "MDReader-archive-")?;
    mdreader_profile::stage(
        &mut super::super::BorrowedFreeze { frozen: original },
        inventory,
        &directory.join("original"),
        &limits,
    )
}
impl Workspace {
    pub(super) fn prepare(
        mut original: ClosedOwner,
        parent: &Path,
        selected: &[super::Selection],
    ) -> Result<Self> {
        let limits = Limits::default();
        original.revalidate()?;
        let choices = super::super::backup_inventory::selections(original.path(), &limits)?;
        let inventory = Inventory::capture(original.path(), &choices, &limits)?;
        let directory = private_directory(parent, "MDReader-migration-")?;
        let root = SourceRoot::capture(directory).map_err(|_| Error::Storage)?;
        let mut journal = connection(&root, "selection.sqlite")?;
        let (operation, generation) = {
            let mut lifecycle = Journal::attach(&mut journal)?;
            let generation = lifecycle.bootstrap(&inventory)?.generation;
            (lifecycle.prepare(&inventory, generation)?, generation)
        };
        let snapshot = mdreader_profile::stage(
            &mut super::super::BorrowedFreeze {
                frozen: &mut original,
            },
            inventory,
            &root.path.join("original"),
            &limits,
        )?;
        Journal::attach(&mut journal)?.preserved(operation, &snapshot, &limits)?;
        let candidate_path = root
            .create_or_capture_directory_relative(Path::new("candidate"))
            .map_err(|_| Error::Storage)?
            .path;
        let plan = candidate::prepare_at_database(
            operation,
            &snapshot,
            &candidate_path,
            Path::new(DATABASE),
            &limits,
        )?;
        let mut source = original.store()?;
        let mut destination =
            ReaderStore::open(&candidate_path.join("reader-v1")).map_err(|_| Error::Storage)?;
        let (count, digest) = super::conversion::convert(
            &mut source,
            &mut destination,
            original.path(),
            &candidate_path,
            selected,
        )?;
        original.revalidate()?;
        snapshot.revalidate(&limits)?;
        let receipt = destination.with_library_connection(|db| -> Result<_> {
            let tx = db.transaction().map_err(|_| Error::Storage)?;
            let receipt = candidate::record(&tx, &plan, count, &digest)?;
            original.revalidate()?;
            tx.commit().map_err(|_| Error::Storage)?;
            Ok(receipt)
        })?;
        drop(destination);
        drop(source);
        let candidate_root =
            SourceRoot::capture(candidate_path.clone()).map_err(|_| Error::Storage)?;
        let bytes = serde_json::to_vec(&Marker {
            version: 1,
            receipt: receipt.clone(),
        })
        .map_err(|_| Error::Invalid)?;
        let mut marker = candidate_root
            .create_new_relative(Path::new(MARKER))
            .map_err(|_| Error::Storage)?;
        marker
            .write_all(&bytes)
            .and_then(|()| marker.sync_all())
            .map_err(|_| Error::Storage)?;
        candidate_root
            .try_clone_directory_handle()
            .map_err(|_| Error::Storage)?
            .sync_all()
            .map_err(|_| Error::Storage)?;
        candidate_root
            .create_new_relative(Path::new(".mdreader-json-owner.lock"))
            .map_err(|_| Error::Storage)?
            .sync_all()
            .map_err(|_| Error::Storage)?;
        let candidate = ClosedOwner::acquire(&candidate_path)?;
        let mut workspace = Self {
            root,
            original,
            candidate,
            journal,
            operation,
        };
        workspace.with_journal(|journal, owners| journal.staged(operation, &receipt, owners))?;
        if generation != 0 {
            return Err(Error::Conflict);
        }
        Ok(workspace)
    }
    fn with_journal<T>(
        &mut self,
        action: impl FnOnce(&mut Journal<'_>, &mut Owners<'_>) -> Result<T>,
    ) -> Result<T> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        let mut owners = Owners {
            original: &mut self.original,
            candidate: &mut self.candidate,
            rollback: None,
        };
        let result = action(&mut Journal::attach(&mut self.journal)?, &mut owners)?;
        self.root.verify().map_err(|_| Error::Conflict)?;
        Ok(result)
    }
    pub(super) fn activate(&mut self) -> Result<PathBuf> {
        let operation = self.operation;
        let self_root = self.root.path.clone();
        let receipt = self.with_journal(|journal, owners| {
            let transition = journal.get(operation)?;
            let candidate = transition.candidate.as_ref().ok_or(Error::Conflict)?;
            owners.revalidate()?;
            owners.verify_candidate(candidate)?;
            let selection = journal.selection()?;
            match transition.state {
                State::Staged => {
                    let limits = Limits::default();
                    let choices = super::super::backup_inventory::selections(
                        owners.original.path(),
                        &limits,
                    )?;
                    let current = Inventory::capture(owners.original.path(), &choices, &limits)?;
                    let snapshot = mdreader_profile::verify(
                        &self_root.join("original"),
                        &candidate.snapshot,
                        &limits,
                    )?;
                    original_fence::verify(&current, snapshot.manifest())?;
                }
                State::Active
                    if selection.root == candidate.candidate
                        && Some(selection.generation) == transition.activation_generation => {}
                _ => return Err(Error::Conflict),
            }
            journal.activate(operation, transition.expected_generation, owners)
        })?;
        receipt.selected_root.locator.restore()
    }
    pub(super) fn imported(&mut self) -> Result<u32> {
        let op = Journal::attach(&mut self.journal)?.get(self.operation)?;
        u32::try_from(op.candidate.ok_or(Error::Conflict)?.imported_records)
            .map_err(|_| Error::Bounds)
    }
}
struct Owners<'a> {
    original: &'a mut ClosedOwner,
    candidate: &'a mut ClosedOwner,
    rollback: Option<&'a rollback::PreservedWrites>,
}
impl ActivationOwner for Owners<'_> {
    fn with_frozen<T>(
        &mut self,
        transition: &Transition,
        action: impl FnOnce(&mut dyn FrozenActivation) -> Result<T>,
    ) -> Result<T> {
        if self.original.binding() != &transition.original
            || transition
                .candidate
                .as_ref()
                .is_none_or(|receipt| &receipt.candidate != self.candidate.binding())
        {
            return Err(Error::Conflict);
        }
        self.revalidate()?;
        action(self)
    }
}
impl FrozenActivation for Owners<'_> {
    fn revalidate(&mut self) -> Result<()> {
        self.original.revalidate()?;
        self.candidate.revalidate()
    }
    fn verify_candidate(&mut self, receipt: &CandidateReceipt) -> Result<()> {
        if self.original.binding() != &receipt.original
            || self.candidate.binding() != &receipt.candidate
        {
            return Err(Error::Conflict);
        }
        self.candidate
            .with_database(Path::new(DATABASE), &mut |db| {
                candidate::verify_committed_at_database(db, receipt, Path::new(DATABASE))
            })
    }
    fn verify_rollback_preservation(&mut self, activation: &ActivationReceipt) -> Result<()> {
        let proof = self.rollback.ok_or(Error::ReconciliationRequired)?;
        if activation.kind != ReceiptKind::Activation
            || activation.operation != proof.operation
            || self.candidate.binding() != &activation.selected_root
            || &proof.snapshot.manifest().original != self.candidate.binding()
            || activation.to_generation != proof.generation
        {
            return Err(Error::Conflict);
        }
        proof.fence.verify()
    }
}
/// Explicit native prepared-profile startup. Validates exact root and committed
/// import receipt, never restores old source grants or OS-vault references.
pub(crate) fn verify_prepared(path: &Path) -> Result<()> {
    use std::io::Read;
    let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
    let file = root
        .open_optional_file_relative(Path::new(MARKER))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Invalid)?;
    let mut bytes = Vec::new();
    file.take(262145)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > 262144 {
        return Err(Error::Bounds);
    }
    let marker: Marker = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    let expected = &marker.receipt.candidate;
    let restored = SourceRoot::restore(
        expected.locator.restore()?,
        mdreader_core::SourceRootIdentity::Unix {
            device: expected.device,
            inode: expected.inode,
        },
    )
    .map_err(|_| Error::Conflict)?;
    if marker.version != 1 || root.path != restored.path || root.identity != restored.identity {
        return Err(Error::Conflict);
    }
    let db = Connection::open_with_flags(
        root.path.join(DATABASE),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| Error::Storage)?;
    candidate::verify_committed_at_database(&db, &marker.receipt, Path::new(DATABASE))?;
    let parent = root.path.parent().ok_or(Error::Invalid)?;
    if root.path.file_name() != Some(std::ffi::OsStr::new("candidate")) {
        return Err(Error::Invalid);
    }
    let mut journal = Connection::open_with_flags(
        parent.join("selection.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| Error::Storage)?;
    let mut journal = Journal::attach(&mut journal)?;
    let transition = journal.get(marker.receipt.operation)?;
    let selected = journal.selection()?;
    if transition.state != State::Active
        || selected.root != marker.receipt.candidate
        || selected.mode != SelectionMode::ReaderOnly
        || Some(selected.generation) != transition.activation_generation
        || transition.candidate.as_ref().is_none_or(|receipt| {
            serde_json::to_vec(receipt).ok() != serde_json::to_vec(&marker.receipt).ok()
        })
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)
}

/// Startup follows only an explicit existing native selector, with exact
/// generation/root identity. Rollback opens the retained original in place;
/// no archive bytes/grants/vault entries are copied back into it.
pub(crate) fn selected_for_launch(path: &Path) -> Result<SourceRoot> {
    use std::os::unix::fs::PermissionsExt;
    let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
    if std::fs::symlink_metadata(&root.path)
        .map_err(|_| Error::Storage)?
        .permissions()
        .mode()
        & 0o077
        != 0
    {
        return Err(Error::Invalid);
    }
    let mut connection = Connection::open_with_flags(
        root.path.join("selection.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| Error::Storage)?;
    let mut journal = Journal::attach(&mut connection)?;
    let selection = journal.selection()?;
    let transitions = journal.recover(None, 2)?;
    if transitions.len() != 1 {
        return Err(Error::Conflict);
    }
    let transition = &transitions[0];
    let selected = SourceRoot::restore(
        selection.root.locator.restore()?,
        mdreader_core::SourceRootIdentity::Unix {
            device: selection.root.device,
            inode: selection.root.inode,
        },
    )
    .map_err(|_| Error::Conflict)?;
    match selection.mode {
        SelectionMode::ReaderOnly => verify_prepared(&selected.path)?,
        SelectionMode::LegacyCurrent
            if transition.state == State::RolledBack
                && selection.root == transition.original
                && Some(selection.generation) == transition.rollback_generation =>
        {
            rollback::verify_record(&root, transition)?;
        }
        _ => return Err(Error::Conflict),
    }
    root.verify().map_err(|_| Error::Conflict)?;
    selected.verify().map_err(|_| Error::Conflict)?;
    Ok(selected)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
