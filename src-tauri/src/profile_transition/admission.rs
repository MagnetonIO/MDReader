//! Shared admission for real native writers. An issued lease stays alive until
//! its effect (including blocking work or an uncertain remote receipt) finishes.
use mdreader_profile::{Error, Result};
use std::{
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_EFFECTS: usize = 1024;
pub(super) const MAX_FREEZE_WAIT: Duration = Duration::from_secs(30);

enum Phase {
    Running,
    Denied(Uuid),
}
struct State {
    phase: Phase,
    active: usize,
    broken: bool,
    sealed: bool,
}
pub(super) struct Admission {
    epoch: Uuid,
    state: Mutex<State>,
    changed: Condvar,
    transition: Mutex<()>,
}
impl Admission {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Uuid::new_v4(),
            state: Mutex::new(State {
                phase: Phase::Running,
                active: 0,
                broken: false,
                sealed: false,
            }),
            changed: Condvar::new(),
            transition: Mutex::new(()),
        })
    }
    pub(super) fn epoch(&self) -> Uuid {
        self.epoch
    }
    pub(super) fn registration_open(&self) -> Result<()> {
        let state = self.state.lock().map_err(|_| Error::NotQuiescent)?;
        if state.sealed || state.broken || !matches!(state.phase, Phase::Running) {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub(super) fn enter(self: &Arc<Self>) -> Result<EffectLease> {
        let mut state = self.state.lock().map_err(|_| Error::NotQuiescent)?;
        if state.broken || !matches!(state.phase, Phase::Running) {
            return Err(Error::NotQuiescent);
        }
        if state.active >= MAX_EFFECTS {
            return Err(Error::Bounds);
        }
        state.sealed = true;
        state.active += 1;
        Ok(EffectLease {
            admission: self.clone(),
        })
    }
    pub(super) fn deny_and_drain(
        &self,
        operation: Uuid,
        deadline: Instant,
    ) -> Result<DeniedLease<'_>> {
        let serial = self
            .transition
            .try_lock()
            .map_err(|_| Error::NotQuiescent)?;
        {
            let mut state = self.state.lock().map_err(|_| Error::NotQuiescent)?;
            if state.broken || !matches!(state.phase, Phase::Running) {
                return Err(Error::NotQuiescent);
            }
            state.sealed = true;
            state.phase = Phase::Denied(operation);
        }
        let lease = DeniedLease {
            admission: self,
            operation,
            _serial: serial,
        };
        // No participant/DB mutex is held while an issued effect finishes.
        self.wait_for_idle(deadline)?;
        Ok(lease)
    }
    fn wait_for_idle(&self, deadline: Instant) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| Error::NotQuiescent)?;
        while state.active != 0 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(Error::NotQuiescent)?;
            let (next, result) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| Error::NotQuiescent)?;
            state = next;
            if result.timed_out() && state.active != 0 {
                return Err(Error::NotQuiescent);
            }
        }
        if state.broken || Instant::now() >= deadline {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
}

/// Neither cloneable nor constructible outside admission. Drop is completion,
/// not cancellation: native callers must move this into their actual worker.
pub(crate) struct EffectLease {
    admission: Arc<Admission>,
}
impl Drop for EffectLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.admission.state.lock() {
            if state.active == 0 {
                state.broken = true;
            } else {
                state.active -= 1;
            }
            self.admission.changed.notify_all();
        }
    }
}

pub(super) struct DeniedLease<'a> {
    admission: &'a Admission,
    operation: Uuid,
    _serial: MutexGuard<'a, ()>,
}
impl DeniedLease<'_> {
    /// Explicit preservation-only resume. Retain the transition mutex until
    /// operation-bound notifications finish so another freeze cannot overtake.
    pub(super) fn resume(&self) -> Result<()> {
        let mut state = self
            .admission
            .state
            .lock()
            .map_err(|_| Error::NotQuiescent)?;
        if state.broken
            || state.active != 0
            || !matches!(state.phase, Phase::Denied(id) if id == self.operation)
        {
            return Err(Error::NotQuiescent);
        }
        state.phase = Phase::Running;
        self.admission.changed.notify_all();
        Ok(())
    }
    /// Invalid live authority requires a new explicitly selected owner/registry;
    /// Drop must not silently resume effects into a replaced root/session.
    pub(super) fn quarantine(&self) {
        if let Ok(mut state) = self.admission.state.lock() {
            state.broken = true;
        }
    }
    pub(super) fn revalidate(&self) -> Result<()> {
        let state = self
            .admission
            .state
            .lock()
            .map_err(|_| Error::NotQuiescent)?;
        if state.broken
            || state.active != 0
            || !matches!(state.phase, Phase::Denied(id) if id == self.operation)
        {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
}
impl Drop for DeniedLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.admission.state.lock() {
            if matches!(state.phase, Phase::Denied(id) if id == self.operation) && !state.broken {
                state.phase = Phase::Running;
                self.admission.changed.notify_all();
            }
        }
    }
}
