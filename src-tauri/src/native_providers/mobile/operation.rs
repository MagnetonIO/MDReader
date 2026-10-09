//! Mobile work is retained after renderer cancellation; there is no desktop freeze capability.
use super::*;
use std::future::Future;
pub(super) async fn run<T: Send + 'static>(
    _app: &tauri::AppHandle,
    action: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn(action)
        .await
        .map_err(|_| NativeProviderError::ServiceUnavailable)?
}
