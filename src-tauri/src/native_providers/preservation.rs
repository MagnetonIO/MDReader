//! Retained GUI OAuth/vault work participates in actual profile admission.
use super::*;
use crate::profile_transition::{FreezeScope, FrozenParticipant, LiveParticipant};
use mdreader_profile::{Participant, RootBinding};
use std::future::Future;
use tauri::Manager;

pub(super) async fn run<T: Send + 'static>(
    app: &tauri::AppHandle,
    action: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    let state = app
        .try_state::<crate::profile_transition::persistence::commands::PersistenceState>()
        .ok_or(NativeProviderError::Unsupported)?;
    let lease = state
        .transition
        .effect(Participant::LegacyCloudAndEnterprise)
        .map_err(|_| NativeProviderError::Busy)?;
    // Dropping the renderer invoke only detaches the join. The actual worker
    // owns its lease until OAuth, ledger publication and uncertainty settle.
    tauri::async_runtime::spawn(async move {
        let _lease = lease;
        action.await
    })
    .await
    .map_err(|_| NativeProviderError::ServiceUnavailable)?
}

pub(crate) struct ParticipantOwner {
    root: RootBinding,
    owner: Arc<NativeProviders>,
}
impl ParticipantOwner {
    pub(crate) fn new(root: RootBinding, owner: Arc<NativeProviders>) -> Self {
        Self { root, owner }
    }
}
impl LiveParticipant for ParticipantOwner {
    fn participant(&self) -> Participant {
        Participant::LegacyCloudAndEnterprise
    }
    fn root(&self) -> &RootBinding {
        &self.root
    }
    fn freeze(
        &self,
        scope: &FreezeScope<'_>,
    ) -> mdreader_profile::Result<Box<dyn FrozenParticipant>> {
        self.verify(scope)?;
        if let Some(vault) = &self.owner.vault {
            let context = IoContext::new(
                scope.operation(),
                scope.deadline(),
                1,
                1,
                mdreader_provider::Cancellation::default(),
            )
            .map_err(|_| mdreader_profile::Error::Invalid)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| mdreader_profile::Error::NotQuiescent)?;
            runtime
                .block_on(vault.settle(&context))
                .map_err(|_| mdreader_profile::Error::ReconciliationRequired)?;
        }
        self.verify(scope)?;
        Ok(Box::new(FrozenOwner {
            root: self.root.clone(),
            owner: self.owner.clone(),
            operation: scope.operation(),
            epoch: scope.owner_epoch(),
        }))
    }
}
impl ParticipantOwner {
    fn verify(&self, scope: &FreezeScope<'_>) -> mdreader_profile::Result<()> {
        if &self.root != scope.root()
            || Instant::now() >= scope.deadline()
            || self.owner.admission.available_permits() != 1
        {
            return Err(mdreader_profile::Error::NotQuiescent);
        }
        self.owner
            .with_ledger(|ledger| {
                if ledger.root.path
                    != self
                        .root
                        .locator
                        .restore()
                        .map_err(|_| NativeProviderError::Storage)?
                        .join("reader-v1")
                {
                    return Err(NativeProviderError::Storage);
                }
                ledger.verify()
            })
            .map_err(|_| mdreader_profile::Error::Conflict)
    }
}
struct FrozenOwner {
    root: RootBinding,
    owner: Arc<NativeProviders>,
    operation: Uuid,
    epoch: Uuid,
}
impl FrozenParticipant for FrozenOwner {
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> mdreader_profile::Result<()> {
        if self.operation != scope.operation() || self.epoch != scope.owner_epoch() {
            return Err(mdreader_profile::Error::Conflict);
        }
        ParticipantOwner::new(self.root.clone(), self.owner.clone()).verify(scope)
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> mdreader_profile::Result<()>,
    ) -> mdreader_profile::Result<()> {
        Err(mdreader_profile::Error::Invalid)
    }
}
