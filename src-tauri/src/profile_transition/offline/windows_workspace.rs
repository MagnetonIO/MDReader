//! Windows selected-copy preparation. Portable lifecycle CAS borrows the real
//! private selector owner; filesystem names never substitute for committed SQL.
use super::{owner::ClosedOwner, Selection, *};
use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_profile::{candidate, lifecycle::*, Inventory, Limits};
use uuid::Uuid;
pub(super) mod controller;
mod controller_dispatch;
mod controller_record;
mod fence;
pub(super) mod legacy;
mod legacy_record;
mod marker;
mod original;
#[path = "original_fence.rs"]
mod original_fence;
mod resume;
mod rollback;
mod rollback_record;
mod selection;

pub(super) struct Prepared {
    // Connections/closed candidate end before the retained ancestor authority.
    control: ReaderStore,
    candidate: Option<ClosedOwner>,
    root: SourceRoot,
    operation: Uuid,
    native_operation: Uuid,
    candidate_handoff: super::shutdown_handoff::ShutdownHandoff,
    rollback_fence: Option<fence::Fence>,
}
pub(super) fn private_directory(parent: &Path, prefix: &str) -> Result<PathBuf> {
    let root = SourceRoot::capture(parent.to_owned()).map_err(|_| Error::InvalidDestination)?;
    let parent = mdreader_capture::SelectedDirectory::open(&root.path)
        .map_err(|_| Error::InvalidDestination)?;
    let name = format!("{prefix}{}", Uuid::new_v4());
    let child = parent
        .create_private_child(Path::new(&name))
        .map_err(|_| Error::Storage)?;
    let path = root.path.join(name);
    mdreader_profile::RootBinding::capture_private(&path)?;
    child.verify().map_err(|_| Error::Conflict)?;
    parent.verify().map_err(|_| Error::Conflict)?;
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(path)
}
impl Prepared {
    pub(super) fn prepare(
        original: &mut ClosedOwner,
        parent: &Path,
        selected: &[Selection],
        native_operation: Uuid,
    ) -> Result<Self> {
        let limits = Limits::default();
        original.revalidate()?;
        let choices = super::super::backup_inventory::selections(original.path(), &limits)?;
        let inventory = Inventory::capture(original.path(), &choices, &limits)?;
        let directory = private_directory(parent, "MDReader-migration-")?;
        let root = SourceRoot::capture(directory).map_err(|_| Error::Storage)?;
        let private =
            mdreader_capture::PrivateDirectory::open(&root.path).map_err(|_| Error::Conflict)?;
        let mut control =
            ReaderStore::open_private(&root.path.join("control")).map_err(|_| Error::Storage)?;
        let operation = control.with_library_connection(|db| -> Result<_> {
            let mut lifecycle = Journal::attach(db)?;
            let generation = lifecycle.bootstrap(&inventory)?.generation;
            if generation != 0 {
                return Err(Error::Conflict);
            }
            lifecycle.prepare(&inventory, generation)
        })?;
        let snapshot = mdreader_profile::stage(
            &mut super::super::BorrowedFreeze { frozen: original },
            inventory,
            &root.path.join("original"),
            &limits,
        )?;
        control.with_library_connection(|db| {
            Journal::attach(db)?.preserved(operation, &snapshot, &limits)
        })?;
        let child = private
            .create_child(Path::new("candidate"))
            .map_err(|_| Error::Storage)?;
        let path = root.path.join("candidate");
        let plan = candidate::prepare_at_database(
            operation,
            &snapshot,
            &path,
            Path::new("reader-v1/library.sqlite"),
            &limits,
        )?;
        let reader = mdreader_capture::PrivateDirectory::create(&path.join("reader-v1"))
            .map_err(|_| Error::Storage)?;
        let writer = reader
            .open_lock(Path::new("reader.lock"))
            .map_err(|_| Error::Storage)?;
        fs2::FileExt::try_lock_exclusive(&writer).map_err(|_| Error::NotQuiescent)?;
        let candidate_root = SourceRoot::capture(path.clone()).map_err(|_| Error::Storage)?;
        let json = super::super::persistence::JsonOwner::new(candidate_root)
            .map_err(|_| Error::Storage)?;
        let mut source = original.store()?;
        let mut destination =
            ReaderStore::open_private(&path.join("reader-v1")).map_err(|_| Error::Storage)?;
        let (count, digest) = super::conversion::convert(
            &mut source,
            &mut destination,
            original.path(),
            &path,
            selected,
        )?;
        original.revalidate()?;
        snapshot.revalidate(&limits)?;
        let receipt = destination.with_library_connection(|db| -> Result<_> {
            let tx = db.transaction().map_err(|_| Error::Storage)?;
            let receipt = candidate::record(&tx, &plan, count, &digest)?;
            original.revalidate()?;
            child.verify().map_err(|_| Error::Conflict)?;
            tx.commit().map_err(|_| Error::ReconciliationRequired)?;
            Ok(receipt)
        })?;
        drop(destination);
        drop(source);
        drop(json);
        drop(writer);
        // This is a newly created, native-owned candidate. A competing owner is
        // refused without dispatching a fresh, unretained service attempt.
        let mut candidate = ClosedOwner::acquire(&path)?;
        candidate.verify_candidate(&receipt)?;
        control.with_library_connection(|db| -> Result<()> {
            let mut owners = Owners {
                original,
                candidate: &mut candidate,
                rollback: None,
            };
            Journal::attach(db)?.staged(operation, &receipt, &mut owners)?;
            marker::record(db, native_operation, &receipt)?;
            owners.revalidate()?;
            root.verify().map_err(|_| Error::Conflict)
        })?;
        Ok(Self {
            control,
            candidate: Some(candidate),
            root,
            operation,
            native_operation,
            candidate_handoff: Default::default(),
            rollback_fence: None,
        })
    }
    pub(super) fn imported(&mut self) -> Result<u32> {
        self.verify()?;
        let receipt = self
            .control
            .with_library_connection(|db| marker::read(db))?;
        if receipt.0 != self.native_operation
            || receipt.1.operation != self.operation
            || mdreader_profile::RootBinding::capture_private(
                &receipt.1.candidate.locator.restore()?,
            )? != receipt.1.candidate
        {
            return Err(Error::Conflict);
        }
        u32::try_from(receipt.1.imported_records).map_err(|_| Error::Bounds)
    }
    fn verify(&mut self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        self.control
            .store_identity()
            .map_err(|_| Error::Conflict)?
            .verify_at(&self.root.path.join("control"))
            .map_err(|_| Error::Conflict)?;
        if let Some(fence) = &self.rollback_fence {
            fence.verify()?;
        }
        if let Some(candidate) = self.candidate.as_mut() {
            candidate.revalidate()?;
        }
        Ok(())
    }
}
struct Owners<'a> {
    original: &'a mut dyn original::Original,
    candidate: &'a mut ClosedOwner,
    rollback: Option<&'a rollback::Proof>,
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
        self.candidate.verify_candidate(receipt)
    }
    fn verify_rollback_preservation(&mut self, receipt: &ActivationReceipt) -> Result<()> {
        self.rollback
            .ok_or(Error::ReconciliationRequired)?
            .verify(receipt)
    }
}

#[cfg(test)]
mod tests;

pub(crate) use selection::{
    active_selection, legacy_selection, selected_for_launch, startup_selection, verify_profile,
};
