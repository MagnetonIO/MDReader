//! Thin read-only service adapter. No second device owner or GUI asset runtime.
mod model;
use crate::reader::ReaderBackend;
use mdreader_service::{device as service, Request};
pub use model::*;
use serde::Deserialize;
use tauri::State;

#[derive(Deserialize)]
struct Envelope {
    result: Option<service::DeviceReply>,
    error: Option<service::DeviceError>,
}
fn request(reader: &ReaderBackend, operation: DeviceRequest) -> Result<DeviceReply, DeviceError> {
    let response: Envelope = reader
        .typed(Request::Device {
            request: operation.service()?,
        })
        .map_err(|_| model::unavailable())?;
    match (response.result, response.error) {
        (Some(value), None) => DeviceReply::project(value),
        (None, Some(error)) => Err(error.into()),
        _ => Err(model::unavailable()),
    }
}
#[tauri::command]
#[specta::specta]
pub async fn reader_device(
    operation: DeviceRequest,
    reader: State<'_, ReaderBackend>,
) -> Result<DeviceReply, DeviceError> {
    let backend = reader.inner().clone();
    tauri::async_runtime::spawn_blocking(move || request(&backend, operation))
        .await
        .map_err(|_| model::unavailable())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_read_inputs_and_projection_do_not_expose_paths_or_native_handles() {
        assert!(serde_json::from_value::<DeviceRequest>(serde_json::json!({"operation":"close","sessionId":uuid::Uuid::new_v4(),"token":"extra"})).is_err());
        assert!(serde_json::from_value::<DeviceRequest>(serde_json::json!({"operation":"read","receipt":{"documentId":uuid::Uuid::new_v4(),"sourceId":"a".repeat(64),"revision":"b".repeat(64),"path":"/private"}})).is_err());
        let reply = DeviceReply::project(service::DeviceReply::ReadDocument {
            receipt: mdreader_core::device::catalog::Receipt {
                document_id: uuid::Uuid::new_v4().to_string(),
                source_id: "a".repeat(64),
                revision: "b".repeat(64),
            },
            name: "synthetic.md".into(),
            markdown: "# Exact\n\nBody".into(),
            line: 1,
        })
        .unwrap();
        let value = serde_json::to_value(reply).unwrap();
        assert_eq!(value["path"], "document.md");
        assert_eq!(value["lineEnd"], 3);
        assert!(value.get("root").is_none());
    }
}
