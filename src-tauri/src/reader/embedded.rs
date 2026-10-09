//! Embedded reader composition: one connection owner and one account actor.
//! Account dispatch happens before taking the SQLite mutex, so polling and
//! cancellation never wait on the actor while holding its writer connection.
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
pub(super) struct Owner {
    pub(super) store: Arc<Mutex<mdreader_core::ReaderStore>>,
    #[cfg(mobile)]
    directory: std::path::PathBuf,
    #[cfg(mobile)]
    search: std::sync::OnceLock<Result<mdreader_service::EmbeddedRuntime, String>>,
    #[cfg(mobile)]
    active: std::sync::atomic::AtomicBool,
    #[cfg(mobile)]
    accounts: std::sync::OnceLock<
        Result<
            Arc<mdreader_service::accounts::Coordinator>,
            mdreader_service::accounts::AccountError,
        >,
    >,
}
impl Owner {
    pub(super) fn open(directory: &Path) -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(Mutex::new(
                mdreader_core::ReaderStore::open(directory)
                    .map_err(|_| "Reader storage unavailable")?,
            )),
            #[cfg(mobile)]
            directory: directory.to_path_buf(),
            #[cfg(mobile)]
            search: Default::default(),
            #[cfg(mobile)]
            active: std::sync::atomic::AtomicBool::new(true),
            #[cfg(mobile)]
            accounts: Default::default(),
        })
    }
    #[cfg(mobile)]
    pub(super) fn execute(
        &self,
        request: mdreader_service::Request,
    ) -> Result<serde_json::Value, String> {
        self.search_runtime()?
            .execute(request)
            .map_err(|error| error.to_string())
    }
    #[cfg(mobile)]
    pub(super) fn search_runtime(&self) -> Result<&mdreader_service::EmbeddedRuntime, String> {
        if !self.active.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Reader is suspended; retry when the app is active".into());
        }
        let runtime = self
            .search
            .get_or_init(|| {
                mdreader_service::EmbeddedRuntime::new(
                    self.store.clone(),
                    &self.directory,
                    mdreader_service::ServiceOptions::default(),
                )
                .map_err(|_| "Reader search owner is unavailable".to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        runtime.set_active(self.active.load(std::sync::atomic::Ordering::Acquire));
        Ok(runtime)
    }
    #[cfg(mobile)]
    pub(super) fn set_active(&self, active: bool) {
        self.active
            .store(active, std::sync::atomic::Ordering::Release);
        if let Some(Ok(runtime)) = self.search.get() {
            runtime.set_active(active);
        }
    }
    #[cfg(mobile)]
    pub(super) fn bind_accounts(
        &self,
        accounts: Result<
            Arc<mdreader_service::accounts::Coordinator>,
            mdreader_service::accounts::AccountError,
        >,
    ) -> Result<(), String> {
        self.accounts
            .set(accounts)
            .map_err(|_| "Account owner already initialized".into())
    }
    #[cfg(mobile)]
    pub(super) fn account(
        &self,
        request: mdreader_service::accounts::rpc::AccountRequest,
    ) -> serde_json::Value {
        use mdreader_service::accounts::AccountError;
        let result = match self.accounts.get() {
            Some(Ok(owner)) => owner.execute(request),
            Some(Err(error)) => Err(*error),
            None => Err(AccountError::Unconfigured),
        };
        match result {
            Ok(reply) => serde_json::json!({"result":reply,"error":null}),
            Err(error) => serde_json::json!({"result":null,"error":error}),
        }
    }
}
