//! Exact reads of "Agent files": Markdown an agent session wrote outside the
//! library. The owner resolves the opaque source id to the one recorded file;
//! these documents are read-only, like captures.
use super::model::{failure, CatalogError, CatalogReply};
use crate::reader::ReaderBackend;
use mdreader_service::{agents::activity::ActivityRequest, Request};

/// `Ok(None)` when `source_id` is not an agent file, so the caller continues
/// with its library and capture scopes.
pub(super) fn read(
    backend: &ReaderBackend,
    source_id: &str,
    path: &str,
    revision: &str,
) -> Result<Option<CatalogReply>, CatalogError> {
    if source_id.starts_with("provider:") {
        return Ok(None);
    }
    let request = Request::AgentActivity {
        request: ActivityRequest::ReadExact {
            source_id: source_id.into(),
            path: path.into(),
            revision: Some(revision.into()),
        },
    };
    #[cfg(not(mobile))]
    let document: Option<mdreader_core::Document> = match backend.call_native(request) {
        Ok(value) => serde_json::from_value(value).map_err(|_| failure("persistence"))?,
        Err(mdreader_core::ReaderError::Conflict) => return Err(failure("stale")),
        // An id that is not an agent file never errors; anything else is a
        // recorded file that is gone, private or unreadable now.
        Err(_) => return Err(failure("unavailable")),
    };
    #[cfg(mobile)]
    let document: Option<mdreader_core::Document> = {
        let _ = (backend, request);
        None
    };
    let Some(document) = document else {
        return Ok(None);
    };
    if document.source_id != source_id || document.path != path || document.revision != revision {
        return Err(failure("stale"));
    }
    let line_end = u32::try_from(document.markdown.lines().count().max(1))
        .map_err(|_| failure("too_large"))?;
    Ok(Some(CatalogReply::ReadDocument {
        source_kind: "capture".into(),
        source_id: document.source_id,
        path: document.path,
        revision: document.revision,
        markdown: document.markdown,
        line_start: 1,
        line_end,
    }))
}
