use super::*;
use std::collections::HashSet;

/// Discover only authenticated, bounded SHA256 blob dependencies. This never
/// returns titles, asset paths, plaintext or keys; the full graph must still pass
/// prepare before the selected owner can publish it. An opaque deletion has none.
pub fn dependencies(
    scope: &SyncScope,
    revision: &RemoteRevision,
    keys: &[&crate::LibraryKey],
) -> Result<Vec<String>> {
    let (change, plaintext) = authenticated_root(scope, revision, keys)?;
    let Some(plaintext) = plaintext else {
        return Ok(Vec::new());
    };
    let kind: PayloadKind<'_> =
        serde_json::from_slice(&plaintext).map_err(|_| SyncError::Invalid)?;
    match kind.kind {
        "mdreader-content" => {
            let content = codec::decode_content(&plaintext)?;
            if content.document_id != change.item_id || content.revision_id != change.revision_id {
                return Err(SyncError::Scope);
            }
            let blobs = content
                .source
                .chunks
                .iter()
                .chain(content.assets.iter().flat_map(|a| a.chunks.iter()))
                .map(|c| c.blob.clone())
                .collect::<Vec<_>>();
            if blobs.iter().collect::<HashSet<_>>().len() != blobs.len() {
                return Err(SyncError::Invalid);
            }
            Ok(blobs)
        }
        "mdreader-user-state" => {
            let state = codec::decode_user_state(&plaintext)?;
            if state.document_id == change.item_id {
                return Err(SyncError::Scope);
            }
            Ok(Vec::new())
        }
        _ => Err(SyncError::Invalid),
    }
}
