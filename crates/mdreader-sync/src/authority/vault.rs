use super::*;
use std::{
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
/// Native service is sole mutator. This is sync custody, never a ProviderGrant.
pub trait SyncVault: Send + Sync {
    fn get(&self, reference: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>>;
    /// Idempotent only for the same exact pending material/generation/state.
    fn create(&self, record: &CustodyRecord) -> AuthorityResult<()>;
    fn compare_exchange(&self, expected: u64, record: &CustodyRecord) -> AuthorityResult<()>;
}
type Reply<T> = tokio::sync::oneshot::Sender<AuthorityResult<T>>;
enum Command {
    Get(SyncCredentialReference, Reply<Option<CustodyRecord>>),
    Create(CustodyRecord, Reply<()>),
    Exchange(u64, CustodyRecord, Reply<()>),
    Barrier(Reply<()>),
}
#[derive(Clone)]
pub struct SyncVaultClient {
    sender: mpsc::SyncSender<Command>,
}
impl std::fmt::Debug for SyncVaultClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("SyncVaultClient", f)
    }
}
impl SyncVaultClient {
    /// Construct once in the native custodian. One worker/one queued operation; a hung OS
    /// call never causes this client to spawn replacement workers. No live I/O on creation.
    pub fn new(vault: Arc<dyn SyncVault>) -> AuthorityResult<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("sync-vault".into())
            .spawn(move || {
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Get(r, reply) => {
                            let _ = reply.send(vault.get(&r));
                        }
                        Command::Create(r, reply) => {
                            let _ = reply.send(vault.create(&r));
                        }
                        Command::Exchange(g, r, reply) => {
                            let _ = reply.send(vault.compare_exchange(g, &r));
                        }
                        Command::Barrier(reply) => {
                            let _ = reply.send(Ok(()));
                        }
                    }
                }
            })
            .map_err(|_| AuthorityError::Custody)?;
        Ok(Self { sender })
    }
    /// FIFO proof that earlier issued OS mutations have actually returned, even
    /// when their original async receiver timed out. This does not infer success
    /// of those mutations and never reads/exports private records.
    pub async fn settle(&self, deadline: Instant) -> AuthorityResult<()> {
        self.request(deadline, Command::Barrier).await
    }
    async fn request<T>(
        &self,
        deadline: Instant,
        make: impl FnOnce(Reply<T>) -> Command,
    ) -> AuthorityResult<T> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_secs(10) {
            return Err(AuthorityError::Interrupted);
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender.try_send(make(tx)).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => AuthorityError::Busy,
            mpsc::TrySendError::Disconnected(_) => AuthorityError::Custody,
        })?;
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), rx)
            .await
            .map_err(|_| AuthorityError::Interrupted)?
            .map_err(|_| AuthorityError::Custody)?
    }
    pub async fn get(
        &self,
        reference: &SyncCredentialReference,
        deadline: Instant,
    ) -> AuthorityResult<Option<CustodyRecord>> {
        let record = self
            .request(deadline, |reply| Command::Get(reference.clone(), reply))
            .await?;
        if let Some(record) = &record {
            record.validate(reference)?;
        }
        Ok(record)
    }
    pub async fn create(&self, pending: &PendingCustody, deadline: Instant) -> AuthorityResult<()> {
        pending.record.validate(&pending.record.reference)?;
        self.request(deadline, |reply| {
            Command::Create(pending.record.copy(), reply)
        })
        .await
    }
    pub(super) async fn exchange(
        &self,
        expected: u64,
        record: &CustodyRecord,
        deadline: Instant,
    ) -> AuthorityResult<()> {
        record.validate(&record.reference)?;
        self.request(deadline, |reply| {
            Command::Exchange(expected, record.copy(), reply)
        })
        .await
    }
}
