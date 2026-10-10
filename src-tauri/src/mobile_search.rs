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
    pub indexed_documents: u32,
    pub lexical_snapshot: String,
    pub embedded_documents: u32,
}
impl TryFrom<mdreader_service::DeviceSearchStatus> for DeviceSearchSettings {
    type Error = mdreader_core::ReaderError;
    fn try_from(value: mdreader_service::DeviceSearchStatus) -> Result<Self, Self::Error> {
        if value.documents > 1000
            || value.indexed_documents > value.documents
            || value.embedded_documents > value.indexed_documents
            || value.lexical_snapshot.len() != 64
            || !value
                .lexical_snapshot
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(mdreader_core::ReaderError::Invalid(
                "Invalid device search readiness".into(),
            ));
        }
        Ok(Self {
            enabled: value.enabled,
            model: value.model.into(),
            documents: value.documents as u32,
            indexed_documents: value.indexed_documents as u32,
            lexical_snapshot: value.lexical_snapshot,
            embedded_documents: value.embedded_documents as u32,
        })
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
        owner.device_search_status().and_then(TryInto::try_into)
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
        owner
            .set_device_semantic_enabled(enabled)
            .and_then(TryInto::try_into)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> mdreader_service::DeviceSearchStatus {
        mdreader_service::DeviceSearchStatus {
            enabled: false,
            model: "disabled",
            documents: 1000,
            indexed_documents: 999,
            lexical_snapshot: "a".repeat(64),
            embedded_documents: 0,
        }
    }

    #[test]
    fn readiness_transport_is_bounded_and_retains_exact_lexical_fields() {
        let actual = DeviceSearchSettings::try_from(status()).unwrap();
        assert_eq!(actual.documents, 1000);
        assert_eq!(actual.indexed_documents, 999);
        assert_eq!(actual.lexical_snapshot, "a".repeat(64));
        assert!(!actual.enabled);
        let mut invalid = status();
        invalid.documents = 1001;
        assert!(DeviceSearchSettings::try_from(invalid).is_err());
        let mut invalid = status();
        invalid.indexed_documents = 1001;
        assert!(DeviceSearchSettings::try_from(invalid).is_err());
        let mut invalid = status();
        invalid.embedded_documents = 1000;
        assert!(DeviceSearchSettings::try_from(invalid).is_err());
        for token in ["A".repeat(64), "a".repeat(63), "g".repeat(64)] {
            let mut invalid = status();
            invalid.lexical_snapshot = token;
            assert!(DeviceSearchSettings::try_from(invalid).is_err());
        }
    }
}
