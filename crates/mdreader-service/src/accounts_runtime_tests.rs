use super::*;
use crate::accounts::{rpc::AccountRequest, AccountError, CacheRoots};

#[test]
fn account_setup_failure_is_typed_and_does_not_disable_local_reader() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(ReaderStore::open(directory.path()).unwrap()));
    let runtime = Runtime::new_with_adapters(
        store.clone(),
        directory.path().join("models"),
        crate::ServiceOptions {
            disable_model: true,
        },
        crate::NativeAdapters {
            factory: None,
            accounts: Some((
                Err(AccountError::IdentityOrScope),
                CacheRoots {
                    documents: directory.path().join("device-markdown-v1"),
                    assets: directory.path().join("device-assets-v1"),
                },
            )),
        },
    )
    .unwrap();
    assert!(runtime.execute(Request::Ping).is_ok());
    let _held = store.lock().unwrap();
    let value = runtime
        .execute(Request::Account {
            request: AccountRequest::Status {},
        })
        .unwrap();
    assert_eq!(value["error"], "identity_or_scope");
    assert!(value["result"].is_null());
    assert!(!directory.path().join("device-markdown-v1").exists());
}
