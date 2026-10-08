//! Actual service-effect drain plus the hook processes' exclusive spool lock.
//! No renderer request, selected-profile discovery, or backup receipt is exposed.
mod namespace;
mod state;
#[cfg(all(test, unix))]
mod tests;
use mdreader_capture::{CaptureError, FrozenSpool, Limits, Spool};
#[cfg(unix)]
use mdreader_core::{LibraryIdentity, ReaderError};
use mdreader_core::{ReaderStore, SourceRoot};
pub use state::EffectLease;
#[cfg(unix)]
use std::fs::File;
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
    Unsupported,
    Unavailable,
}
pub type Result<T> = std::result::Result<T, Error>;

/// Construct in the selected service process with its actual writer Arc. The
/// same Arc must remain its sole ReaderStore owner; owner replacement/reopening
/// requires a new coordinator, not a pathname or epoch reset.
pub struct Coordinator {
    owner: Arc<Mutex<ReaderStore>>,
    root: SourceRoot,
    #[cfg(unix)]
    library: LibraryIdentity,
    namespace: namespace::Namespace,
    shared: Arc<state::Shared>,
}
impl Coordinator {
    pub fn new(owner: Arc<Mutex<ReaderStore>>, directory: &Path) -> Result<Self> {
        let root = SourceRoot::capture(directory.to_path_buf()).map_err(|_| Error::Conflict)?;
        // Ask the actual opened owner for its original handle-bound proof.
        // Never adopt a replacement merely because its pathname is unchanged.
        #[cfg(unix)]
        let library = {
            let store = owner.try_lock().map_err(|_| Error::Busy)?;
            store.library_identity().map_err(|error| match error {
                ReaderError::Conflict => Error::Conflict,
                _ => Error::Unavailable,
            })?
        };
        #[cfg(unix)]
        library.verify_at(&root.path).map_err(|_| Error::Conflict)?;
        // Ordinary non-Unix service startup keeps its prior filename association;
        // it never issues a preservation certificate (freeze is Unsupported).
        #[cfg(not(unix))]
        {
            let mut store = owner.try_lock().map_err(|_| Error::Busy)?;
            let actual: String = store
                .with_library_connection(|connection| {
                    connection.query_row(
                        "SELECT file FROM pragma_database_list WHERE name='main'",
                        [],
                        |row| row.get(0),
                    )
                })
                .map_err(|_| Error::Unavailable)?;
            if Path::new(&actual)
                .canonicalize()
                .map_err(|_| Error::Conflict)?
                != root
                    .path
                    .join("library.sqlite")
                    .canonicalize()
                    .map_err(|_| Error::Conflict)?
            {
                return Err(Error::Conflict);
            }
        }
        let namespace = namespace::Namespace::capture(&root)?;
        let coordinator = Self {
            owner,
            root,
            #[cfg(unix)]
            library,
            namespace,
            shared: Arc::new(state::Shared {
                state: Mutex::new(state::State {
                    active: 0,
                    frozen: None,
                    broken: false,
                    quarantine: None,
                }),
                changed: std::sync::Condvar::new(),
            }),
        };
        coordinator.verify_root()?;
        Ok(coordinator)
    }
    pub fn enter(&self) -> Result<EffectLease> {
        if self.shared.lock()?.broken {
            return Err(Error::Unavailable);
        }
        self.verify_root()
            .inspect_err(|_| self.shared.quarantine(None))?;
        let mut state = self.shared.lock()?;
        if state.broken {
            return Err(Error::Unavailable);
        }
        if state.frozen.is_some() || state.active >= 1024 {
            return Err(Error::Busy);
        }
        state.active += 1;
        Ok(EffectLease {
            shared: self.shared.clone(),
        })
    }
    fn verify_root(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        self.namespace.verify()?;
        #[cfg(not(unix))]
        self.root
            .open_optional_file_relative(Path::new("library.sqlite"))
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        #[cfg(unix)]
        self.library
            .verify_at(&self.root.path)
            .map_err(|_| Error::Conflict)?;
        Ok(())
    }
    /// Blocking bounded wait; call on a service preservation worker, never an
    /// async executor or while holding the reader owner. No pending output is
    /// projected/discarded and no policy/session mutation is performed here.
    pub fn freeze(
        self: &Arc<Self>,
        operation: Uuid,
        owner_epoch: Uuid,
        deadline: Instant,
    ) -> Result<FrozenCapture> {
        if !cfg!(unix) {
            return Err(Error::Unsupported);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if operation.get_version_num() != 4
            || owner_epoch.get_version_num() != 4
            || remaining.is_zero()
            || remaining > Duration::from_secs(30)
        {
            return Err(Error::Deadline);
        }
        self.verify_root()?;
        let tag = state::Tag {
            token: Uuid::new_v4(),
            operation,
            owner_epoch,
        };
        {
            let mut state = self.shared.lock()?;
            if state.broken {
                return Err(Error::Unavailable);
            }
            if state.frozen.is_some() {
                return Err(Error::Busy);
            }
            state.frozen = Some(tag);
        }
        let mut guard = FrozenCapture {
            coordinator: self.clone(),
            tag,
            deadline,
            spool: None,
        };
        {
            let mut state = self.shared.lock()?;
            while state.active != 0 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(Error::Deadline);
                }
                state = self
                    .shared
                    .changed
                    .wait_timeout(state, remaining)
                    .map_err(|_| Error::Unavailable)?
                    .0;
                if state.broken {
                    return Err(Error::Unavailable);
                }
            }
        }
        // Existing hook CLI retries only Busy within one second; freeze shares
        // that same actual queue lock. Initialization settles before proof.
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Deadline);
            }
            match Spool::open(self.root.path.join("captures"), Limits::default())
                .and_then(Spool::freeze)
            {
                Ok(spool) => {
                    guard.spool = Some(spool);
                    break;
                }
                Err(CaptureError::Busy) => std::thread::sleep(Duration::from_millis(5)),
                Err(CaptureError::UnsupportedPlatform) => return Err(Error::Unsupported),
                Err(CaptureError::UnsafePath) => {
                    self.shared.quarantine(None);
                    return Err(Error::Conflict);
                }
                Err(_) => return Err(Error::Unavailable),
            }
        }
        guard.revalidate(self, &self.owner, operation, owner_epoch)?;
        Ok(guard)
    }
}
/// Retain on the server through preservation. No writer connection, secret,
/// serialization, or transferable remote readiness acknowledgement is provided.
pub struct FrozenCapture {
    coordinator: Arc<Coordinator>,
    tag: state::Tag,
    deadline: Instant,
    spool: Option<FrozenSpool>,
}
impl FrozenCapture {
    pub fn revalidate(
        &self,
        coordinator: &Coordinator,
        owner: &Arc<Mutex<ReaderStore>>,
        operation: Uuid,
        owner_epoch: Uuid,
    ) -> Result<()> {
        if !Arc::ptr_eq(&self.coordinator.shared, &coordinator.shared)
            || !Arc::ptr_eq(&coordinator.owner, owner)
            || operation != self.tag.operation
            || owner_epoch != self.tag.owner_epoch
        {
            return Err(Error::Conflict);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Deadline);
        }
        self.verify_release()?;
        self.spool
            .as_ref()
            .ok_or(Error::Unavailable)?
            .revalidate()
            .map_err(|_| Error::Conflict)
    }
    // Deadline expiry by itself is not changed ownership: valid preservation
    // failure can resume. Namespace/owner proof failure must remain denied.
    fn verify_release(&self) -> Result<()> {
        self.coordinator.verify_root()?;
        let state = self.coordinator.shared.lock()?;
        if state.broken
            || state.frozen != Some(self.tag)
            || (self.spool.is_some() && state.active != 0)
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
impl Drop for FrozenCapture {
    fn drop(&mut self) {
        if self.verify_release().is_err()
            || self
                .spool
                .as_mut()
                .is_some_and(|spool| spool.release().is_err())
        {
            self.coordinator.shared.quarantine(self.spool.take());
            return;
        }
        let resumed = match self.coordinator.shared.lock() {
            Ok(mut state) if state.frozen == Some(self.tag) && !state.broken => {
                state.frozen = None;
                self.coordinator.shared.changed.notify_all();
                true
            }
            _ => false,
        };
        if !resumed {
            self.coordinator.shared.quarantine(self.spool.take());
        }
    }
}
