use super::super::*;
pub(super) struct EmbeddedRpc {
    pub coordinator: mdreader_service::provider::Coordinator,
    pub backend: ReaderBackend,
    pub slots: Arc<Semaphore>,
}
impl EmbeddedRpc {
    async fn retained<T: Send + 'static>(
        &self,
        action: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| NativeProviderError::Busy)?;
        let task = tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            action()
        });
        tokio::time::timeout(RPC_BUDGET, task)
            .await
            .map_err(|_| NativeProviderError::Timeout)?
            .map_err(|_| NativeProviderError::ServiceUnavailable)?
    }
}
#[async_trait::async_trait]
impl Rpc for EmbeddedRpc {
    async fn provider(&self, request: ProviderRequest) -> Result<Value> {
        let coordinator = self.coordinator.clone();
        self.retained(move || coordinator.execute(request).map_err(Into::into))
            .await
    }
    async fn read(&self, projection: &ProjectionDto) -> Result<mdreader_core::Document> {
        let backend = self.backend.clone();
        let projection = projection.clone();
        self.retained(move || {
            let owner = backend
                .mobile_owner()
                .map_err(|_| NativeProviderError::Storage)?;
            let store = owner.lock().map_err(|_| NativeProviderError::Busy)?;
            store
                .read_document(
                    &projection.reader_source_id,
                    &projection.path,
                    Some(&projection.revision),
                )
                .map_err(core_error)
        })
        .await
    }
    async fn read_current(&self, projection: &ProjectionDto) -> Result<mdreader_core::Document> {
        let backend = self.backend.clone();
        let projection = projection.clone();
        self.retained(move || {
            let owner = backend
                .mobile_owner()
                .map_err(|_| NativeProviderError::Storage)?;
            let store = owner.lock().map_err(|_| NativeProviderError::Busy)?;
            store
                .read_document(&projection.reader_source_id, &projection.path, None)
                .map_err(core_error)
        })
        .await
    }
}
