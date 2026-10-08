//! Exactly one retained issued backup. Guard custody survives unwind/uncertainty.
use super::*;
use crate::preservation::{BackupStatus, guards::profile_error};
use mdreader_profile::database_backup::{BackupRequest, RemoteBackup};
use std::sync::Arc;

pub(super) struct Work {
    guards: Mutex<Option<Box<Guards>>>,
}
pub(super) struct Record {
    request: BackupRequest,
    work: Arc<Work>,
    status: BackupStatus,
}
struct Finalize<'a> {
    session: &'a Session,
    operation: Uuid,
    finished: bool,
}
impl Drop for Finalize<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let mut state = self.session.lock();
            if let Some(active) = &mut state.active {
                if active.scope.operation == self.operation {
                    active.phase = Phase::Quarantined;
                    active.failure = Some(Error::Uncertain);
                    if let Some(record) = &mut active.backup {
                        record.status = BackupStatus::Failed {
                            error: Error::Uncertain,
                        };
                    }
                }
            }
        }
    }
}
impl Session {
    pub(super) fn reconcile_backup(
        &self,
        scope: &Scope,
        lease: Uuid,
        request: &BackupRequest,
    ) -> Result<BackupStatus> {
        self.validate_scope(scope)?;
        request.validate().map_err(profile_error)?;
        let state = self.lock();
        if state.broken {
            return Err(Error::Quarantined);
        }
        let active = state.active.as_ref().ok_or(Error::UnknownOperation)?;
        if &active.scope != scope || active.lease != lease {
            return Err(Error::NotHeld);
        }
        let record = active.backup.as_ref().ok_or(Error::UnknownOperation)?;
        if &record.request != request {
            return Err(Error::StaleOwner);
        }
        Ok(record.status.clone())
    }
    pub(super) fn backup(
        &self,
        scope: Scope,
        lease: Uuid,
        request: BackupRequest,
    ) -> Result<BackupStatus> {
        self.validate_scope(&scope)?;
        request.validate().map_err(profile_error)?;
        // Only the actual held ReaderStore main database is selectable, never
        // arbitrary files or attached index/recovery paths from a caller.
        if request
            .source
            .root
            .locator
            .restore()
            .map_err(profile_error)?
            .join(request.source.relative.restore().map_err(profile_error)?)
            != self.root.path.join("library.sqlite")
        {
            return Err(Error::StaleOwner);
        }
        let (work, deadline) = {
            let mut state = self.lock();
            if state.broken {
                return Err(Error::Quarantined);
            }
            let active = state.active.as_mut().ok_or(Error::UnknownOperation)?;
            if active.scope != scope || active.lease != lease {
                return Err(Error::NotHeld);
            }
            if let Some(record) = &active.backup {
                if record.request != request {
                    return Err(Error::StaleOwner);
                }
                return Ok(record.status.clone());
            }
            if active.phase != Phase::Held || Instant::now() >= active.deadline {
                return Err(Error::NotHeld);
            }
            let guards = active.guards.as_ref().ok_or(Error::NotHeld)?;
            guards.verify(&self.owners, &scope)?;
            let work = Arc::new(Work {
                guards: Mutex::new(active.guards.take()),
            });
            active.backup = Some(Record {
                request: request.clone(),
                work: work.clone(),
                status: BackupStatus::Running,
            });
            active.phase = Phase::BackingUp;
            (
                work,
                active
                    .deadline
                    .min(Instant::now() + Duration::from_millis(request.duration_ms.into())),
            )
        };
        let mut finalize = Finalize {
            session: self,
            operation: scope.operation,
            finished: false,
        };
        // State retains the same Arc throughout. Even an unexpected unwind or
        // poisoned work mutex cannot drop the issued guards and reopen admission.
        let result = (|| {
            let retained = work.guards.lock().map_err(|_| Error::Quarantined)?;
            let guards = retained.as_ref().ok_or(Error::Quarantined)?;
            let mut output =
                RemoteBackup::open(request.clone(), deadline).map_err(profile_error)?;
            guards.with_main(|source| output.write_from(source))?;
            // No ReaderStore mutex across hashing/streaming the private output.
            let receipt = output.finish().map_err(profile_error)?;
            guards.with_main(|source| output.check_source(source))?;
            guards.verify(&self.owners, &scope)?;
            self.validate_scope(&scope)?;
            output.verify_receipt(&receipt).map_err(profile_error)?;
            Ok(receipt)
        })();
        let mut state = self.lock();
        let broken = state.broken;
        let active = state.active.as_mut().ok_or(Error::Quarantined)?;
        if active.scope != scope || active.lease != lease {
            return Err(Error::Quarantined);
        }
        let record = active.backup.as_mut().ok_or(Error::Quarantined)?;
        if !Arc::ptr_eq(&record.work, &work) || record.request != request {
            return Err(Error::Quarantined);
        }
        let valid = !broken && active.phase == Phase::BackingUp && Instant::now() < active.deadline;
        record.status = match result {
            Ok(receipt) if valid => BackupStatus::Completed {
                receipt: Box::new(receipt),
            },
            Ok(_) => BackupStatus::Failed {
                error: Error::Quarantined,
            },
            Err(error) => BackupStatus::Failed { error },
        };
        if matches!(record.status, BackupStatus::Completed { .. }) {
            active.guards = work.guards.lock().map_err(|_| Error::Quarantined)?.take();
            active.phase = Phase::Held;
        } else {
            active.phase = Phase::Quarantined;
            active.failure = Some(Error::Uncertain);
            // The record still holds the actual guards, including partial output.
        }
        let status = record.status.clone();
        finalize.finished = true;
        Ok(status)
    }
}
