use crate::*;
use mdreader_provider::{IoContext, ProviderError, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, mpsc};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};
/// Existing references have one service custodian. OS keyrings do not offer
/// cross-process CAS; existing references must only be mutated by that owner.
pub trait Vault: Send + Sync {
    fn get(&self, reference: &CredentialReference) -> Result<Option<ProviderGrant>>;
    fn create(&self, reference: &CredentialReference, grant: &ProviderGrant) -> Result<()>;
    fn compare_exchange(
        &self,
        reference: &CredentialReference,
        expected_generation: u64,
        grant: &ProviderGrant,
    ) -> Result<()>;
    fn delete(&self, reference: &CredentialReference) -> Result<()>;
}
type Reply<T> = tokio::sync::oneshot::Sender<Result<T>>;
enum Command {
    Get(CredentialReference, Reply<Option<ProviderGrant>>),
    Create(CredentialReference, ProviderGrant, Reply<()>),
    Exchange(CredentialReference, u64, ProviderGrant, Reply<()>),
    Delete(CredentialReference, Reply<()>),
    Barrier(Reply<()>),
}
/// Exactly one native worker and one queued operation. A hung OS call cannot
/// spawn unbounded replacement workers. A timed-out mutation may still finish;
/// source authority must be revalidated independently before any provider I/O.
#[derive(Clone)]
pub struct VaultClient {
    sender: mpsc::SyncSender<Command>,
}
impl VaultClient {
    pub fn new(vault: Arc<dyn Vault>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("provider-vault".into())
            .spawn(move || {
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Get(r, reply) => {
                            let _ = reply.send(vault.get(&r));
                        }
                        Command::Create(r, g, reply) => {
                            let _ = reply.send(vault.create(&r, &g));
                        }
                        Command::Exchange(r, n, g, reply) => {
                            let _ = reply.send(vault.compare_exchange(&r, n, &g));
                        }
                        Command::Delete(r, reply) => {
                            let _ = reply.send(vault.delete(&r));
                        }
                        Command::Barrier(reply) => {
                            let _ = reply.send(Ok(()));
                        }
                    }
                }
            })
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        Ok(Self { sender })
    }
    async fn request<T>(
        &self,
        context: &IoContext,
        build: impl FnOnce(Reply<T>) -> Command,
    ) -> Result<T> {
        context.check()?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.sender.try_send(build(sender)).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => ProviderError::Unavailable,
            mpsc::TrySendError::Disconnected(_) => ProviderError::CredentialUnavailable,
        })?;
        let result = tokio::select! {biased;
            _=context.cancellation().cancelled()=>Err(ProviderError::Cancelled),
            result=tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),receiver)=>match result{Ok(Ok(result))=>result,Ok(Err(_))=>Err(ProviderError::CredentialUnavailable),Err(_)=>Err(ProviderError::Timeout)},
        };
        context.check()?;
        result
    }
    pub async fn get(
        &self,
        r: &CredentialReference,
        c: &IoContext,
    ) -> Result<Option<ProviderGrant>> {
        self.request(c, |reply| Command::Get(r.clone(), reply))
            .await
    }
    pub async fn create(
        &self,
        r: &CredentialReference,
        g: &ProviderGrant,
        c: &IoContext,
    ) -> Result<()> {
        self.request(c, |reply| Command::Create(r.clone(), g.clone(), reply))
            .await
    }
    pub async fn compare_exchange(
        &self,
        r: &CredentialReference,
        n: u64,
        g: &ProviderGrant,
        c: &IoContext,
    ) -> Result<()> {
        self.request(c, |reply| Command::Exchange(r.clone(), n, g.clone(), reply))
            .await
    }
    pub async fn delete(&self, r: &CredentialReference, c: &IoContext) -> Result<()> {
        self.request(c, |reply| Command::Delete(r.clone(), reply))
            .await
    }
    /// After caller admission is denied, positively join every earlier issued
    /// OS operation, including a mutation whose caller timed out or cancelled.
    /// Failure provides no settlement proof and never starts another worker.
    pub async fn settle(&self, context: &IoContext) -> Result<()> {
        loop {
            match self.request(context, Command::Barrier).await {
                // Only a full, not-yet-dispatched queue produces Unavailable
                // for this no-op command. A timed-out issued barrier is never
                // redispatched and cannot count as successful settlement.
                Err(ProviderError::Unavailable) => {
                    context.check()?;
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                result => return result,
            }
        }
    }
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct Wire {
    #[zeroize(skip)]
    v: u8,
    #[zeroize(skip)]
    registration: Registration,
    #[zeroize(skip)]
    account: mdreader_provider::AccountBinding,
    #[zeroize(skip)]
    selection: NativeSelection,
    access: String,
    refresh: String,
    cursor: [u8; 32],
    #[zeroize(skip)]
    expires: u64,
    #[zeroize(skip)]
    generation: u64,
    #[serde(default)]
    verified_scopes: Option<String>,
}
fn encode(g: &ProviderGrant) -> Result<Zeroizing<String>> {
    let wire = Wire {
        v: 4,
        registration: g.registration().clone(),
        account: g.account().clone(),
        selection: g.selection().clone(),
        access: g.secret().access_token().into(),
        refresh: g.secret().refresh_token().into(),
        cursor: g.secret().cursor_key(),
        expires: g.secret().expires_at_unix_seconds(),
        generation: g.secret().generation(),
        verified_scopes: g.verified_scopes().map(str::to_owned),
    };
    let text = Zeroizing::new(
        serde_json::to_string(&wire).map_err(|_| ProviderError::CredentialUnavailable)?,
    );
    if text.len() > 128 * 1024 {
        return Err(ProviderError::TooLarge);
    }
    Ok(text)
}
fn decode(text: Zeroizing<String>) -> Result<ProviderGrant> {
    if text.len() > 128 * 1024 {
        return Err(ProviderError::TooLarge);
    }
    let mut wire: Wire =
        serde_json::from_str(&text).map_err(|_| ProviderError::CredentialUnavailable)?;
    if !matches!(wire.v, 3 | 4) || wire.v == 3 && wire.verified_scopes.is_some() {
        return Err(ProviderError::CredentialUnavailable);
    }
    let secret = SecretBundle::new(
        std::mem::take(&mut wire.access),
        std::mem::take(&mut wire.refresh),
        wire.expires,
        wire.cursor,
    )?
    .restore_generation(wire.generation)?;
    let grant = ProviderGrant::new(
        &wire.registration,
        wire.account.clone(),
        wire.selection.clone(),
        secret,
    )?;
    match wire.verified_scopes.as_deref() {
        Some(scopes) => grant.with_verified_scopes(scopes),
        None => Ok(grant),
    }
}
/// Fresh namespace only. Construction never reads or writes the OS keychain.
pub struct KeyringVault {
    service: String,
}
static CUSTODY: Mutex<()> = Mutex::new(());
impl KeyringVault {
    pub fn new(identifier: &str) -> Result<Self> {
        if identifier.is_empty()
            || identifier.len() > 200
            || !identifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            service: format!("{identifier}.native-provider-credentials-v3"),
        })
    }
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn entry(&self, r: &CredentialReference) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, r.as_str())
            .map_err(|_| ProviderError::CredentialUnavailable)
    }
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn read(&self, r: &CredentialReference) -> Result<Option<ProviderGrant>> {
        match self.entry(r)?.get_password() {
            Ok(text) => decode(Zeroizing::new(text)).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(ProviderError::CredentialUnavailable),
        }
    }
}
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl Vault for KeyringVault {
    fn get(&self, r: &CredentialReference) -> Result<Option<ProviderGrant>> {
        let _guard = CUSTODY
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        self.read(r)
    }
    fn create(&self, r: &CredentialReference, g: &ProviderGrant) -> Result<()> {
        let _guard = CUSTODY
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        if self.read(r)?.is_some() {
            return Err(ProviderError::Conflict);
        }
        self.entry(r)?
            .set_password(&encode(g)?)
            .map_err(|_| ProviderError::CredentialUnavailable)
    }
    fn compare_exchange(&self, r: &CredentialReference, n: u64, g: &ProviderGrant) -> Result<()> {
        let _guard = CUSTODY
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        let prior = self.read(r)?.ok_or(ProviderError::CredentialUnavailable)?;
        if prior.secret().generation() != n
            || g.secret().generation() != n.checked_add(1).ok_or(ProviderError::Invalid)?
            || prior.configuration_digest() != g.configuration_digest()
            || prior.secret().cursor_key() != g.secret().cursor_key()
        {
            return Err(ProviderError::Conflict);
        }
        self.entry(r)?
            .set_password(&encode(g)?)
            .map_err(|_| ProviderError::CredentialUnavailable)
    }
    fn delete(&self, r: &CredentialReference) -> Result<()> {
        let _guard = CUSTODY
            .lock()
            .map_err(|_| ProviderError::CredentialUnavailable)?;
        match self.entry(r)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(ProviderError::CredentialUnavailable),
        }
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
impl Vault for KeyringVault {
    fn get(&self, _: &CredentialReference) -> Result<Option<ProviderGrant>> {
        Err(ProviderError::Unsupported)
    }
    fn create(&self, _: &CredentialReference, _: &ProviderGrant) -> Result<()> {
        Err(ProviderError::Unsupported)
    }
    fn compare_exchange(&self, _: &CredentialReference, _: u64, _: &ProviderGrant) -> Result<()> {
        Err(ProviderError::Unsupported)
    }
    fn delete(&self, _: &CredentialReference) -> Result<()> {
        Err(ProviderError::Unsupported)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_wire_rejects_unknown_schema_and_preserves_rotation() {
        let registration =
            Registration::google_drive("synthetic.apps.googleusercontent.com".into()).unwrap();
        let grant = ProviderGrant::new(
            &registration,
            mdreader_provider::AccountBinding::new("permission").unwrap(),
            NativeSelection::DriveFiles {
                files: vec![DriveFile {
                    id: mdreader_provider::RecordKey::new("file").unwrap(),
                    resource_key: None,
                }],
            },
            SecretBundle::new("access".into(), "refresh".into(), 1999999999, [1; 32]).unwrap(),
        )
        .unwrap()
        .refreshed("new".into(), None, 1999999999)
        .unwrap();
        let wire = encode(&grant).unwrap();
        let loaded = decode(wire.clone()).unwrap();
        assert_eq!(loaded.secret().generation(), 2);
        assert_eq!(loaded.configuration_digest(), grant.configuration_digest());
        assert_eq!(loaded.secret().cursor_key(), [1; 32]);
        assert!(wire.contains("\"v\":4"));
        assert!(decode(Zeroizing::new(wire.replace("\"v\":4", "\"v\":9"))).is_err());
        assert!(!format!("{grant:?}").contains("refresh"));
    }
}
