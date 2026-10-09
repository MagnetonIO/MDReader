//! A saved preview supplies archive provenance, never reconstructed closure.
use super::super::legacy_record::{Conversion, Record};
pub(in crate::profile_transition::offline) enum Recovery {
    Draft(Box<Draft>),
    Prepared(Box<Prepared>),
}
use super::*;

fn saved(root: &SourceRoot, db: &mut rusqlite::Connection) -> Result<Option<(Record, Transition)>> {
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    if super::super::marker::present(&tx)? {
        return Ok(None);
    }
    let Some(record) = super::super::legacy_record::read(&tx)? else {
        return Ok(None);
    };
    if record.draft.is_none() {
        return Err(Error::Unsupported);
    }
    let (transition, selected) = observe(&tx, record.operation)?;
    if !matches!(transition.state, State::Preserved | State::Staged)
        || transition.original != *record.launch.profile()
        || selected.root != transition.original
        || selected.mode != SelectionMode::LegacyCurrent
        || selected.generation != transition.expected_generation
        || match transition.state {
            State::Preserved => transition.candidate.is_some(),
            State::Staged => transition.candidate.is_none(),
            _ => true,
        }
        || transition.activation_generation.is_some()
        || transition.rollback_generation.is_some()
        || transition.snapshot.is_none()
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    tx.commit().map_err(|_| Error::Conflict)?;
    Ok(Some((record, transition)))
}
fn selected(path: &Path) -> Result<SourceRoot> {
    let root =
        super::super::selection::restore(&mdreader_profile::RootBinding::capture_private(path)?)?;
    if root
        .open_optional_file_relative(Path::new("control/library.sqlite"))
        .map_err(|_| Error::Conflict)?
        .is_none()
    {
        return Err(Error::Invalid);
    }
    Ok(root)
}
pub(in crate::profile_transition::offline) fn observation(
    path: &Path,
) -> Result<
    Option<(
        Uuid,
        SourceRoot,
        mdreader_profile::legacy_windows::LaunchRecord,
    )>,
