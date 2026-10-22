//! Reader-owned capture/device membership comes from the same service owner,
//! never from a caller's filesystem path or a second registry.
use crate::reader::ReaderBackend;
use mdreader_service::{catalog::ReaderOwnedSources, Request};
pub(crate) fn capture_selected(reader: &ReaderBackend, id: &str) -> Result<bool, String> {
    let sources: Vec<mdreader_core::Source> = reader.typed(Request::CaptureReaderSources)?;
    Ok(sources.iter().any(|s| s.id == id && s.enabled))
}
pub(crate) fn same(a: &ReaderOwnedSources, b: &ReaderOwnedSources) -> bool {
    let captured = |sources: &ReaderOwnedSources| {
        sources
            .captures
            .iter()
            .map(|s| (s.id.clone(), s.enabled))
            .collect::<std::collections::BTreeSet<_>>()
    };
    captured(a) == captured(b) && a.devices == b.devices && a.agent_files == b.agent_files
}
