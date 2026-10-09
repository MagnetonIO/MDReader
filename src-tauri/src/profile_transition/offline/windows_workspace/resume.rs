//! Reattach only to a committed native selector. No original VFS is opened and
//! no live service is dispatched until its exact attempt can remain in Pending.
use super::*;

impl Prepared {
    pub(super) fn resume(path: &Path) -> Result<Self> {
        let captured = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
        let binding = mdreader_profile::RootBinding::capture_private(path)?;
        let root = selection::restore(&binding)?;
        if captured.identity != root.identity {
            return Err(Error::Conflict);
        }
        captured.verify().map_err(|_| Error::Conflict)?;
        // Never adopt a missing/partial control store as a new operation.
        if root
            .open_optional_file_relative(Path::new("control/library.sqlite"))
            .map_err(|_| Error::Conflict)?
            .is_none()
        {
            return Err(Error::Invalid);
        }
        let mut control = ReaderStore::open_durable_readonly(&root.path.join("control"))
            .map_err(|_| Error::Storage)?;
        let (native_operation, receipt) = control.with_library_connection(|db| marker::read(db))?;
        let candidate = selection::restore(&receipt.candidate)?;
        if candidate.path != root.path.join("candidate") {
            return Err(Error::Conflict);
        }
        let expected = control.with_library_connection(|db| -> Result<Transition> {
            let present: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='profile_transition_schema_v1')", [], |r| r.get(0)).map_err(|_| Error::Conflict)?;
            if !present { return Err(Error::Invalid); }
            let mut journal = Journal::attach(db)?;
            let transition = journal.get(receipt.operation)?;
            let selected = journal.selection()?;
            if transition.candidate.as_ref().is_none_or(|actual| serde_json::to_vec(actual).ok() != serde_json::to_vec(&receipt).ok())
                || transition.original != receipt.original
                || !matches!(transition.state, State::Staged | State::Active)
                || match transition.state {
                    State::Staged => selected.root != receipt.original || selected.mode != SelectionMode::LegacyCurrent || selected.generation != transition.expected_generation,
                    State::Active => selected.root != receipt.candidate || selected.mode != SelectionMode::ReaderOnly || Some(selected.generation) != transition.activation_generation,
                    _ => true,
                } { return Err(Error::Conflict); }
            Ok(transition)
        })?;
        mdreader_profile::verify(
            &root.path.join("original"),
            &receipt.snapshot,
            &Limits::default(),
        )?;
        let mut copied = ReaderStore::open_durable_readonly(&candidate.path.join("reader-v1"))
            .map_err(|_| Error::Conflict)?;
        copied.with_library_connection(|db| {
            candidate::verify_committed_at_database(
                db,
                &receipt,
                Path::new("reader-v1/library.sqlite"),
            )
        })?;
        drop(copied);
        drop(control);
        root.verify().map_err(|_| Error::Conflict)?;
        // Only the already verified native control copy gets a writable owner.
        // Recheck the complete operation after that adapter's initialization.
        let mut control =
            ReaderStore::open_private(&root.path.join("control")).map_err(|_| Error::Storage)?;
        control.with_library_connection(|db| -> Result<()> {
            let actual = marker::read(db)?;
            let transition = Journal::attach(db)?.get(receipt.operation)?;
            if actual.0 != native_operation
                || serde_json::to_vec(&actual.1).map_err(|_| Error::Invalid)?
                    != serde_json::to_vec(&receipt).map_err(|_| Error::Invalid)?
                || serde_json::to_vec(&transition).map_err(|_| Error::Invalid)?
                    != serde_json::to_vec(&expected).map_err(|_| Error::Invalid)?
            {
                return Err(Error::Conflict);
            }
            root.verify().map_err(|_| Error::Conflict)
        })?;
        Ok(Self {
            control,
            candidate: None,
            root,
            operation: receipt.operation,
            native_operation,
            candidate_handoff: Default::default(),
        })
    }
    pub(super) fn native_operation(&self) -> Uuid {
        self.native_operation
    }
    pub(super) fn original_root(&mut self) -> Result<SourceRoot> {
        self.verify()?;
        let (_, receipt) = self
            .control
            .with_library_connection(|db| marker::read(db))?;
        selection::restore(&receipt.original)
    }
    pub(super) fn handoff_issued(&self) -> bool {
        self.candidate_handoff.issued()
    }
    pub(super) fn handoff_failure(&self) -> Option<&'static str> {
        self.candidate_handoff.failure_kind()
    }
    pub(super) fn reconcile_handoff(&mut self) -> Result<()> {
        if self.candidate_handoff.issued() {
            self.close_candidate()?;
        }
        Ok(())
    }
    pub(super) fn close_candidate(&mut self) -> Result<()> {
        self.verify()?;
        if self.candidate.is_some() {
            return Ok(());
        }
        if self.state()? != "prepared" {
            return Err(Error::Conflict);
        }
        let (_, receipt) = self
            .control
            .with_library_connection(|db| marker::read(db))?;
        let root = selection::restore(&receipt.candidate)?;
        let mut owner = ClosedOwner::acquire_retaining(&root.path, &mut self.candidate_handoff)?;
        if owner.binding() != &receipt.candidate {
            return Err(Error::Conflict);
        }
        owner.verify_candidate(&receipt)?;
        self.candidate = Some(owner);
        Ok(())
    }
}
