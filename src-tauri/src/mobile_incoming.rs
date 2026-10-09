//! Native selected-stream admission: durable owner publication precedes queue
//! acknowledgement. Retry the same queue UUID/body after a lost acknowledgement.
use crate::{mobile_reader::DeviceDocument, reader::ReaderBackend};
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::device;
use mdreader_mobile_transfer::{acknowledge_incoming, claim_incoming, next_incoming, IncomingItem};
use serde::Serialize;

#[derive(Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum IncomingFailure {
    InvalidSelection,
    UnreadableSelection,
    SelectionLimit,
    ImportUnavailable,
    QueueUnavailable,
}
#[derive(Serialize, specta::Type)]
pub struct IncomingReply {
    pub document: Option<DeviceDocument>,
    pub remaining: u32,
    pub error: Option<IncomingFailure>,
    pub retained: bool,
    pub open: bool,
}
impl IncomingReply {
    fn failed(error: IncomingFailure, remaining: u32, retained: bool) -> Self {
        Self {
            document: None,
            remaining,
            error: Some(error),
            retained,
            open: false,
        }
    }
}
fn selected(item: IncomingItem) -> Result<(uuid::Uuid, String, String), IncomingFailure> {
    let id = uuid::Uuid::parse_str(&item.id).map_err(|_| IncomingFailure::InvalidSelection)?;
    if id.get_version_num() != 4
        || id.to_string() != item.id
        || item.failure.is_some()
        || item.bytes > 1024 * 1024
    {
        return Err(IncomingFailure::InvalidSelection);
    }
    let encoded = item.base64.ok_or(IncomingFailure::InvalidSelection)?;
    if encoded.len() > (1024 * 1024 + 2) / 3 * 4 {
        return Err(IncomingFailure::SelectionLimit);
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| IncomingFailure::InvalidSelection)?;
    if bytes.len() != item.bytes as usize {
        return Err(IncomingFailure::InvalidSelection);
    }
    let content = String::from_utf8(bytes).map_err(|_| IncomingFailure::UnreadableSelection)?;
    let name = item.name.ok_or(IncomingFailure::InvalidSelection)?;
    if name.is_empty()
        || name.len() > 240
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        || !(name.to_lowercase().ends_with(".md") || name.to_lowercase().ends_with(".markdown"))
    {
        return Err(IncomingFailure::InvalidSelection);
    }
    Ok((id, name, content))
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_import_incoming(
    app: tauri::AppHandle,
    backend: tauri::State<'_, ReaderBackend>,
) -> Result<IncomingReply, String> {
    let _lease = claim_incoming(&app)
        .map_err(|_| "Incoming selection is busy or unavailable".to_string())?;
    let queue = next_incoming(&app).await.map_err(|_| {
        "Incoming selection could not be read. Retry to keep the selected copy.".to_string()
    })?;
    if queue.remaining > 21 {
        return Err("Incoming selection exceeds the queue limit".into());
    }
    let Some(item) = queue.item else {
        if queue.remaining != 0 {
            return Err("Incoming selection could not be verified".into());
        }
        return Ok(IncomingReply {
            document: None,
            remaining: 0,
            error: None,
            retained: false,
            open: false,
        });
    };
    if queue.remaining == 0 {
        return Err("Incoming selection could not be verified".into());
    }
    let identity = item.id.clone();
    let id = uuid::Uuid::parse_str(&identity)
        .map_err(|_| "Incoming selection could not be verified".to_string())?;
    if id.get_version_num() != 4 || id.to_string() != identity {
        return Err("Incoming selection could not be verified".into());
    }
    if let Some(failure) = item.failure.as_deref() {
        let failure = match failure {
            "invalid_selection" => IncomingFailure::InvalidSelection,
            "unreadable_selection" => IncomingFailure::UnreadableSelection,
            "selection_limit" => IncomingFailure::SelectionLimit,
            _ => return Err("Incoming selection could not be verified".into()),
        };
        let acknowledged = acknowledge_incoming(&app, &identity).await.is_ok();
        return Ok(IncomingReply::failed(
            failure,
            queue.remaining - u32::from(acknowledged),
            !acknowledged,
        ));
    }
    let open = item.open;
    let (id, name, content) = match selected(item) {
        Ok(value) => value,
        Err(error) => return Ok(IncomingReply::failed(error, queue.remaining, true)),
    };
    let directory = crate::reader_profile::directory(&app)
        .map_err(|_| "Device library directory is unavailable".to_string())?
        .join("device-markdown-v1");
    let owner = backend.inner().clone();
    let imported = tauri::async_runtime::spawn_blocking(move || {
        owner.with_mobile_store(|store| {
            let document = device::import_selected(store, &directory, id, name, content)
                .map_err(|_| "Selected copy could not be imported".to_string())?;
            // The Markdown publication is already durable. Index readiness is
            // separate; its failure must not duplicate or lose this selection.
            let _ = store.queue_index(&document.source_id);
            Ok(DeviceDocument::from(document))
        })
    })
    .await;
    let document = match imported {
        Ok(Ok(document)) => document,
        _ => {
            return Ok(IncomingReply::failed(
                IncomingFailure::ImportUnavailable,
                queue.remaining,
                true,
            ))
        }
    };
    let acknowledged = acknowledge_incoming(&app, &identity).await.is_ok();
    Ok(IncomingReply {
        document: Some(document),
        remaining: queue.remaining - u32::from(acknowledged),
        error: (!acknowledged).then_some(IncomingFailure::QueueUnavailable),
        retained: !acknowledged,
        open,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_stream_preserves_bom_and_crlf_and_rejects_bad_names_or_bytes() {
        let body = "\u{feff}# Imported\r\n";
        let item = || IncomingItem {
            id: uuid::Uuid::new_v4().to_string(),
            name: Some("note.markdown".into()),
            bytes: body.len() as u32,
            base64: Some(STANDARD.encode(body)),
            failure: None,
            open: true,
        };
        assert_eq!(selected(item()).ok().unwrap().2, body);
        let mut invalid = item();
        invalid.name = Some("../note.md".into());
        assert!(selected(invalid).is_err());
        let mut invalid = item();
        invalid.bytes += 1;
        assert!(selected(invalid).is_err());
        let mut invalid = item();
        invalid.base64 = Some(STANDARD.encode([0xff]));
        invalid.bytes = 1;
        assert!(selected(invalid).is_err());
    }
}
