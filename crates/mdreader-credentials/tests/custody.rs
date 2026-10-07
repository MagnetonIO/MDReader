use mdreader_credentials::*;
use mdreader_provider::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[derive(Default)]
struct MemoryVault(Mutex<BTreeMap<CredentialReference, ProviderGrant>>);
impl Vault for MemoryVault {
    fn get(&self, r: &CredentialReference) -> Result<Option<ProviderGrant>> {
        Ok(self.0.lock().unwrap().get(r).cloned())
    }
    fn create(&self, r: &CredentialReference, g: &ProviderGrant) -> Result<()> {
        let mut records = self.0.lock().unwrap();
        if records.contains_key(r) {
            return Err(ProviderError::Conflict);
        }
        records.insert(r.clone(), g.clone());
        Ok(())
    }
    fn compare_exchange(
        &self,
        r: &CredentialReference,
        expected: u64,
        g: &ProviderGrant,
    ) -> Result<()> {
        let mut records = self.0.lock().unwrap();
        if records
            .get(r)
            .is_none_or(|prior| prior.secret().generation() != expected)
        {
            return Err(ProviderError::Conflict);
        }
        records.insert(r.clone(), g.clone());
        Ok(())
    }
    fn delete(&self, r: &CredentialReference) -> Result<()> {
        self.0.lock().unwrap().remove(r);
        Ok(())
    }
}
fn grant() -> ProviderGrant {
    ProviderGrant::new(
        &Registration::google_drive("synthetic.apps.googleusercontent.com".into()).unwrap(),
        AccountBinding::new("permission-id").unwrap(),
        NativeSelection::DriveFiles {
            files: vec![DriveFile {
                id: RecordKey::new("selected").unwrap(),
                resource_key: None,
            }],
        },
        SecretBundle::new(
            "access-do-not-log".into(),
            "refresh-do-not-log".into(),
            1_999_999_999,
            [1; 32],
        )
        .unwrap(),
    )
    .unwrap()
}
fn context() -> IoContext {
    IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(2),
        256,
        1024,
        Cancellation::default(),
    )
    .unwrap()
}
#[tokio::test]
async fn native_vault_worker_loads_only_explicit_fresh_reference() {
    let memory = Arc::new(MemoryVault::default());
    let reference = CredentialReference::fresh();
    memory.create(&reference, &grant()).unwrap();
    let client = VaultClient::new(memory).unwrap();
    let loaded = client.get(&reference, &context()).await.unwrap().unwrap();
    assert_eq!(loaded.secret().access_token(), "access-do-not-log");
    assert!(CredentialReference::new("cloud-credential-legacy".into()).is_err());
}
#[tokio::test]
async fn rotation_is_scoped_and_vault_stall_is_bounded() {
    let original = grant();
    let updated = original
        .refreshed("new-access".into(), None, 2_000_000_000)
        .unwrap();
    assert_eq!(
        updated.configuration_digest(),
        original.configuration_digest()
    );
    assert_eq!(
        updated.secret().cursor_key(),
        original.secret().cursor_key()
    );
    assert_eq!(updated.secret().generation(), 2);
    assert_eq!(
        updated.secret().refresh_token(),
        original.secret().refresh_token()
    );
    assert!(!format!("{original:?} {:?}", original.secret()).contains("do-not-log"));
    struct Stall {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl Vault for Stall {
        fn get(&self, _: &CredentialReference) -> Result<Option<ProviderGrant>> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Ok(None)
        }
        fn create(&self, _: &CredentialReference, _: &ProviderGrant) -> Result<()> {
            unreachable!()
        }
        fn compare_exchange(
            &self,
            _: &CredentialReference,
            _: u64,
            _: &ProviderGrant,
        ) -> Result<()> {
            unreachable!()
        }
        fn delete(&self, _: &CredentialReference) -> Result<()> {
            unreachable!()
        }
    }
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(2);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(2);
    let client = VaultClient::new(Arc::new(Stall {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    }))
    .unwrap();
    let short = || {
        IoContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_millis(30),
            256,
            1024,
            Cancellation::default(),
        )
        .unwrap()
    };
    assert_eq!(
        client
            .get(&CredentialReference::fresh(), &short())
            .await
            .unwrap_err(),
        ProviderError::Timeout
    );
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        client
            .get(&CredentialReference::fresh(), &short())
            .await
            .unwrap_err(),
        ProviderError::Timeout
    );
    assert_eq!(
        client
            .get(&CredentialReference::fresh(), &context())
            .await
            .unwrap_err(),
        ProviderError::Unavailable
    );
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();
}
