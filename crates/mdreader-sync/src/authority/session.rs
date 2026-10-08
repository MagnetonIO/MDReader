use super::*;
use crate::SyncScope;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, RwLock, Weak,
};

pub(super) struct Lease {
    pub(super) epoch: u64,
    pub(super) token: uuid::Uuid,
    pub(super) scope: SyncScope,
    pub(super) valid: AtomicBool,
}
pub(super) struct State {
    pub(super) epoch: u64,
    active: Option<Weak<Lease>>,
}
/// Native owner shares this exact binding barrier with TransportOwner. No renderer handles it.
pub struct SessionController {
    pub(super) binding: Arc<RwLock<Option<SyncScope>>>,
    pub(super) state: Mutex<State>,
    pub(super) changes: tokio::sync::watch::Sender<u64>,
}
impl std::fmt::Debug for SessionController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("SessionController", f)
    }
}
impl SessionController {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            binding: Arc::new(RwLock::new(None)),
            state: Mutex::new(State {
                epoch: 0,
                active: None,
            }),
            changes: tokio::sync::watch::channel(0).0,
        })
    }
    pub fn active_scope_barrier(&self) -> Arc<RwLock<Option<SyncScope>>> {
        self.binding.clone()
    }
    pub(super) fn epoch(&self) -> AuthorityResult<u64> {
        Ok(self
            .state
            .lock()
            .map_err(|_| AuthorityError::Inactive)?
            .epoch)
    }
    pub(super) fn check_epoch(&self, epoch: u64) -> AuthorityResult<()> {
        if self.epoch()? != epoch {
            Err(AuthorityError::Inactive)
        } else {
            Ok(())
        }
    }
    /// Local sign-out retains offline keys/data and never implies remote key erasure.
    pub fn logout(&self) -> AuthorityResult<()> {
        let mut binding = self.binding.write().map_err(|_| AuthorityError::Inactive)?;
        let mut state = self.state.lock().map_err(|_| AuthorityError::Inactive)?;
        if let Some(old) = state.active.take().and_then(|w| w.upgrade()) {
            old.valid.store(false, Ordering::Release);
        }
        *binding = None;
        state.epoch = state.epoch.checked_add(1).ok_or(AuthorityError::Inactive)?;
        self.changes.send_replace(state.epoch);
        Ok(())
    }
    pub(super) fn check(&self, lease: &Lease) -> AuthorityResult<()> {
        let binding = self.binding.read().map_err(|_| AuthorityError::Inactive)?;
        let state = self.state.lock().map_err(|_| AuthorityError::Inactive)?;
        if !lease.valid.load(Ordering::Acquire)
            || state.epoch != lease.epoch
            || binding.as_ref() != Some(&lease.scope)
        {
            return Err(AuthorityError::Inactive);
        }
        Ok(())
    }
    pub(super) fn activate(&self, expected: u64, scope: SyncScope) -> AuthorityResult<Arc<Lease>> {
        let mut binding = self.binding.write().map_err(|_| AuthorityError::Inactive)?;
        let mut state = self.state.lock().map_err(|_| AuthorityError::Inactive)?;
        if state.epoch != expected {
            return Err(AuthorityError::Inactive);
        }
        let next = state.epoch.checked_add(1).ok_or(AuthorityError::Inactive)?;
        if let Some(old) = state.active.take().and_then(|w| w.upgrade()) {
            old.valid.store(false, Ordering::Release);
        }
        *binding = None;
        state.epoch = next;
        self.changes.send_replace(state.epoch);
        let lease = Arc::new(Lease {
            epoch: next,
            token: uuid::Uuid::new_v4(),
            scope: scope.clone(),
            valid: AtomicBool::new(true),
        });
        *binding = Some(scope);
        state.active = Some(Arc::downgrade(&lease));
        Ok(lease)
    }
    pub(super) fn reject(&self, lease: &Lease) -> AuthorityResult<()> {
        let mut binding = self.binding.write().map_err(|_| AuthorityError::Inactive)?;
        let mut state = self.state.lock().map_err(|_| AuthorityError::Inactive)?;
        lease.valid.store(false, Ordering::Release);
        // A late failure from an old request must not sign out a replacement session.
        if state.epoch == lease.epoch {
            *binding = None;
            state.active = None;
            state.epoch = state.epoch.checked_add(1).ok_or(AuthorityError::Inactive)?;
            self.changes.send_replace(state.epoch);
        }
        Ok(())
    }
}
