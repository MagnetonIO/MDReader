//! Restart reconciliation reads exact retained journal/receipt/archive. Never
//! starts an old writer or silently regenerates a lost operation.
use super::*;
impl Workspace {
    pub(in crate::profile_transition::offline) fn resume(path: &Path) -> Result<Self> {
        use std::{io::Read, os::unix::fs::PermissionsExt};
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
        let candidate_path = root.path.join("candidate");
        let candidate_root =
            SourceRoot::capture(candidate_path.clone()).map_err(|_| Error::Conflict)?;
        let file = candidate_root
            .open_optional_file_relative(Path::new(MARKER))
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Invalid)?;
        let mut raw = Vec::new();
        file.take(262145)
            .read_to_end(&mut raw)
            .map_err(|_| Error::Storage)?;
        if raw.len() > 262144 {
            return Err(Error::Bounds);
        }
        let marker: Marker = serde_json::from_slice(&raw).map_err(|_| Error::Invalid)?;
        if marker.version != 1 || marker.receipt.candidate.locator.restore()? != candidate_root.path
        {
            return Err(Error::Conflict);
        }
        let original = ClosedOwner::acquire(&marker.receipt.original.locator.restore()?)?;
        let candidate = ClosedOwner::acquire(&candidate_path)?;
        if original.binding() != &marker.receipt.original
            || candidate.binding() != &marker.receipt.candidate
        {
            return Err(Error::Conflict);
        }
        // No CREATE flag and no future/partial journal initialization. Require
        // the existing version marker before asking the portable owner to validate.
        if root
            .open_optional_file_relative(Path::new("selection.sqlite"))
            .map_err(|_| Error::Conflict)?
            .is_none()
        {
            return Err(Error::Invalid);
        }
        let mut journal = Connection::open_with_flags(
            root.path.join("selection.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| Error::Storage)?;
        let present: bool = journal.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='profile_transition_schema_v1' AND type='table')",
            [], |row| row.get(0)).map_err(|_| Error::Invalid)?;
        if !present {
            return Err(Error::Invalid);
        }
        let operation = marker.receipt.operation;
        let transition = Journal::attach(&mut journal)?.get(operation)?;
        if transition.candidate.as_ref().is_none_or(|receipt| {
            serde_json::to_vec(receipt).ok() != serde_json::to_vec(&marker.receipt).ok()
        }) {
            return Err(Error::Conflict);
        }
        mdreader_profile::verify(
            &root.path.join("original"),
            &marker.receipt.snapshot,
            &Limits::default(),
        )?;
        let mut workspace = Self {
            root,
            original,
            candidate,
            journal,
            operation,
        };
        workspace.with_journal(|_, owners| owners.verify_candidate(&marker.receipt))?;
        Ok(workspace)
    }
    pub(in crate::profile_transition::offline) fn status(&mut self) -> Result<&'static str> {
        Ok(
            match Journal::attach(&mut self.journal)?
                .get(self.operation)?
                .state
            {
                State::Staged => "prepared",
                State::Active => "activated",
                State::RolledBack => "rolled_back",
                _ => return Err(Error::ReconciliationRequired),
            },
        )
    }
}
