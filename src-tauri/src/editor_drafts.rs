//! Thin private checkpoint adapter. No file reads, grants, or remote writes.
use crate::reader::ReaderBackend;
use mdreader_core::editor_drafts::DraftRequest;
use mdreader_service::Request;
use serde_json::Value;
use tauri::State;
/// Reserve room for the authenticated IPC envelope and escaping of its token.
const MAX_JSON_REQUEST: usize = 2 * 1024 * 1024 - 4096;
#[tauri::command]
#[specta::specta]
pub async fn native_editor_drafts(
    request: Value,
    reader: State<'_, ReaderBackend>,
) -> Result<Value, String> {
    let request = checked_request(request)?;
    reader.invoke(Request::EditorDrafts { request }).await
}
fn checked_request(request: Value) -> Result<DraftRequest, String> {
    let encoded = serde_json::to_vec(&request).map_err(|_| "Draft checkpoint unavailable")?;
    if encoded.len() > MAX_JSON_REQUEST {
        return Err("Draft checkpoint exceeds the local storage transport limit; keep the buffer or save a copy".into());
    }
    serde_json::from_value(request).map_err(|_| "Draft request is invalid".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaped_body_is_bounded_before_ipc_not_only_by_utf8_length() {
        let request = serde_json::json!({"operation":"checkpoint","key":{"kind":"device","document_id":"public"},"expected_generation":0,"draft":{"base_revision":"original","base_hash":null,"title":"Public.md","markdown":"\u{0}".repeat(400_000)}});
        assert!(checked_request(request).is_err());
        let request = serde_json::json!({"operation":"checkpoint","key":{"kind":"device","document_id":"public"},"expected_generation":0,"draft":{"base_revision":"original","base_hash":null,"title":"Public.md","markdown":"\u{0}\"\\\n"}});
        let DraftRequest::Checkpoint { draft, .. } = checked_request(request).unwrap() else {
            panic!("checkpoint")
        };
        assert_eq!(draft.markdown, "\u{0}\"\\\n");
    }
}
