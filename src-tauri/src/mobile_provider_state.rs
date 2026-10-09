//! Thin mobile adapter over the existing embedded provider owner. No remote edits.
use mdreader_core::provider_reading_state as core;
use serde::{Deserialize, Serialize};
use specta::Type;
#[derive(Clone, Deserialize, Serialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderReadingReference {
    pub source_id: String,
    pub path: String,
    pub revision: String,
}
impl ProviderReadingReference {
    pub(crate) fn core(&self) -> Result<core::Reference, String> {
        let value = core::Reference {
            source_id: self.source_id.clone(),
            path: self.path.clone(),
            revision: self.revision.clone(),
        };
        value.validate().map_err(|error| error.to_string())?;
        Ok(value)
    }
}
#[derive(Deserialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderReadingRequest {
    State {
        reference: ProviderReadingReference,
    },
    SetState {
        reference: ProviderReadingReference,
        expected_generation: String,
        pinned: Option<bool>,
        line: Option<u32>,
    },
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReadingState {
    pub reference: ProviderReadingReference,
    pub pinned: bool,
    pub line: u32,
    pub line_end: u32,
    pub generation: String,
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_provider_reading_state(
    backend: tauri::State<'_, crate::reader::ReaderBackend>,
    request: ProviderReadingRequest,
) -> Result<ProviderReadingState, String> {
    if !cfg!(mobile) {
        return Err("unsupported".into());
    }
    let backend = backend.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        backend.with_mobile_store(|store| {
            let value = match request {
                ProviderReadingRequest::State { reference } => {
                    core::read(store, &reference.core()?).map_err(|error| error.to_string())?
                }
                ProviderReadingRequest::SetState {
                    reference,
                    expected_generation,
                    pinned,
                    line,
                } => {
                    let expected: u64 = expected_generation.parse().map_err(|_| "invalid")?;
                    if expected.to_string() != expected_generation {
                        return Err("invalid".into());
                    }
                    core::update(store, &reference.core()?, expected, pinned, line)
                        .map_err(|error| error.to_string())?
                }
            };
            Ok(ProviderReadingState {
                reference: ProviderReadingReference {
                    source_id: value.reference.source_id,
                    path: value.reference.path,
                    revision: value.reference.revision,
                },
                pinned: value.pinned,
                line: value.line,
                line_end: value.line_end,
                generation: value.generation.to_string(),
            })
        })
    })
    .await
    .map_err(|_| "unavailable".to_string())?
}
