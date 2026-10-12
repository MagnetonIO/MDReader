//! Exact selected-reference Related uses the existing catalog/device admission.
use crate::{
    reader::ReaderBackend,
    reader_catalog::{CatalogReply, CatalogRequest},
    search::{RelatedDocDto, RelatedOutcomeDto},
};
use mdreader_service::Request;
use serde::Deserialize;
use specta::Type;
use tauri::State;

#[derive(Clone, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct RelatedRequest {
    pub source_id: String,
    pub path: String,
    pub revision: String,
    pub document_id: Option<String>,
}
fn verify(
    app: &tauri::AppHandle,
    backend: &ReaderBackend,
    request: &RelatedRequest,
) -> Result<(), String> {
    if let Some(id) = &request.document_id {
        if request.path != "document.md" {
            return Err("Document changed; reopen it".into());
        }
        let receipt = crate::reader_device::DeviceReceipt {
            document_id: id.clone(),
            source_id: request.source_id.clone(),
            revision: request.revision.clone(),
        };
        crate::reader_device::request(
            backend,
            crate::reader_device::DeviceRequest::Read { receipt },
        )
        .map_err(|_| "Document changed; reopen it")?;
    } else {
        let result = crate::reader_catalog::execute(
            app,
            backend,
            CatalogRequest::Read {
                source_id: request.source_id.clone(),
                path: request.path.clone(),
                revision: request.revision.clone(),
            },
        )
        .map_err(|_| "Document changed; reopen it")?;
        if !matches!(result, CatalogReply::ReadDocument { .. }) {
            return Err("Document unavailable".into());
        }
    }
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub async fn reader_related_request(
    app: tauri::AppHandle,
    request: RelatedRequest,
    reader: State<'_, ReaderBackend>,
) -> Result<RelatedOutcomeDto, String> {
    let backend = reader.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        verify(&app, &backend, &request)?;
        let result: mdreader_core::RelatedRanked = backend.typed(Request::RelatedRanked {
            source_id: request.source_id.clone(),
            path: request.path.clone(),
            query: None,
            limit: 12,
        })?;
        verify(&app, &backend, &request)?;
        let kind = if request.document_id.is_some() {
            "device"
        } else if request.source_id.starts_with("provider:") {
            "provider"
        } else if crate::reader_owned::capture_selected(&backend, &request.source_id)? {
            "capture"
        } else {
            "local"
        };
        let hits = result
            .hits
            .into_iter()
            .map(|hit| RelatedDocDto {
                source_kind: Some(kind.into()),
                device_document_id: None,
                graph_direction: hit.graph_direction,
                source_id: Some(hit.source_id.clone()),
                revision: Some(hit.revision),
                line: Some(hit.line),
                document_id: uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_URL,
                    format!("mdreader:{}:{}", hit.source_id, hit.path).as_bytes(),
                )
                .to_string(),
                abs_path: hit.path.clone(),
                name: std::path::Path::new(&hit.path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&hit.path)
                    .into(),
                file_path: hit.path,
                score: hit.score as f32,
                components: crate::search::RelatedComponentsDto {
                    graph: hit.components.graph as f32,
                    concept: 0.0,
                    vector: hit.components.vector.map(|v| v as f32),
                    shared_tags: Some(hit.components.shared_tags as f32),
                },
            })
            .collect();
        Ok(RelatedOutcomeDto {
            hits,
            availability: result.availability.into(),
        })
    })
    .await
    .map_err(|_| "Related notes unavailable".to_string())?
}
