//! Native-only account owner. No startup, RPC dispatch or renderer credential path.
//! Callers supply reviewed registration, one reader owner and native adapters.
#[cfg(test)]
mod acceptance;
mod actor;
mod binding;
mod enrollment;
mod freeze;
mod interaction;
mod operations;
mod selection;
pub use selection::{SelectedOperation, SelectedPurpose, SelectedScope};
pub mod rpc;
mod state;
mod storage;
#[cfg(test)]
mod tests;
mod types;

pub use freeze::FrozenAccounts;
pub use interaction::{NativeRecovery, RecoveryInput};
use mdreader_core::{device::assets::AssetRuntime, ReaderStore};
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
    sender: Option<tokio::sync::mpsc::Sender<actor::Message>>,
}
impl Coordinator {
    pub fn new(
        owner: Arc<Mutex<ReaderStore>>,
        setup: Option<NativeSetup>,
        roots: CacheRoots,
    ) -> Result<Self> {
        Self::with_asset_runtime(owner, setup, roots, AssetRuntime::default())
    }
    /// Native product composition supplies one runtime shared with its exact
    /// reader owner and read-only device asset adapter; no renderer capability.
    pub fn with_asset_runtime(
        owner: Arc<Mutex<ReaderStore>>,
        setup: Option<NativeSetup>,
        roots: CacheRoots,
        runtime: AssetRuntime,
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
            let actor_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| AccountError::Interrupted)?;
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            let actor = actor::Actor::new(owner, setup, roots, runtime, binding, shared.clone());
            std::thread::Builder::new()
                .name("mdreader-account".into())
                .spawn(move || actor_runtime.block_on(actor.run(receiver)))
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
        self.begin_action(actor::Action::Account(intent))
    }
    /// Explicit exact selection. Neither queued status nor a selected UUID alone
    /// authorizes uploading a newer revision than the caller actually selected.
    pub fn begin_selected_sync(&self, selection: SelectedOperation) -> Result<Uuid> {
        selection.validate()?;
        self.begin_action(actor::Action::SelectedSync(
            selection,
            SelectedPurpose::Content,
        ))
    }
    /// Readonly reconciliation runs behind earlier issued/retained actor work.
    pub fn inspect_selected_sync(&self, selection: SelectedOperation) -> Result<Uuid> {
        selection.validate()?;
        self.begin_action(actor::Action::InspectSelected(
            selection,
            SelectedPurpose::Content,
        ))
    }
    pub fn begin_selected_state(&self, selection: SelectedOperation) -> Result<Uuid> {
        selection.validate()?;
        self.begin_action(actor::Action::SelectedSync(
            selection,
            SelectedPurpose::UserState,
        ))
    }
    pub fn inspect_selected_state(&self, selection: SelectedOperation) -> Result<Uuid> {
        selection.validate()?;
        self.begin_action(actor::Action::InspectSelected(
            selection,
            SelectedPurpose::UserState,
        ))
    }
    pub fn begin_selected_delete(&self, selection: SelectedOperation) -> Result<Uuid> {
        selection.validate()?;
        self.begin_action(actor::Action::SelectedSync(
            selection,
            SelectedPurpose::Delete,
        ))
    }
    pub fn inspect_selected_delete(&self, selection: SelectedOperation) -> Result<Uuid> {
        selection.validate()?;
        self.begin_action(actor::Action::InspectSelected(
            selection,
            SelectedPurpose::Delete,
        ))
    }
    fn begin_action(&self, action: actor::Action) -> Result<Uuid> {
        let sender = self.sender.as_ref().ok_or(AccountError::Unconfigured)?;
        let mut state = self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?;
        if state.running.is_some() || state.frozen.is_some() {
            return Err(AccountError::Busy);
        }
        let id = Uuid::new_v4();
        let selected_sync = match &action {
            actor::Action::SelectedSync(selection, _)
            | actor::Action::InspectSelected(selection, _) => Some(selection.clone()),
            _ => None,
        };
        let command = actor::Command {
            id,
            epoch: state.epoch,
            action,
        };
        let purpose = match &command.action {
            actor::Action::SelectedSync(_, purpose)
            | actor::Action::InspectSelected(_, purpose) => Some(*purpose),
            _ => None,
        };
        sender
            .try_send(actor::Message::Run(command))
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => AccountError::Busy,
                tokio::sync::mpsc::error::TrySendError::Closed(_) => AccountError::Interrupted,
            })?;
        state.start(id, selected_sync, purpose);
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
        let (active, _lease) = self.shared.projection_lease()?;
        active.incoming.apply(&active.selected, input)
    }
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        let _ = self.shared.invalidate(None);
    }
}
