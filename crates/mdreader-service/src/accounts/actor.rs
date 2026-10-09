use super::*;
use crate::sync::incoming::IncomingCoordinator;
use storage::Binding;

pub(super) struct Command {
    pub id: Uuid,
    pub epoch: u64,
    pub action: Action,
}
/// Private actor actions. Only closed, validated public selection DTOs enter.
pub(super) enum Action {
    ListPending(SelectedScope),
    ResolvePending(SelectedScope, crate::sync::incoming::ResolutionRequest),
    Account(AccountIntent),
    SelectedSync(SelectedOperation, SelectedPurpose),
    InspectSelected(SelectedOperation, SelectedPurpose),
}
pub(super) enum Message {
    Run(Command),
    InspectCustody {
        epoch: u64,
        reply: std::sync::mpsc::SyncSender<Result<()>>,
    },
    Settle {
        token: Uuid,
        deadline: std::time::Instant,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
}
pub(super) struct Actor {
    pub owner: Arc<Mutex<ReaderStore>>,
    pub setup: NativeSetup,
    pub roots: CacheRoots,
    runtime: AssetRuntime,
    pub binding: Option<Binding>,
    /// Exact same private record is retained after a failed/uncertain OS write.
    pub pending: Option<PendingCustody>,
    pub incoming: Option<Arc<IncomingCoordinator>>,
    pub shared: Arc<state::Shared>,
    // Retain join handles independently of the cancellable job future. A profile
    // freeze/cancel may not mistake a dropped await for completed owner work.
    blocking: Vec<tokio::task::JoinHandle<()>>,
}
impl Actor {
    pub fn new(
        owner: Arc<Mutex<ReaderStore>>,
        setup: NativeSetup,
        roots: CacheRoots,
        runtime: AssetRuntime,
        binding: Option<Binding>,
        shared: Arc<state::Shared>,
    ) -> Self {
        Self {
            owner,
            setup,
            roots,
            runtime,
            binding,
            pending: None,
            incoming: None,
            shared,
            blocking: Vec::new(),
        }
    }
    pub async fn run(mut self, mut receiver: tokio::sync::mpsc::Receiver<Message>) {
        while let Some(message) = receiver.recv().await {
            let command = match message {
                Message::Run(command) => command,
                Message::InspectCustody { epoch, reply } => {
                    let result = self.inspect_custody(epoch).await;
                    let _ = reply.send(result);
                    continue;
                }
                Message::Settle {
                    token,
                    deadline,
                    reply,
                } => {
                    let result =
                        async {
                            self.shared.require_frozen(token)?;
                            if let Some(recovery) = &self.setup.recovery {
                                recovery.settle(deadline).await?;
                            }
                            self.setup
                                .factory
                                .settle_custody(deadline.min(
                                    std::time::Instant::now() + std::time::Duration::from_secs(5),
                                ))
                                .await?;
                            self.shared.require_frozen(token)
                        }
                        .await;
                    let _ = reply.send(result);
                    continue;
                }
            };
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
            self.settle_blocking().await;
            // Cancelled operations cannot publish status or establish a replacement.
            // Any persisted uncertain enrollment/approval remains intact in the vault.
            if shared.check(&command).is_ok()
                && !matches!(&command.action, Action::InspectSelected(..))
            {
                let _ = self.refresh_custody(&command).await;
            }
            shared.finish(&command, result);
        }
        let _ = self.shared.invalidate(None);
    }
    pub fn blocking<T: Send + 'static>(
        &mut self,
        action: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> tokio::sync::oneshot::Receiver<Result<T>> {
        let (send, receive) = tokio::sync::oneshot::channel();
        self.blocking.push(tokio::task::spawn_blocking(move || {
            let _ = send.send(action());
        }));
        receive
    }
    async fn settle_blocking(&mut self) {
        for job in self.blocking.drain(..) {
            let _ = job.await;
        }
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
        state.custody_inspected = true;
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
                    IncomingCoordinator::with_runtime(
                        self.owner.clone(),
                        binding.library,
                        self.roots.documents.clone(),
                        self.roots.assets.clone(),
                        self.runtime.clone(),
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
