//! Real service dispatch/model/provider drain. This is not all-owner readiness.
//!
//! Asynchronous account/device/custody effects outliving synchronous dispatch
//! require their separate live-owner freeze capabilities. This gate never
//! substitutes for FrozenAccounts or claims a whole-profile backup barrier.
mod admission;
mod startup;
pub(crate) use admission::{Admission, EffectLease};
use mdreader_core::{ReaderStore, StoreIdentity};
pub(crate) use startup::StartupOwner;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Busy,
    Deadline,
    Conflict,
    Unavailable,
    Unsupported,
}
pub type Result<T> = std::result::Result<T, Error>;

pub struct Coordinator {
    admission: Arc<Admission>,
    startup: Arc<StartupOwner>,
    owner: Arc<Mutex<ReaderStore>>,
    queries: Arc<Mutex<ReaderStore>>,
    writer_identity: StoreIdentity,
    query_identity: StoreIdentity,
}
pub struct FrozenReaderDaemon {
    coordinator: Arc<Coordinator>,
    denied: admission::Denied,
    operation: Uuid,
    owner_epoch: Uuid,
    deadline: Instant,
}
impl Coordinator {
    pub(crate) fn new(
        admission: Arc<Admission>,
        startup: Arc<StartupOwner>,
        owner: Arc<Mutex<ReaderStore>>,
        queries: Arc<Mutex<ReaderStore>>,
    ) -> Result<Arc<Self>> {
        if !cfg!(unix) {
            return Err(Error::Unsupported);
        }
        if !admission.bound_to(&startup) {
            return Err(Error::Conflict);
        }
        let writer_identity = owner
            .try_lock()
            .map_err(|_| Error::Busy)?
            .store_identity()
            .map_err(|_| Error::Conflict)?;
        let query_identity = queries
            .try_lock()
            .map_err(|_| Error::Busy)?
            .store_identity()
            .map_err(|_| Error::Conflict)?;
        let coordinator = Arc::new(Self {
            admission,
            startup,
            owner,
            queries,
            writer_identity,
            query_identity,
        });
        coordinator.verify_names()?;
        Ok(coordinator)
    }
    pub(crate) fn enter(&self) -> Result<EffectLease> {
        self.verify_names()
            .inspect_err(|_| self.admission.quarantine())?;
        self.admission.enter()
    }
    fn verify_names(&self) -> Result<()> {
        self.startup.verify()?;
        let directory = self.startup.directory();
        self.writer_identity
            .verify_at(directory)
            .map_err(|_| Error::Conflict)?;
        self.query_identity
            .verify_at(directory)
            .map_err(|_| Error::Conflict)
    }
    fn verify_handles(&self) -> Result<()> {
        self.verify_names()?;
        for store in [&self.owner, &self.queries] {
            let store = store.try_lock().map_err(|_| Error::Busy)?;
            store
                .store_identity()
                .map_err(|_| Error::Conflict)?
                .verify_at(self.startup.directory())
                .map_err(|_| Error::Conflict)?;
        }
        self.verify_names()
    }
    pub fn freeze(
        self: &Arc<Self>,
        operation: Uuid,
        owner_epoch: Uuid,
        deadline: Instant,
    ) -> Result<FrozenReaderDaemon> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if operation.get_version_num() != 4
            || owner_epoch.get_version_num() != 4
            || remaining.is_zero()
            || remaining > Duration::from_secs(30)
        {
            return Err(Error::Deadline);
        }
        self.verify_names()
            .inspect_err(|_| self.admission.quarantine())?;
        let denied = self.admission.deny(operation, owner_epoch, deadline)?;
        // A separate registered owner may briefly hold SQLite. Never wait on
        // its mutex: bounded retries yield without retaining any owner borrow.
        loop {
            match self.verify_handles() {
                Ok(()) => break,
                Err(Error::Busy) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(error) => {
                    if error != Error::Busy {
                        self.admission.quarantine();
                    }
                    return Err(if error == Error::Busy {
                        Error::Deadline
                    } else {
                        error
                    });
                }
            }
        }
        denied.verify(operation, owner_epoch)?;
        Ok(FrozenReaderDaemon {
            coordinator: self.clone(),
            denied,
            operation,
            owner_epoch,
            deadline,
        })
    }
}
impl FrozenReaderDaemon {
    pub fn revalidate(
        &self,
        coordinator: &Arc<Coordinator>,
        operation: Uuid,
        owner_epoch: Uuid,
    ) -> Result<()> {
        if !Arc::ptr_eq(&self.coordinator, coordinator)
            || self.operation != operation
            || self.owner_epoch != owner_epoch
        {
            return Err(Error::Conflict);
        }
        self.denied.verify(operation, owner_epoch)?;
        self.coordinator.verify_handles()
    }
    /// Only for an all-participant server-side snapshot adapter. This scoped
    /// borrow does not freeze account/custody producers and exposes only the
    /// actual readonly query connection.
    pub fn with_library<T>(
        &self,
        action: impl FnOnce(&rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        self.denied.verify(self.operation, self.owner_epoch)?;
        self.coordinator.verify_handles()?;
        let mut owner = self
            .coordinator
            .queries
            .try_lock()
            .map_err(|_| Error::Busy)?;
        let result = owner.with_library_connection(|connection| action(&*connection));
        owner.store_identity().map_err(|_| Error::Conflict)?;
        drop(owner);
        self.revalidate(&self.coordinator, self.operation, self.owner_epoch)?;
        result
    }
    pub fn directory(&self) -> &Path {
        self.coordinator.startup.directory()
    }
}
impl Drop for FrozenReaderDaemon {
    fn drop(&mut self) {
        // Never reopen admission on changed authority, including uncertain
        // teardown. The retained actual process lease remains with Runtime.
        if Instant::now() >= self.deadline || self.coordinator.verify_handles().is_err() {
            self.coordinator.admission.quarantine();
        }
    }
}
