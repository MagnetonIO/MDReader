//! Actual issued effects, including their descendants, not RPC acknowledgements.
use super::{Error, Result, StartupOwner};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};
use uuid::Uuid;

const MAX_EFFECTS: u32 = 4096;
#[derive(Clone, Copy, PartialEq, Eq)]
struct Tag {
    token: Uuid,
    operation: Uuid,
    owner_epoch: Uuid,
}
struct State {
    active: u32,
    denied: Option<Tag>,
    epoch: Uuid,
    broken: bool,
    stopped: bool,
}
pub(crate) struct Admission {
    startup: Option<Arc<StartupOwner>>,
    state: Mutex<State>,
    settled: Condvar,
}
pub(crate) struct EffectLease {
    admission: Arc<Admission>,
    epoch: Uuid,
}
pub(super) struct Denied {
    admission: Arc<Admission>,
    tag: Tag,
    deadline: Instant,
}

impl Admission {
    pub(crate) fn new(startup: Option<Arc<StartupOwner>>) -> Arc<Self> {
        Arc::new(Self {
            startup,
            state: Mutex::new(State {
                active: 0,
                denied: None,
                epoch: Uuid::new_v4(),
                broken: false,
                stopped: false,
            }),
            settled: Condvar::new(),
        })
    }
    /// The worker sleeps until real resume/shutdown, never polling while frozen.
    pub(crate) fn wait_for_admission(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        while !state.broken
            && !state.stopped
            && (state.denied.is_some() || state.active >= MAX_EFFECTS)
        {
            let Ok(next) = self.settled.wait(state) else {
                return false;
            };
            state = next;
        }
        !state.broken && !state.stopped
    }
    pub(crate) fn stop_worker(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stopped = true;
            state.broken = true;
        }
        self.settled.notify_all();
    }
    pub(crate) fn bound_to(&self, startup: &Arc<StartupOwner>) -> bool {
        self.startup
            .as_ref()
            .is_some_and(|actual| Arc::ptr_eq(actual, startup))
    }
    pub(crate) fn enter(self: &Arc<Self>) -> Result<EffectLease> {
        if let Some(startup) = &self.startup {
            startup.verify().inspect_err(|_| self.quarantine())?;
        }
        let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
        if state.broken {
            return Err(Error::Unavailable);
        }
        if state.denied.is_some() || state.active >= MAX_EFFECTS {
            return Err(Error::Busy);
        }
        state.active += 1;
        Ok(EffectLease {
            admission: self.clone(),
            epoch: state.epoch,
        })
    }
    pub(crate) fn fork(self: &Arc<Self>, parent: &EffectLease) -> Result<EffectLease> {
        if !Arc::ptr_eq(self, &parent.admission) {
            return Err(Error::Conflict);
        }
        let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
        if state.broken {
            return Err(Error::Unavailable);
        }
        if state.active == 0 || state.active >= MAX_EFFECTS {
            return Err(Error::Busy);
        }
        // Denial stops NEW effects. Children of an actual admitted effect must
        // still be carried into queued/blocking work while freeze is draining.
        state.active += 1;
        Ok(EffectLease {
            admission: self.clone(),
            epoch: parent.epoch,
        })
    }
    pub(super) fn deny(
        self: &Arc<Self>,
        operation: Uuid,
        owner_epoch: Uuid,
        deadline: Instant,
    ) -> Result<Denied> {
        let tag = Tag {
            token: Uuid::new_v4(),
            operation,
            owner_epoch,
        };
        let mut state = self.state.lock().map_err(|_| Error::Unavailable)?;
        if state.broken {
            return Err(Error::Unavailable);
        }
        if state.denied.is_some() {
            return Err(Error::Busy);
        }
        state.denied = Some(tag);
        let denied = Denied {
            admission: self.clone(),
            tag,
            deadline,
        };
        while state.active != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                drop(state);
                return Err(Error::Deadline);
            }
            state = self
                .settled
                .wait_timeout(state, remaining)
                .map_err(|_| Error::Unavailable)?
                .0;
        }
        drop(state);
        denied.verify(operation, owner_epoch)?;
        Ok(denied)
    }
    pub(crate) fn quarantine(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.broken = true;
        }
        self.settled.notify_all();
    }
}
impl EffectLease {
    pub(crate) fn fork(&self) -> Result<Self> {
        self.admission.fork(self)
    }
    pub(crate) fn epoch(&self) -> Uuid {
        self.epoch
    }
}
impl Drop for EffectLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.admission.state.lock() {
            if let Some(active) = state.active.checked_sub(1) {
                state.active = active;
            } else {
                state.broken = true;
            }
        }
        self.admission.settled.notify_all();
    }
}
impl Denied {
    pub(super) fn verify(&self, operation: Uuid, owner_epoch: Uuid) -> Result<()> {
        if operation != self.tag.operation || owner_epoch != self.tag.owner_epoch {
            return Err(Error::Conflict);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Deadline);
        }
        let state = self
            .admission
            .state
            .lock()
            .map_err(|_| Error::Unavailable)?;
        if state.broken || state.denied != Some(self.tag) || state.active != 0 {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
}
impl Drop for Denied {
    fn drop(&mut self) {
        if let Ok(mut state) = self.admission.state.lock() {
            if !state.broken && state.denied == Some(self.tag) {
                state.denied = None;
                // Buffered tickets/vectors from the old scheduling epoch are
                // disposable; durable pending index jobs remain untouched.
                state.epoch = Uuid::new_v4();
            }
        }
        self.admission.settled.notify_all();
    }
}
