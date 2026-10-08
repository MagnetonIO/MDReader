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
            accounts: Default::default(),
        })
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
