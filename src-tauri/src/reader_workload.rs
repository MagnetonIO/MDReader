//! User-visible background inference policy; no host paths or tuning knobs.
use crate::reader::ReaderBackend;
use mdreader_service::{workload::EmbeddingPolicy, Request};
#[tauri::command]
#[specta::specta]
pub async fn reader_embedding_policy(
    backend: tauri::State<'_, ReaderBackend>,
) -> Result<String, String> {
    backend.invoke(Request::EmbeddingPolicy).await
}
#[tauri::command]
#[specta::specta]
pub async fn set_reader_embedding_policy(
    backend: tauri::State<'_, ReaderBackend>,
    policy: String,
) -> Result<String, String> {
    let policy: EmbeddingPolicy = serde_json::from_value(serde_json::Value::String(policy))
        .map_err(|_| "Unknown background indexing policy".to_string())?;
    backend.invoke(Request::SetEmbeddingPolicy { policy }).await
}
