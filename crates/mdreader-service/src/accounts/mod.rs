//! Native-only account owner. No startup, RPC dispatch or renderer credential path.
//! Callers supply reviewed registration, one reader owner and native adapters.
mod actor;
mod binding;
mod enrollment;
mod interaction;
mod operations;
mod state;
mod storage;
#[cfg(test)]
mod tests;
mod types;

pub use interaction::{NativeRecovery, RecoveryInput};
use mdreader_core::ReaderStore;
use mdreader_sync::authority::{
    CandidateKind, CustodyLifecycle, CustodySummary, EnrollmentLogin, EnrollmentProgress,
    NativeAuthority, NativeAuthorityFactory, NativeBrowser, PendingCustody, SessionController,
    SyncCredentialReference, VerifiedLogin,
};
use mdreader_sync::transport::{Authority, Budget, Transport};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub use types::*;
use uuid::Uuid;

/// Trusted native composition only; neither Deserialize nor Debug. No defaults.
pub struct NativeSetup {
    pub factory: NativeAuthorityFactory,
    pub transport: Transport,
    pub browser: Arc<dyn NativeBrowser>,
    pub registered_redirect: String,
    pub recovery: Option<Arc<dyn NativeRecovery>>,
}
/// Native-owned cache roots, never paths supplied by an account RPC.
pub struct CacheRoots {
    pub documents: PathBuf,
    pub assets: PathBuf,
}

/// One serial account actor per reader owner. Clone an Arc<Coordinator>, not
/// separate coordinators over the same owner. No SQLite guard spans an await.
pub struct Coordinator {
    shared: Arc<state::Shared>,
    sender: Option<tokio::sync::mpsc::Sender<actor::Command>>,
}
impl Coordinator {
    pub fn new(
        owner: Arc<Mutex<ReaderStore>>,
        setup: Option<NativeSetup>,
        roots: CacheRoots,
    ) -> Result<Self> {
        let binding = {
            let mut store = owner.try_lock().map_err(|_| AccountError::Busy)?;
            storage::load(&mut store)?
        };
        let controller = setup.as_ref().map(|s| s.factory.session_controller());
        let recovery_available = setup.as_ref().is_some_and(|s| s.recovery.is_some());
        let shared = Arc::new(state::Shared::new(
            controller,
            binding.is_some(),
            recovery_available,
        ));
        let sender = if let Some(setup) = setup {
            // Build before publishing the coordinator; failure cannot leave a running job.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| AccountError::Interrupted)?;
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            let actor = actor::Actor::new(owner, setup, roots, binding, shared.clone());
            std::thread::Builder::new()
                .name("mdreader-account".into())
                .spawn(move || runtime.block_on(actor.run(receiver)))
                .map_err(|_| AccountError::Interrupted)?;
            Some(sender)
        } else {
            None
        };
        Ok(Self { shared, sender })
    }
    /// Returns immediately. Only the actor owns browser/network/private custody work.
    pub fn begin(&self, intent: AccountIntent) -> Result<Uuid> {
        intent.validate()?;
        let sender = self.sender.as_ref().ok_or(AccountError::Unconfigured)?;
        let mut state = self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?;
        if state.running.is_some() {
            return Err(AccountError::Busy);
        }
        let id = Uuid::new_v4();
        let command = actor::Command {
            id,
            epoch: state.epoch,
            intent,
        };
        sender.try_send(command).map_err(|error| match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => AccountError::Busy,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => AccountError::Interrupted,
        })?;
        state.start(id);
        Ok(id)
    }
    pub fn status(&self) -> Result<AccountStatus> {
        self.shared.status()
    }
    /// History is bounded to eight jobs. Evicted/unknown IDs return None.
    pub fn operation(&self, id: Uuid) -> Result<Option<Job>> {
        Ok(self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?
            .jobs
            .iter()
            .find(|j| j.id == id)
            .cloned())
    }
    /// Cancel only this exact running job; stale cancellation cannot sign out a
    /// replacement. The actor drops its in-flight future before accepting more work.
    pub fn cancel(&self, id: Uuid) -> Result<()> {
        self.shared.invalidate(Some(id))
    }
    /// Retains offline data, selected remote library and opaque OS-vault reference.
    pub fn disconnect(&self) -> Result<()> {
        self.shared.invalidate(None)
    }
    /// Native blocking-worker seam only. Uses the same actual authority/selection
    /// as transport, and the existing atomic owner projection; never marks placeholders.
    pub fn apply_prepared(
        &self,
        input: &mdreader_sync::incoming::PreparedIncoming,
    ) -> crate::sync::incoming::Result<crate::sync::incoming::Outcome> {
        let active = self
            .shared
            .view
            .lock()
            .map_err(|_| crate::sync::incoming::Error::Inactive)?
            .active
            .clone()
            .ok_or(crate::sync::incoming::Error::Inactive)?;
        active.incoming.apply(&active.selected, input)
    }
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        let _ = self.shared.invalidate(None);
    }
}
