//! Mobile composition owns no listener/process and borrows the existing writer.
pub(super) mod configuration;
mod rpc;
mod vault;
use super::*;

struct MobileAuthorizer {
    app: tauri::AppHandle,
    transport: Arc<dyn Transport>,
}
#[async_trait::async_trait]
impl Authorizer for MobileAuthorizer {
    async fn authorize(
        &self,
        registration: Registration,
        selection: Option<NativeSelection>,
        allow_editing: bool,
    ) -> Result<CompletedAuthorization> {
        let redirect = match registration.provider() {
            ProviderKind::GoogleDrive => option_env!("MDREADER_MOBILE_DRIVE_REDIRECT_URI"),
            ProviderKind::Dropbox => option_env!("MDREADER_MOBILE_DROPBOX_REDIRECT_URI"),
        }
        .ok_or(NativeProviderError::Unconfigured)?;
        let context = IoContext::new(
            Uuid::new_v4(),
            Instant::now() + Duration::from_secs(300),
            1,
            64 * 1024,
            Default::default(),
        )?;
        provider_oauth::authorize_mobile(
            &self.app,
            registration,
            redirect,
            selection,
            allow_editing,
            &*self.transport,
            &context,
        )
        .await
        .map_err(Into::into)
    }
}
impl NativeProviders {
    pub(crate) fn new(app: &tauri::AppHandle, backend: ReaderBackend) -> Result<Self> {
        use tauri::Manager;
        if app.config().identifier != env!("MDREADER_NATIVE_APP_ID")
            || !mdreader_mobile_transfer::available(app)
        {
            return Err(NativeProviderError::Unsupported);
        }
        let directory = crate::reader_profile::directory(app)
            .map_err(|_| NativeProviderError::Storage)?
            .join("reader-v1");
        let owner = backend
            .mobile_owner()
            .map_err(|_| NativeProviderError::Storage)?;
        let profile = owner
            .lock()
            .map_err(|_| NativeProviderError::Busy)?
            .profile_uuid()
            .map_err(core_error)?;
        // App sandbox paths can change on mobile upgrades; durable profile UUID
        // scopes custody without borrowing another profile's grants.
        let ledger = CustodyLedger::open_in_namespace(
            &directory,
            format!("{}.{}", env!("MDREADER_NATIVE_APP_ID"), profile),
        )?;
        let config = compiled_config();
        let vault = VaultClient::new(Arc::new(mdreader_credentials::PlatformVault::new(
            Arc::new(vault::Store {
                app: app.clone(),
                namespace: ledger.namespace.clone(),
            }),
        )))?;
        let transport: Arc<dyn Transport> = Arc::new(ReqwestTransport::new()?);
        let factory = Arc::new(NativeFactory::new(
            vault.clone(),
            config.registrations.clone(),
            transport.clone(),
        )?);
        let library = owner
            .lock()
            .map_err(|_| NativeProviderError::Busy)?
            .with_library_connection(mdreader_service::provider::library_id)?;
        let projection = Arc::new(mdreader_service::provider::CoreProjection::open(
            &directory.join("provider-cache"),
            library,
        )?);
        let coordinator = mdreader_service::provider::Coordinator::new(owner, projection, factory)?;
        Ok(Self {
            config,
            rpc: Arc::new(rpc::EmbeddedRpc {
                coordinator,
                backend,
                slots: Arc::new(Semaphore::new(4)),
            }),
            vault: Some(vault),
            authorizer: Some(Arc::new(MobileAuthorizer {
                app: app.clone(),
                transport,
            })),
            ledger: Mutex::new(Ok(ledger)),
            admission: Semaphore::new(1),
        })
    }
}
