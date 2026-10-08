//! One server-held operation, no renew/rebind and no release by client Drop.
#[cfg(unix)]
#[path = "backup.rs"]
mod backup;
use super::{
    guards::{Guards, Owners},
    types::MAX_DURATION_MS,
    *,
};
use mdreader_core::SourceRoot;
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use uuid::Uuid;
const MAX_OPERATIONS: usize = 64;

struct Active {
    scope: Scope,
    duration_ms: u32,
    deadline: Instant,
    lease: Uuid,
    phase: Phase,
    guards: Option<Box<Guards>>,
    failure: Option<Error>,
    #[cfg(unix)]
    backup: Option<backup::Record>,
}
struct Completed {
    scope: Scope,
    duration_ms: u32,
    lease: Uuid,
    status: Status,
}
struct State {
    active: Option<Active>,
    completed: Vec<Completed>,
    broken: bool,
}
pub(crate) struct Session {
    root: SourceRoot,
    boot: Uuid,
    owners: Owners,
    state: Mutex<State>,
}
impl Session {
    pub(crate) fn new(
        directory: &std::path::Path,
        store: Arc<Mutex<mdreader_core::ReaderStore>>,
        daemon: Arc<crate::daemon::Coordinator>,
        accounts: Arc<crate::accounts::Coordinator>,
        devices: Arc<crate::device::Coordinator>,
        capture: Arc<crate::capture::admission::Coordinator>,
    ) -> Result<Arc<Self>> {
        if !cfg!(unix) {
            return Err(Error::Unsupported);
        }
        let root = SourceRoot::capture(directory.to_owned()).map_err(|_| Error::StaleOwner)?;
        // Bind the current actual startup owner after capturing this root.
        let _issued = daemon.enter().map_err(|_| Error::Unavailable)?;
        Ok(Arc::new(Self {
            root,
            boot: Uuid::new_v4(),
            owners: Owners {
                store,
                daemon,
                accounts,
                devices,
                capture,
            },
            state: Mutex::new(State {
                active: None,
                completed: Vec::new(),
                broken: false,
            }),
        }))
    }
    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                // Recover only to RETAIN acquired capabilities. Never clear
                // poison, release guards or report proof after uncertain panic.
                let mut state = poisoned.into_inner();
                state.broken = true;
                if let Some(active) = &mut state.active {
                    active.phase = Phase::Quarantined;
                }
                state
            }
        }
    }
    fn selected(&self) -> SelectedRoot {
        SelectedRoot {
            directory: self.root.path.clone(),
            identity: self.root.identity.clone(),
        }
    }
    fn validate_root(&self, expected: &SelectedRoot) -> Result<()> {
        expected.validate()?;
        if expected != &self.selected() {
            return Err(Error::StaleOwner);
        }
        if self.root.verify().is_err() {
            self.lock().broken = true;
            return Err(Error::StaleOwner);
        }
        Ok(())
    }
    fn validate_scope(&self, scope: &Scope) -> Result<()> {
        scope.validate()?;
        if scope.boot != self.boot {
            return Err(Error::StaleOwner);
        }
        self.validate_root(&scope.root)
    }
    pub(crate) fn execute(&self, request: ControlRequest) -> Result<ControlReply> {
        let status = match request {
            ControlRequest::Backup {
                scope,
                lease,
                request,
            } => {
                #[cfg(unix)]
                {
                    return self
                        .backup(scope, lease, *request)
                        .map(|status| ControlReply::Backup { status });
                }
                #[cfg(not(unix))]
                {
                    let _ = (scope, lease, request);
                    return Err(Error::Unsupported);
                }
            }
            ControlRequest::ReconcileBackup {
                scope,
                lease,
                request,
            } => {
                #[cfg(unix)]
                {
                    return self
                        .reconcile_backup(&scope, lease, &request)
                        .map(|status| ControlReply::Backup { status });
                }
                #[cfg(not(unix))]
                {
                    let _ = (scope, lease, request);
                    return Err(Error::Unsupported);
                }
            }
            ControlRequest::Inspect { root } => {
                self.validate_root(&root)?;
                if self.lock().broken {
                    return Err(Error::Quarantined);
                }
                return Ok(ControlReply::Hello { boot: self.boot });
            }
            ControlRequest::Begin { scope, duration_ms } => self.begin(scope, duration_ms)?,
            ControlRequest::Reconcile { scope } => self.status(&scope, None, false)?,
            ControlRequest::Revalidate { scope, lease } => {
                self.status(&scope, Some(lease), false)?
            }
            ControlRequest::Release { scope, lease } => self.status(&scope, Some(lease), true)?,
        };
        Ok(ControlReply::Status { status })
    }
    fn begin(&self, scope: Scope, duration_ms: u32) -> Result<Status> {
        self.validate_scope(&scope)?;
        if duration_ms == 0 || duration_ms > MAX_DURATION_MS {
            return Err(Error::Bounds);
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(duration_ms.into()))
            .ok_or(Error::Bounds)?;
        {
            let mut state = self.lock();
            if state.broken {
                return Err(Error::Quarantined);
            }
            if let Some(completed) = state
                .completed
                .iter()
                .find(|entry| entry.scope.operation == scope.operation)
            {
                if completed.scope != scope || completed.duration_ms != duration_ms {
                    return Err(Error::StaleOwner);
                }
                return Ok(completed.status.clone());
            }
            if let Some(active) = &state.active {
                if active.scope != scope {
                    return Err(Error::Busy);
                }
                if active.duration_ms != duration_ms {
                    return Err(Error::StaleOwner);
                }
                drop(state);
                return self.status(&scope, None, false);
            }
            // No eviction may let an old uncertain Begin mint a new lease.
            // Bound this process's receipts; an exhausted idle owner requires
            // explicit restart, never an automatic client restart/rebind.
            if state.completed.len() >= MAX_OPERATIONS {
                return Err(Error::Bounds);
            }
            state.active = Some(Active {
                scope: scope.clone(),
                duration_ms,
                deadline,
                lease: Uuid::new_v4(),
                phase: Phase::Acquiring,
                guards: None,
                failure: None,
                #[cfg(unix)]
                backup: None,
            });
        }
        // NO control/ReaderStore lock spans native drain, account/vault await,
        // device verification or cross-process spool lock acquisition.
        let acquired = self.owners.acquire(&scope, deadline);
        let mut state = self.lock();
        let broken = state.broken;
        let active = state.active.as_mut().ok_or(Error::Quarantined)?;
        match acquired {
            Ok(guards) => {
                active.guards = Some(guards);
                if broken || active.phase == Phase::Quarantined || Instant::now() >= deadline {
                    active.phase = Phase::Quarantined;
                    active.failure = Some(Error::Deadline);
                } else {
                    active.phase = Phase::Held;
                }
            }
            Err((error, Some(guards))) => {
                // Partial failure never drops already-issued exclusive guards.
                active.guards = Some(guards);
                active.phase = Phase::Quarantined;
                active.failure = Some(error);
            }
            Err((error, None)) if !broken && active.phase != Phase::Quarantined => {
                let active = state.active.take().ok_or(Error::Quarantined)?;
                let status = Status {
                    phase: Phase::Failed,
                    remaining_ms: 0,
                    lease: None,
                    failure: Some(error),
                };
                state.completed.push(Completed {
                    scope: active.scope,
                    duration_ms,
                    lease: active.lease,
                    status: status.clone(),
                });
                return Ok(status);
            }
            Err((error, None)) => {
                active.phase = Phase::Quarantined;
                active.failure = Some(error);
            }
        }
        drop(state);
        self.status(&scope, None, false)
    }
    fn status(&self, scope: &Scope, lease: Option<Uuid>, release: bool) -> Result<Status> {
        self.validate_scope(scope)?;
        let mut state = self.lock();
        if state.broken {
            return Err(Error::Quarantined);
        }
        if let Some(done) = state
            .completed
            .iter()
            .find(|entry| entry.scope.operation == scope.operation)
        {
            if &done.scope != scope {
                return Err(Error::StaleOwner);
            }
            if lease.is_some_and(|lease| lease != done.lease) {
                return Err(Error::NotHeld);
            }
            return Ok(done.status.clone());
        }
        let active = state.active.as_mut().ok_or(Error::UnknownOperation)?;
        if &active.scope != scope {
            return Err(Error::StaleOwner);
        }
        if lease.is_some_and(|lease| lease != active.lease) {
            return Err(Error::NotHeld);
        }
        if Instant::now() >= active.deadline {
            active.phase = Phase::Quarantined;
            active.failure = Some(Error::Deadline);
        }
        if active.phase == Phase::Held {
            if let Err(error) = active
                .guards
                .as_ref()
                .ok_or(Error::NotHeld)?
                .verify(&self.owners, scope)
            {
                active.phase = Phase::Quarantined;
                active.failure = Some(error);
            }
        }
        if release && active.phase == Phase::BackingUp {
            return Err(Error::Busy);
        }
        if release && (active.phase != Phase::Held || lease.is_none()) {
            return Err(Error::Quarantined);
        }
        if release {
            let active = state.active.take().ok_or(Error::Quarantined)?;
            // Only exact verified explicit release resumes. Losing a reply,
            // client Drop, expiry or revalidation failure cannot take this path.
            let mut active = active;
            let Some(guards) = active.guards.take() else {
                active.phase = Phase::Quarantined;
                active.failure = Some(Error::Quarantined);
                state.active = Some(active);
                return Err(Error::Quarantined);
            };
            if let Err(guards) = guards.release(&self.owners, scope) {
                active.guards = Some(guards);
                active.phase = Phase::Quarantined;
                active.failure = Some(Error::Quarantined);
                state.active = Some(active);
                return Err(Error::Quarantined);
            }
            let status = Status {
                phase: Phase::Released,
                remaining_ms: 0,
                lease: None,
                failure: None,
            };
            state.completed.push(Completed {
                scope: active.scope,
                duration_ms: active.duration_ms,
                lease: active.lease,
                status: status.clone(),
            });
            return Ok(status);
        }
        Ok(Status {
            phase: active.phase,
            remaining_ms: active
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u32::MAX.into()) as u32,
            lease: (active.phase == Phase::Held).then_some(active.lease),
            failure: active.failure,
        })
    }
}
