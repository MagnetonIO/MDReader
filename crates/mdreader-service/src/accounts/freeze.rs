//! Native preservation admission/drain. Not a renderer request or backup proof.
use super::*;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Tag {
    token: Uuid,
    operation: Uuid,
    owner_epoch: Uuid,
}
/// Holds the exact live native coordinator; no Clone/Debug/serialization or
/// database writer access. The service backup owner must independently bind its
/// actual root and every other writer participant before issuing any export.
pub struct FrozenAccounts {
    coordinator: Arc<Coordinator>,
    tag: Tag,
    deadline: Instant,
}
impl FrozenAccounts {
    pub fn revalidate(
        &self,
        coordinator: &Coordinator,
        operation: Uuid,
        owner_epoch: Uuid,
    ) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(AccountError::Interrupted);
        }
        if !Arc::ptr_eq(&self.coordinator.shared, &coordinator.shared)
            || operation != self.tag.operation
            || owner_epoch != self.tag.owner_epoch
        {
            return Err(AccountError::Conflict);
        }
        self.coordinator.shared.require_frozen(self.tag.token)
    }
}
impl Drop for FrozenAccounts {
    fn drop(&mut self) {
        if let Ok(mut state) = self.coordinator.shared.view.lock() {
            if state.frozen == Some(self.tag) {
                state.resumed = Some(self.tag);
                state.frozen = None;
            }
        }
    }
}
impl Coordinator {
    pub(crate) fn resumed(&self, operation: Uuid, owner_epoch: Uuid) -> Result<()> {
        let state = self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?;
        if state.frozen.is_some()
            || state
                .resumed
                .is_none_or(|tag| tag.operation != operation || tag.owner_epoch != owner_epoch)
        {
            return Err(AccountError::Conflict);
        }
        Ok(())
    }

    /// Preservation-only: deny new admission and drain already-issued work, with
    /// no inferred logout/cancellation or profile mutation. Caller owns this future
    /// in the actual service process; timeout returns no quiescence proof. A remote
    /// adapter must retain this guard server-side under a native-only scoped token.
    pub async fn freeze(
        self: &Arc<Self>,
        operation: Uuid,
        owner_epoch: Uuid,
        deadline: Instant,
    ) -> Result<FrozenAccounts> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if operation.get_version_num() != 4
            || owner_epoch.get_version_num() != 4
            || remaining.is_zero()
            || remaining > Duration::from_secs(30)
        {
            return Err(AccountError::Invalid);
        }
        let tag = Tag {
            token: Uuid::new_v4(),
            operation,
            owner_epoch,
        };
        {
            let mut state = self
                .shared
                .view
                .lock()
                .map_err(|_| AccountError::Interrupted)?;
            if state.frozen.is_some() {
                return Err(AccountError::Busy);
            }
            state.resumed = None;
            state.frozen = Some(tag);
        }
        let guard = FrozenAccounts {
            coordinator: self.clone(),
            tag,
            deadline,
        };
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
            loop {
                let settled = self.shared.settled.notified();
                let ready = {
                    let state = self
                        .shared
                        .view
                        .lock()
                        .map_err(|_| AccountError::Interrupted)?;
                    state.running.is_none() && state.projections == 0
                };
                if ready {
                    break;
                }
                settled.await;
            }
            if let Some(sender) = &self.sender {
                let (reply, receive) = tokio::sync::oneshot::channel();
                sender
                    .try_send(actor::Message::Settle {
                        token: tag.token,
                        deadline,
                        reply,
                    })
                    .map_err(|_| AccountError::Busy)?;
                receive.await.map_err(|_| AccountError::Interrupted)??;
            }
            self.shared.require_frozen(tag.token)
        })
        .await
        .map_err(|_| AccountError::Interrupted)??;
        guard.revalidate(self, operation, owner_epoch)?;
        Ok(guard)
    }
}
pub(super) struct ProjectionLease {
    shared: Arc<state::Shared>,
}
impl Drop for ProjectionLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.view.lock() {
            state.projections = state.projections.saturating_sub(1);
            self.shared.settled.notify_one();
        }
    }
}
impl state::Shared {
    pub fn require_frozen(&self, token: Uuid) -> Result<()> {
        let state = self.view.lock().map_err(|_| AccountError::Interrupted)?;
        if state.frozen.is_none_or(|tag| tag.token != token)
            || state.running.is_some()
            || state.projections != 0
        {
            return Err(AccountError::Conflict);
        }
        Ok(())
    }
    pub fn projection_lease(
        self: &Arc<Self>,
    ) -> crate::sync::incoming::Result<(Arc<state::Active>, ProjectionLease)> {
        let mut state = self
            .view
            .lock()
            .map_err(|_| crate::sync::incoming::Error::Inactive)?;
        if state.frozen.is_some() || state.projections >= 2 {
            return Err(crate::sync::incoming::Error::Busy);
        }
        let active = state
            .active
            .clone()
            .ok_or(crate::sync::incoming::Error::Inactive)?;
        state.projections += 1;
        Ok((
            active,
            ProjectionLease {
                shared: self.clone(),
            },
        ))
    }
}
