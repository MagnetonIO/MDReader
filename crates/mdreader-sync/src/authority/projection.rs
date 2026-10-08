//! Short synchronous publication barrier for a verified native activation.
use super::{session::Lease, *};
use crate::SyncScope;
use std::sync::{atomic::Ordering, Arc};

/// Minted only by a current verified NativeAuthority. Keep expensive work outside
/// `with_current`; callers must also fence their concrete selected owner.
pub struct ProjectionPermit {
    pub(super) controller: Arc<SessionController>,
    pub(super) lease: Arc<Lease>,
    pub(super) expires: u64,
}
impl fmt::Debug for ProjectionPermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        redacted("ProjectionPermit", f)
    }
}
pub struct ProjectionGuard<'a> {
    lease: &'a Lease,
    expires: u64,
}
impl ProjectionGuard<'_> {
    /// Call immediately before the owning transaction's final COMMIT. The
    /// session barrier remains held; this additionally checks wall-clock expiry.
    pub fn check(&self) -> AuthorityResult<()> {
        if !self.lease.valid.load(Ordering::Acquire) || now()? >= self.expires {
            return Err(AuthorityError::Inactive);
        }
        Ok(())
    }
}
impl ProjectionPermit {
    pub fn scope(&self) -> &SyncScope {
        &self.lease.scope
    }
    pub fn session_token(&self) -> uuid::Uuid {
        self.lease.token
    }
    /// The barrier prevents logout/reactivation until the short closure ends.
    /// Guard lifetimes cannot escape it. This grants no selected-library access.
    /// Closure errors/uncertain commits remain the caller's typed responsibility.
    pub fn with_current<T>(
        &self,
        action: impl for<'guard> FnOnce(&ProjectionGuard<'guard>) -> T,
    ) -> AuthorityResult<T> {
        let binding = self
            .controller
            .binding
            .read()
            .map_err(|_| AuthorityError::Inactive)?;
        let state = self
            .controller
            .state
            .lock()
            .map_err(|_| AuthorityError::Inactive)?;
        if state.epoch != self.lease.epoch || binding.as_ref() != Some(&self.lease.scope) {
            return Err(AuthorityError::Inactive);
        }
        drop(state);
        let guard = ProjectionGuard {
            lease: &self.lease,
            expires: self.expires,
        };
        guard.check()?;
        let result = action(&guard);
        guard.check()?;
        drop(binding);
        Ok(result)
    }
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