> {
    let root = selected(path)?;
    let mut control = ReaderStore::open_durable_readonly(&root.path.join("control"))
        .map_err(|_| Error::Conflict)?;
    let Some((record, transition)) = control.with_library_connection(|db| saved(&root, db))? else {
        return Ok(None);
    };
    let snapshot = mdreader_profile::verify(
        &root.path.join("original"),
        transition.snapshot.as_ref().ok_or(Error::Conflict)?,
        &Limits::default(),
    )?;
    if snapshot.manifest().original != transition.original
        || snapshot.manifest().inventory_sha256 != transition.inventory_hash
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    let original = super::super::selection::restore_legacy(&transition.original)?;
    Ok(Some((
        record
            .draft
            .as_ref()
            .ok_or(Error::Unsupported)?
            .native_operation,
        original,
        record.launch,
    )))
}
impl Draft {
    pub(super) fn verify_lock(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        let private = mdreader_capture::PrivateDirectory::open(&self.root.path)
            .map_err(|_| Error::Conflict)?;
        let named = private
            .open_existing_lock(Path::new("draft.lock"))
            .map_err(|_| Error::Conflict)?;
        if mdreader_capture::observe_object(&self.lock.0, false, true)
            .map_err(|_| Error::Conflict)?
            != self.lock.1
            || mdreader_capture::observe_object(&named, false, true).map_err(|_| Error::Conflict)?
                != self.lock.1
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub(in crate::profile_transition::offline) fn resume(
        path: &Path,
        original: &mut ClosedOriginal,
    ) -> Result<Recovery> {
        let root = selected(path)?;
        let mut observed = ReaderStore::open_durable_readonly(&root.path.join("control"))
            .map_err(|_| Error::Conflict)?;
        let (record, transition) = observed
            .with_library_connection(|db| saved(&root, db))?
            .ok_or(Error::Conflict)?;
        let draft = record.draft.as_ref().ok_or(Error::Unsupported)?;
        let private =
            mdreader_capture::PrivateDirectory::open(&root.path).map_err(|_| Error::Conflict)?;
        let lock = private
            .open_existing_lock(Path::new("draft.lock"))
            .map_err(|_| Error::Conflict)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::NotQuiescent)?;
        if mdreader_capture::observe_object(&lock, false, true).map_err(|_| Error::Conflict)?
            != draft.lock
        {
            return Err(Error::Conflict);
        }
        original.revalidate()?;
        if original.binding() != &transition.original
            || original.launch_record()? != record.launch
            || original.inventory()?.identity() != transition.inventory_hash
        {
            return Err(Error::Conflict);
        }
        let snapshot = mdreader_profile::verify(
            &root.path.join("original"),
            transition.snapshot.as_ref().ok_or(Error::Conflict)?,
            &Limits::default(),
        )?;
        if snapshot.manifest().original != transition.original
            || snapshot.manifest().inventory_sha256 != transition.inventory_hash
        {
            return Err(Error::Conflict);
        }
        drop(observed);
        let mut control =
            ReaderStore::open_private(&root.path.join("control")).map_err(|_| Error::Storage)?;
        let actual = control
            .with_library_connection(|db| saved(&root, db))?
            .ok_or(Error::Conflict)?;
        if serde_json::to_vec(&actual).map_err(|_| Error::Integrity)?
            != serde_json::to_vec(&(&record, &transition)).map_err(|_| Error::Integrity)?
        {
            return Err(Error::Conflict);
        }
        // A completed candidate may precede the Staged journal commit. Its
        // sealed receipt is observed from the actual closed candidate, never
        // inferred from an existing directory or reconstructed OS handle.
        let candidate_path = root.path.join("candidate");
        if transition.state == State::Staged
            || candidate_path.try_exists().map_err(|_| Error::Conflict)?
        {
            let conversion = draft.conversion.as_ref().ok_or(Error::Conflict)?;
            let archive = ConversionArchive::reopen(
                &root.path.join(&conversion.relative),
                &conversion.identity,
            )?;
            let (mut candidate, receipt) = super::completed::candidate(
                &root,
                &record,
                &snapshot,
                &archive,
                transition.candidate.as_ref(),
            )?;
            control.with_library_connection(|db| -> Result<()> {
                let mut owners = Owners {
                    original,
                    candidate: &mut candidate,
                    rollback: None,
                };
                // Preserved adoption and idempotent Staged admission both
                // borrow actual retained original and candidate owners.
                Journal::attach(db)?.staged(record.operation, &receipt, &mut owners)?;
                super::super::marker::record(db, draft.native_operation, &receipt)?;
                owners.revalidate()?;
                root.verify().map_err(|_| Error::Conflict)
            })?;
            return Ok(Recovery::Prepared(Box::new(Prepared {
                control,
                candidate: Some(candidate),
                root,
                operation: record.operation,
                native_operation: draft.native_operation,
                candidate_handoff: Default::default(),
                rollback_fence: None,
            })));
        }
        let archive = if let Some(conversion) = &draft.conversion {
            ConversionArchive::reopen(&root.path.join(&conversion.relative), &conversion.identity)?
        } else {
            // Keep an interrupted conversion intact. A fresh absent name is
            // filled only from the same raw snapshot and actual closed owner.
            let relative = format!("conversion-{}", Uuid::new_v4());
            let archive =
                original.conversion_archive(&snapshot, &root.path, &root.path.join(&relative))?;
            control.with_library_connection(|db| {
                super::super::legacy_record::conversion(
                    db,
                    record.operation,
                    Conversion {
                        relative,
                        identity: archive.identity().clone(),
                    },
                )
            })?;
            archive
        };
        original.revalidate()?;
        root.verify().map_err(|_| Error::Conflict)?;
        let result = Self {
            control: Some(control),
            lock: (lock, draft.lock),
            root,
            operation: record.operation,
            native_operation: draft.native_operation,
            snapshot,
            archive,
            launch: record.launch,
        };
        result.verify_lock()?;
        Ok(Recovery::Draft(Box::new(result)))
    }
}
