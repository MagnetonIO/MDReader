//! Thin mobile search commands; consent, retrieval and model work belong to the
//! existing embedded owner. Commands never hold its SQLite writer during ONNX.
use crate::{mobile_reader::DeviceHit, reader::ReaderBackend};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSearchMode {
    Text,
    Hybrid,
    Semantic,
}
#[derive(Serialize, specta::Type)]
pub struct DeviceSearchSettings {
    pub enabled: bool,
    pub model: String,
    pub documents: u32,
    pub embedded_documents: u32,
}
impl From<mdreader_service::DeviceSearchStatus> for DeviceSearchSettings {
    fn from(value: mdreader_service::DeviceSearchStatus) -> Self {
        Self {
            enabled: value.enabled,
            model: value.model.into(),
            documents: value.documents as u32,
            embedded_documents: value.embedded_documents as u32,
        }
    }
}
#[derive(Serialize, specta::Type)]
pub struct DeviceSearchResults {
    pub hits: Vec<DeviceHit>,
    pub semantic_unavailable: Option<String>,
}
async fn invoke<T: Send + 'static>(
    backend: ReaderBackend,
    operation: impl FnOnce(&mdreader_service::EmbeddedRuntime) -> mdreader_core::Result<T>
        + Send
        + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || backend.with_mobile_search(operation))
        .await
        .map_err(|_| "Device search worker failed".to_string())?
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_search_settings(
    backend: tauri::State<'_, ReaderBackend>,
) -> Result<DeviceSearchSettings, String> {
    invoke(backend.inner().clone(), |owner| {
        owner.device_search_status().map(Into::into)
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_search_set_enabled(
    backend: tauri::State<'_, ReaderBackend>,
    enabled: bool,
) -> Result<DeviceSearchSettings, String> {
    invoke(backend.inner().clone(), move |owner| {
        owner.set_device_semantic_enabled(enabled).map(Into::into)
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_search_query(
    backend: tauri::State<'_, ReaderBackend>,
    query: String,
    mode: DeviceSearchMode,
) -> Result<DeviceSearchResults, String> {
    invoke(backend.inner().clone(), move |owner| {
        let mode = match mode {
            DeviceSearchMode::Text => mdreader_service::SearchMode::Exact,
            DeviceSearchMode::Hybrid => mdreader_service::SearchMode::Hybrid,
            DeviceSearchMode::Semantic => mdreader_service::SearchMode::Semantic,
        };
        owner
            .search_device(&query, mode)
            .map(|value| DeviceSearchResults {
                hits: value.hits.into_iter().map(Into::into).collect(),
                semantic_unavailable: value.semantic_unavailable,
            })
    })
    .await
}
