//! Native service preservation borrows no SQLite handle across processes.
use super::{FreezeScope, FrozenParticipant, LiveParticipant};
use mdreader_profile::{Error, Participant, Result, RootBinding};
use mdreader_service::preservation::{Attempt, BackupStatus, Client, Phase};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

pub(super) struct ServiceParticipant {
    root: RootBinding,
    directory: PathBuf,
    issued: Arc<Mutex<Option<Issued>>>,
}
struct Issued {
    operation: uuid::Uuid,
    epoch: uuid::Uuid,
    attempt: Attempt,
}
impl ServiceParticipant {
    /// Bind the exact reader-owned child; freeze connects without spawning it.
    pub(super) fn new(root: RootBinding, directory: &Path) -> Result<Self> {
        root.validate()?;
        let original = root.locator.restore()?;
        if directory != original.join("reader-v1") {
            return Err(Error::Invalid);
        }
        Ok(Self {
            root,
            directory: directory.to_owned(),
            issued: Arc::new(Mutex::new(None)),
        })
    }
}
impl LiveParticipant for ServiceParticipant {
    fn participant(&self) -> Participant {
        Participant::ReaderDaemon
    }
    fn root(&self) -> &RootBinding {
        &self.root
    }
    fn freeze(&self, scope: &FreezeScope<'_>) -> Result<Box<dyn FrozenParticipant>> {
        if scope.root() != &self.root {
            return Err(Error::Conflict);
        }
        let remaining = scope.deadline().saturating_duration_since(Instant::now());
        let millis = remaining.as_millis().min(30_000) as u32;
        if millis == 0 {
            return Err(Error::NotQuiescent);
        }
        let mut custody = self
            .issued
            .lock()
            .map_err(|_| Error::ReconciliationRequired)?;
        if custody.is_some() {
            return Err(Error::ReconciliationRequired);
        }
        let client = Client::connect(&self.directory).map_err(error)?;
        let attempt = client
            .attempt(scope.operation(), scope.owner_epoch(), millis)
            .map_err(error)?;
        *custody = Some(Issued {
            operation: scope.operation(),
            epoch: scope.owner_epoch(),
            attempt,
        });
        let held = custody
            .as_mut()
            .ok_or(Error::ReconciliationRequired)?
            .attempt
            .begin()
            .map_err(error)?;
        if held.phase != Phase::Held {
            return Err(Error::ReconciliationRequired);
        }
        Ok(Box::new(ServiceFrozen {
            issued: self.issued.clone(),
            root: self.root.clone(),
            operation: scope.operation(),
            epoch: scope.owner_epoch(),
            released: false,
        }))
    }
}
struct ServiceFrozen {
    issued: Arc<Mutex<Option<Issued>>>,
    root: RootBinding,
    operation: uuid::Uuid,
    epoch: uuid::Uuid,
    released: bool,
}
impl ServiceFrozen {
    fn with_attempt<T>(&self, action: impl FnOnce(&mut Attempt) -> Result<T>) -> Result<T> {
        let mut custody = self
            .issued
            .lock()
            .map_err(|_| Error::ReconciliationRequired)?;
        let issued = custody.as_mut().ok_or(Error::ReconciliationRequired)?;
        if issued.operation != self.operation || issued.epoch != self.epoch {
            return Err(Error::Conflict);
        }
        action(&mut issued.attempt)
    }
    fn exact(&self, scope: &FreezeScope<'_>) -> Result<()> {
        if self.released
            || scope.root() != &self.root
            || scope.operation() != self.operation
            || scope.owner_epoch() != self.epoch
            || Instant::now() >= scope.deadline()
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
impl FrozenParticipant for ServiceFrozen {
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> Result<()> {
        self.exact(scope)?;
        if self
            .with_attempt(|attempt| attempt.revalidate().map_err(error))?
            .phase
            != Phase::Held
        {
            return Err(Error::ReconciliationRequired);
        }
        Ok(())
    }
    fn release(&mut self, scope: &FreezeScope<'_>) -> Result<()> {
        self.exact(scope)?;
        if self
            .with_attempt(|attempt| attempt.release().map_err(error))?
            .phase
            != Phase::Released
        {
            return Err(Error::ReconciliationRequired);
        }
        self.released = true;
        self.issued
            .lock()
            .map_err(|_| Error::ReconciliationRequired)?
            .take();
        Ok(())
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> Result<()>,
    ) -> Result<()> {
        Err(Error::Unsupported)
    }
    fn backup_database(
        &mut self,
        target: &mut mdreader_profile::database_backup::DatabaseBackupTarget,
    ) -> Result<mdreader_profile::database_backup::BackupReceipt> {
        let status =
            self.with_attempt(|attempt| attempt.backup(target.request()).map_err(error))?;
        match status {
            BackupStatus::Completed { receipt } => {
                target.accept(&receipt)?;
                Ok(*receipt)
            }
            BackupStatus::Running | BackupStatus::Failed { .. } => {
                Err(Error::ReconciliationRequired)
            }
        }
    }
}
fn error(error: mdreader_service::preservation::Error) -> Error {
    use mdreader_service::preservation::Error as ServiceError;
    match error {
        ServiceError::Invalid => Error::Invalid,
        ServiceError::Bounds => Error::Bounds,
        ServiceError::Unsupported => Error::Unsupported,
        ServiceError::StaleOwner => Error::Conflict,
        ServiceError::Unavailable => Error::Storage,
        ServiceError::Busy | ServiceError::Deadline | ServiceError::NotHeld => Error::NotQuiescent,
        ServiceError::UnknownOperation | ServiceError::Quarantined | ServiceError::Uncertain => {
            Error::ReconciliationRequired
        }
    }
}
