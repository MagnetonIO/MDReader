//! Activated Windows startup reads the committed selector and exact candidate
//! receipt. It grants no original roots and never opens the original SQLite VFS.
use super::*;
use mdreader_core::SourceRootIdentity;
use mdreader_profile::RootBinding;

pub(super) fn restore(binding: &RootBinding) -> Result<SourceRoot> {
    binding.validate()?;
    let root = SourceRoot::restore(
        binding.locator.restore()?,
        SourceRootIdentity::Windows {
            volume_serial_number: binding.device,
            file_id: binding.windows_file_id.ok_or(Error::Invalid)?,
        },
    )
    .map_err(|_| Error::Conflict)?;
    if RootBinding::capture_private(&root.path)? != *binding {
        return Err(Error::Conflict);
    }
    Ok(root)
}
impl Prepared {
    pub(super) fn launch_directory(&self) -> PathBuf {
        self.root.path.clone()
    }
    pub(super) fn state(&mut self) -> Result<&'static str> {
        self.verify()?;
        self.control.with_library_connection(|db| {
            let mut journal = Journal::attach(db)?;
            match journal.get(self.operation)?.state {
                State::Staged => Ok("prepared"),
                State::Active => Ok("launch_requested"),
                _ => Err(Error::Conflict),
            }
        })
    }
    pub(super) fn activate(&mut self, original: &mut ClosedOwner) -> Result<()> {
        self.verify()?;
        if self.candidate.is_none() {
            if self.state()? == "launch_requested" {
                selected_for_launch(&self.root.path)?;
                return Ok(());
            }
            self.close_candidate()?;
        }
        original.revalidate()?;
        let mut candidate = self.candidate.take().ok_or(Error::Conflict)?;
        let result = self.control.with_library_connection(|db| -> Result<()> {
            let mut journal = Journal::attach(db)?;
            let transition = journal.get(self.operation)?;
            let receipt = transition.candidate.as_ref().ok_or(Error::Conflict)?;
            let mut owners = Owners {
                original,
                candidate: &mut candidate,
            };
            owners.revalidate()?;
            owners.verify_candidate(receipt)?;
            if transition.state == State::Staged {
                let limits = Limits::default();
                let choices = super::super::super::backup_inventory::selections(
                    owners.original.path(),
                    &limits,
                )?;
                let inventory = Inventory::capture(owners.original.path(), &choices, &limits)?;
                let snapshot = mdreader_profile::verify(
                    &self.root.path.join("original"),
                    &receipt.snapshot,
                    &limits,
                )?;
                original_fence::verify(&inventory, snapshot.manifest())?;
            }
            journal.activate(self.operation, transition.expected_generation, &mut owners)?;
            self.root.verify().map_err(|_| Error::Conflict)
        });
        // An unsuccessful/unknown CAS keeps the real candidate closure guard.
        // Successful activation ends it before independently owned child startup.
        if result.is_err() {
            self.candidate = Some(candidate);
        }
        result
    }
}

pub(crate) fn selected_for_launch(path: &Path) -> Result<SourceRoot> {
    let original = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
    let binding = RootBinding::capture_private(path)?;
    let root = restore(&binding)?;
    original.verify().map_err(|_| Error::Conflict)?;
    if original.identity != root.identity {
        return Err(Error::Conflict);
    }
    let mut control = ReaderStore::open_durable_readonly(&root.path.join("control"))
        .map_err(|_| Error::Conflict)?;
    let (_, receipt) = control.with_library_connection(|db| marker::read(db))?;
    let selected = control.with_library_connection(|db| -> Result<_> {
        let mut journal = Journal::attach(db)?;
        let transition = journal.get(receipt.operation)?;
        let selection = journal.selection()?;
        if transition.state != State::Active
            || selection.mode != SelectionMode::ReaderOnly
            || selection.root != receipt.candidate
            || Some(selection.generation) != transition.activation_generation
            || transition.candidate.as_ref().is_none_or(|actual| {
                serde_json::to_vec(actual).ok() != serde_json::to_vec(&receipt).ok()
            })
        {
            return Err(Error::Conflict);
        }
        restore(&selection.root)
    })?;
    if selected.path != root.path.join("candidate") {
        return Err(Error::Conflict);
    }
    let mut candidate = ReaderStore::open_durable_readonly(&selected.path.join("reader-v1"))
        .map_err(|_| Error::Conflict)?;
    candidate.with_library_connection(|db| {
        candidate::verify_committed_at_database(db, &receipt, Path::new("reader-v1/library.sqlite"))
    })?;
    // The selector's identity is independent of candidate modifications.
    root.verify().map_err(|_| Error::Conflict)?;
    selected.verify().map_err(|_| Error::Conflict)?;
    Ok(selected)
}
pub(crate) fn verify_profile(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or(Error::Invalid)?;
    let selected = selected_for_launch(parent)?;
    let requested = restore(&RootBinding::capture_private(path)?)?;
    if selected.path != requested.path || selected.identity != requested.identity {
        return Err(Error::Conflict);
    }
    Ok(())
}
