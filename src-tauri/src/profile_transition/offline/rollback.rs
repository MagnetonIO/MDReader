//! Offline rollback first exports every candidate write. The original is never
//! restored from a snapshot, truncated, or overwritten by this operation.
use super::super::archive_fence::ArchiveFence;
use super::*;
use std::io::{Read, Write};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    operation: Uuid,
    generation: u64,
    directory: Uuid,
    snapshot: mdreader_profile::SnapshotIdentity,
}
pub(super) struct PreservedWrites {
    pub(super) operation: Uuid,
    pub(super) generation: u64,
    pub(super) snapshot: VerifiedSnapshot,
    pub(super) fence: ArchiveFence,
}
impl Workspace {
    pub(in crate::profile_transition::offline) fn launch_directory(&self) -> PathBuf {
        self.root.path.clone()
    }
    pub(in crate::profile_transition::offline) fn rollback(&mut self) -> Result<()> {
        self.original.revalidate()?;
        self.candidate.revalidate()?;
        let transition = Journal::attach(&mut self.journal)?.get(self.operation)?;
        if !matches!(transition.state, State::Active | State::RolledBack) {
            return Err(Error::Conflict);
        }
        let generation = transition.activation_generation.ok_or(Error::Conflict)?;
        let existing = self
            .root
            .open_optional_file_relative(Path::new("rollback.json"))
            .map_err(|_| Error::Conflict)?;
        let record = if let Some(file) = existing {
            let mut bytes = Vec::new();
            file.take(4097)
                .read_to_end(&mut bytes)
                .map_err(|_| Error::Storage)?;
            if bytes.len() > 4096 {
                return Err(Error::Bounds);
            }
            let record: Record = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
            if record.version != 1
                || record.operation != self.operation
                || record.generation != generation
                || record.directory.get_version_num() != 4
            {
                return Err(Error::Conflict);
            }
            record
        } else {
            if transition.state != State::Active {
                return Err(Error::ReconciliationRequired);
            }
            let directory = Uuid::new_v4();
            let path = self.root.path.join(format!("rollback-{directory}"));
            let limits = Limits::default();
            let choices =
                super::super::super::backup_inventory::selections(self.candidate.path(), &limits)?;
            let inventory = Inventory::capture(self.candidate.path(), &choices, &limits)?;
            let snapshot = mdreader_profile::stage(
                &mut super::super::super::BorrowedFreeze {
                    frozen: &mut self.candidate,
                },
                inventory,
                &path,
                &limits,
            )?;
            let record = Record {
                version: 1,
                operation: self.operation,
                generation,
                directory,
                snapshot: snapshot.identity().clone(),
            };
            let bytes = serde_json::to_vec(&record).map_err(|_| Error::Invalid)?;
            let mut file = self
                .root
                .create_new_relative(Path::new("rollback.json"))
                .map_err(|_| Error::Storage)?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| Error::Storage)?;
            self.root
                .try_clone_directory_handle()
                .map_err(|_| Error::Storage)?
                .sync_all()
                .map_err(|_| Error::Storage)?;
            record
        };
        let path = self
            .root
            .path
            .join(format!("rollback-{}", record.directory));
        let snapshot = mdreader_profile::verify(&path, &record.snapshot, &Limits::default())?;
        let proof = PreservedWrites {
            operation: self.operation,
            generation,
            fence: ArchiveFence::capture(&path, &snapshot)?,
            snapshot,
        };
        if transition.state == State::Active {
            // A previous uncertain preparation cannot preserve later edits.
            let choices = super::super::super::backup_inventory::selections(
                self.candidate.path(),
                &Limits::default(),
            )?;
            let current = Inventory::capture(self.candidate.path(), &choices, &Limits::default())?;
            if current.identity() != proof.snapshot.manifest().inventory_sha256 {
                return Err(Error::Conflict);
            }
        }
        let mut owners = Owners {
            original: &mut self.original,
            candidate: &mut self.candidate,
            rollback: Some(&proof),
        };
        owners.revalidate()?;
        owners.verify_candidate(transition.candidate.as_ref().ok_or(Error::Conflict)?)?;
        Journal::attach(&mut self.journal)?.rollback(self.operation, generation, &mut owners)?;
        proof.fence.verify()?;
        owners.revalidate()?;
        self.root.verify().map_err(|_| Error::Conflict)
    }
}

pub(super) fn verify_record(root: &SourceRoot, transition: &Transition) -> Result<()> {
    let file = root
        .open_optional_file_relative(Path::new("rollback.json"))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::ReconciliationRequired)?;
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > 4096 {
        return Err(Error::Bounds);
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if record.version != 1
        || record.operation != transition.id
        || Some(record.generation) != transition.activation_generation
        || record.directory.get_version_num() != 4
    {
        return Err(Error::Conflict);
    }
    let snapshot = mdreader_profile::verify(
        &root.path.join(format!("rollback-{}", record.directory)),
        &record.snapshot,
        &Limits::default(),
    )?;
    if transition
        .candidate
        .as_ref()
        .is_none_or(|candidate| snapshot.manifest().original != candidate.candidate)
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
