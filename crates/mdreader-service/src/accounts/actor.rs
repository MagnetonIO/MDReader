use super::*;
use crate::sync::incoming::IncomingCoordinator;
use storage::Binding;

pub(super) struct Command {
    pub id: Uuid,
    pub epoch: u64,
    pub intent: AccountIntent,
}
pub(super) struct Actor {
    pub owner: Arc<Mutex<ReaderStore>>,
    pub setup: NativeSetup,
    pub roots: CacheRoots,
    pub binding: Option<Binding>,
    /// Exact same private record is retained after a failed/uncertain OS write.
    pub pending: Option<PendingCustody>,
    pub incoming: Option<Arc<IncomingCoordinator>>,
    pub shared: Arc<state::Shared>,
}
impl Actor {
    pub fn new(
        owner: Arc<Mutex<ReaderStore>>,
        setup: NativeSetup,
        roots: CacheRoots,
        binding: Option<Binding>,
        shared: Arc<state::Shared>,
    ) -> Self {
        Self {
            owner,
            setup,
            roots,
            binding,
            pending: None,
            incoming: None,
            shared,
        }
    }
    pub async fn run(mut self, mut receiver: tokio::sync::mpsc::Receiver<Command>) {
        while let Some(command) = receiver.recv().await {
            let mut changes = self.shared.changes.subscribe();
            let shared = self.shared.clone();
            let result = if shared.check(&command).is_err() {
                Err(AccountError::Cancelled)
            } else {
                tokio::select! {
                    biased;
                    _ = changes.wait_for(|epoch| *epoch != command.epoch) => Err(AccountError::Cancelled),
                    result = tokio::time::timeout(std::time::Duration::from_secs(660),self.execute(&command)) => result.unwrap_or(Err(AccountError::Interrupted)),
                }
            };
            // Cancelled operations cannot publish status or establish a replacement.
            // Any persisted uncertain enrollment/approval remains intact in the vault.
            if shared.check(&command).is_ok() {
                let _ = self.refresh_custody(&command).await;
            }
            shared.finish(&command, result);
        }
        let _ = self.shared.invalidate(None);
    }
    pub fn bound(&self) -> Result<Binding> {
        self.binding
            .as_ref()
            .map(|b| Binding {
                library: b.library,
                reference: b.reference.clone(),
            })
            .ok_or(AccountError::CustodyUnavailable)
    }
    pub async fn refresh_custody(&self, command: &Command) -> Result<CustodySummary> {
        let binding = self.bound()?;
        let summary = self
            .setup
            .factory
            .custody_summary(&binding.reference, &binding.library.to_string())
            .await?
            .ok_or(AccountError::CustodyUnavailable)?;
        let mut state = self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?;
        if state.epoch != command.epoch || state.running != Some(command.id) {
            return Err(AccountError::Cancelled);
        }
        state.custody = Some(summary.into());
        Ok(summary)
    }
    pub fn active(&self) -> Result<Arc<state::Active>> {
        let active = self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?
            .active
            .clone()
            .ok_or(AccountError::ReconnectRequired)?;
        active
            .authority
            .scope()
            .map_err(|_| AccountError::ReconnectRequired)?;
        Ok(active)
    }
    pub fn activate(&mut self, command: &Command, authority: NativeAuthority) -> Result<Outcome> {
        let binding = self.bound()?;
        self.shared.check(command)?;
        let incoming = match &self.incoming {
            Some(incoming) => incoming.clone(),
            None => {
                let incoming = Arc::new(
                    IncomingCoordinator::new(
                        self.owner.clone(),
                        binding.library,
                        self.roots.documents.clone(),
                        self.roots.assets.clone(),
                    )
                    .map_err(AccountError::from)?,
                );
                self.incoming = Some(incoming.clone());
                incoming
            }
        };
        let selected = incoming.select(&authority).map_err(AccountError::from)?;
        let mut state = self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?;
        if state.epoch != command.epoch || state.running != Some(command.id) {
            return Err(AccountError::Cancelled);
        }
        authority
            .scope()
            .map_err(|_| AccountError::ReconnectRequired)?;
        state.active = Some(Arc::new(state::Active {
            authority,
            incoming,
            selected,
        }));
        Ok(Outcome::Connected)
    }
}
