//! Native preservation participant only. This is not a backup-ready proof.
use super::*;
use mdreader_core::SourceRoot;
use std::{path::Path, sync::Condvar, time::Duration};
use uuid::Uuid;

#[derive(Default)]
struct State {
    frozen: Option<Uuid>,
    active: u32,
}
#[derive(Default)]
pub(super) struct Admission {
    state: Mutex<State>,
    settled: Condvar,
}
pub(super) struct Active {
    admission: Arc<Admission>,
}
impl Admission {
    pub(super) fn begin(self: &Arc<Self>) -> Result<Active> {
        let mut state = self.state.lock().map_err(|_| DeviceError::Persistence)?;
        if state.frozen.is_some() {
            return Err(DeviceError::Busy);
        }
        state.active = state.active.checked_add(1).ok_or(DeviceError::Busy)?;
        Ok(Active {
            admission: self.clone(),
        })
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        if let Ok(mut state) = self.admission.state.lock() {
            state.active = state.active.saturating_sub(1);
            self.admission.settled.notify_all();
        }
    }
}
enum Root {
    Absent,
    Existing(SourceRoot),
}
impl Root {
    fn observe(path: &Path) -> Result<Self> {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Absent),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let path = path.canonicalize().map_err(|_| DeviceError::Unavailable)?;
                SourceRoot::capture(path)
                    .map(Self::Existing)
                    .map_err(|_| DeviceError::Unavailable)
            }
            Ok(_) => Err(DeviceError::Unavailable),
            Err(_) => Err(DeviceError::Unavailable),
        }
    }
    fn verify(&self, path: &Path) -> Result<()> {
        match self {
            Self::Absent if matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound) => {
                Ok(())
            }
            Self::Existing(root) if path.canonicalize().is_ok_and(|path| root.path == path) => {
                root.verify().map_err(|_| DeviceError::Unavailable)
            }
            _ => Err(DeviceError::Stale),
        }
    }
}
/// Keeps native admission denied until dropped. Other shared-runtime producers,
/// database writers and custody participants must be frozen independently.
pub struct FrozenDevices {
    coordinator: Arc<Coordinator>,
    token: Uuid,
    operation: Uuid,
    epoch: Uuid,
    deadline: Instant,
    documents: Root,
    assets: Root,
    snapshot: Option<catalog::Snapshot>,
}
impl FrozenDevices {
    pub fn revalidate(
        &self,
        coordinator: &Coordinator,
        operation: Uuid,
        epoch: Uuid,
    ) -> Result<()> {
        if Instant::now() >= self.deadline
            || self.operation != operation
            || self.epoch != epoch
            || !Arc::ptr_eq(&self.coordinator.owner, &coordinator.owner)
            || !Arc::ptr_eq(&self.coordinator.queries, &coordinator.queries)
            || !Arc::ptr_eq(&self.coordinator.admission, &coordinator.admission)
            || !self.coordinator.runtime.same_instance(&coordinator.runtime)
        {
            return Err(DeviceError::Stale);
        }
        let state = coordinator
            .admission
            .state
            .lock()
            .map_err(|_| DeviceError::Persistence)?;
        if state.frozen != Some(self.token) || state.active != 0 {
            return Err(DeviceError::Stale);
        }
        drop(state);
        self.documents.verify(&coordinator.documents)?;
        self.assets.verify(&coordinator.assets)?;
        {
            let mut owner = coordinator.owner()?;
            coordinator
                .runtime
                .existing_assets(&mut owner, &coordinator.assets)
                .map_err(asset_error)?;
        }
        let query = coordinator
            .queries
            .try_lock()
            .map_err(|_| DeviceError::Busy)?;
        let current = catalog::page_in(
            &query,
            &coordinator.documents,
            None,
            self.snapshot.as_ref(),
            1,
        )
        .map_err(document_error)?;
        if current.snapshot != self.snapshot {
            return Err(DeviceError::Stale);
        }
        Ok(())
    }
}
impl Drop for FrozenDevices {
    fn drop(&mut self) {
        if let Ok(mut state) = self.coordinator.admission.state.lock() {
            if state.frozen == Some(self.token) {
                state.frozen = None;
            }
        }
    }
}
impl Coordinator {
    /// Call on a native blocking worker, never the UI/async executor. Bounded
    /// drain includes verification outside SQLite locks; timeout yields no guard.
    pub fn freeze(
        self: &Arc<Self>,
        operation: Uuid,
        epoch: Uuid,
        deadline: Instant,
    ) -> Result<FrozenDevices> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if operation.get_version_num() != 4
            || epoch.get_version_num() != 4
            || remaining.is_zero()
            || remaining > Duration::from_secs(30)
        {
            return Err(DeviceError::Invalid);
        }
        let token = Uuid::new_v4();
        let mut state = self
            .admission
            .state
            .lock()
            .map_err(|_| DeviceError::Persistence)?;
        if state.frozen.is_some() {
            return Err(DeviceError::Busy);
        }
        state.frozen = Some(token);
        let mut guard = FrozenDevices {
            coordinator: self.clone(),
            token,
            operation,
            epoch,
            deadline,
            documents: Root::Absent,
            assets: Root::Absent,
            snapshot: None,
        };
        while state.active != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                drop(state);
                return Err(DeviceError::Busy);
            }
            state = self
                .admission
                .settled
                .wait_timeout(state, remaining)
                .map_err(|_| DeviceError::Persistence)?
                .0;
        }
        drop(state);
        // Only leases issued by this coordinator. Incoming/archive leases using
        // the same runtime remain the responsibility of their own participant.
        for id in self
            .leases
            .lock()
            .map_err(|_| DeviceError::Persistence)?
            .drain()
        {
            self.runtime.close(&id).map_err(asset_error)?;
        }
        // Retained attachment authority must match its durable identity; a
        // replacement directory is not adopted as a preservation root.
        {
            let mut owner = self.owner()?;
            self.runtime
                .existing_assets(&mut owner, &self.assets)
                .map_err(asset_error)?;
        }
        guard.documents = Root::observe(&self.documents)?;
        guard.assets = Root::observe(&self.assets)?;
        {
            let query = self.queries.try_lock().map_err(|_| DeviceError::Busy)?;
            guard.snapshot = catalog::page_in(&query, &self.documents, None, None, 1)
                .map_err(document_error)?
                .snapshot;
        }
        guard.revalidate(self, operation, epoch)?;
        Ok(guard)
    }
}
