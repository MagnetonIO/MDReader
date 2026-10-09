//! Retain exact native service handoff identity across uncertain replies. This
//! never proves closure: the caller must acquire the actual writer/spool locks.
use super::*;
use mdreader_core::SourceRoot;
use mdreader_service::preservation::{Attempt, Client, Error as ServiceError, Phase, Status};
use std::{
    fs::File,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct ShutdownHandoff {
    root: Option<SourceRoot>,
    attempt: Option<Attempt>,
    failure: Option<&'static str>,
}
impl ShutdownHandoff {
    pub(super) fn failure_kind(&self) -> Option<&'static str> {
        self.failure
    }
    pub(super) fn issued(&self) -> bool {
        self.attempt.is_some()
    }
    fn fail(&mut self, error: ServiceError) -> Error {
        self.failure = Some(if error == ServiceError::Unsupported {
            "background_service_unsupported"
        } else {
            "service_handoff_unconfirmed"
        });
        Error::ReconciliationRequired
    }
    pub(super) fn close(&mut self, root: &SourceRoot, writer_lock: &File) -> Result<()> {
        root.verify().map_err(|_| Error::Conflict)?;
        if self.root.as_ref().is_some_and(|bound| bound != root) {
            return Err(Error::Conflict);
        }
        if self.attempt.is_none() {
            let client =
                Client::connect(&root.path.join("reader-v1")).map_err(|error| self.fail(error))?;
            let attempt = client
                .attempt(Uuid::new_v4(), Uuid::new_v4(), 30000)
                .map_err(|error| self.fail(error))?;
            self.root = Some(root.clone());
            // Store before the first dispatch, including a lost Begin reply.
            self.attempt = Some(attempt);
        }
        let result = (|| {
            let attempt = self.attempt.as_mut().ok_or(ServiceError::Uncertain)?;
            // Same operation on every retry. Begin is exact idempotent
            // reconciliation; no expired or failed attempt is regenerated.
            require_held(&attempt.begin()?)?;
            require_held(&attempt.revalidate()?)?;
            attempt.shutdown()?;
            Ok::<_, ServiceError>(())
        })();
        if let Err(error) = result {
            return Err(self.fail(error));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if fs2::FileExt::try_lock_exclusive(writer_lock).is_ok() {
                root.verify().map_err(|_| Error::Conflict)?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(self.fail(ServiceError::Uncertain));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    pub(super) fn completed(&mut self) {
        self.attempt = None;
        self.root = None;
        self.failure = None;
    }
}
fn require_held(status: &Status) -> std::result::Result<(), ServiceError> {
    if status.phase == Phase::Held
        && status.remaining_ms > 0
        && status.lease.is_some()
        && status.failure.is_none()
    {
        Ok(())
    } else {
        Err(status.failure.unwrap_or(ServiceError::NotHeld))
    }
}

/// A resumed native workspace has at most its original and candidate owners.
/// Keep each exact source identity and issued attempt before any dispatch.
#[derive(Default)]
pub(super) struct OwnerHandoffs(Vec<(SourceRoot, ShutdownHandoff)>);
impl OwnerHandoffs {
    pub(super) fn acquire(&mut self, path: &Path) -> Result<super::owner::ClosedOwner> {
        let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
        let index = if let Some(index) = self.0.iter().position(|(bound, _)| bound == &root) {
            index
        } else {
            if self.0.len() >= 2 || self.0.iter().any(|(bound, _)| bound.path == root.path) {
                return Err(Error::Conflict);
            }
            self.0.push((root.clone(), Default::default()));
            self.0.len() - 1
        };
        let (bound, handoff) = &mut self.0[index];
        bound.verify().map_err(|_| Error::Conflict)?;
        super::owner::ClosedOwner::acquire_retaining(&root.path, handoff)
    }
    pub(super) fn issued(&self) -> bool {
        self.0.iter().any(|(_, handoff)| handoff.issued())
    }
    pub(super) fn failure_kind(&self) -> Option<&'static str> {
        self.0
            .iter()
            .find_map(|(_, handoff)| handoff.failure_kind())
    }
}
