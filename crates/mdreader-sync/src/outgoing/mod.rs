//! Bounded native ciphertext graph construction, without I/O or publication.
//! The reader owner must persist this exact graph and its selected-document
//! association atomically before any upload. Construction grants no selection.
mod content;
use crate::{PreparedRevision, Result, SyncError};
pub use content::{seal_content, OutgoingAsset, OutgoingContent};

/// Opaque encrypted transport values only. Not Clone/Debug/Serialize: callers
/// must load retries from the durable outbox rather than regenerate a graph.
pub struct SealedGraph {
    pub(crate) root: PreparedRevision,
    pub(crate) resources: Vec<PreparedRevision>,
    pub(crate) key_fingerprint: String,
}
impl SealedGraph {
    pub fn root(&self) -> &PreparedRevision {
        &self.root
    }
    pub fn resources(&self) -> &[PreparedRevision] {
        &self.resources
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.resources.len() > crate::codec::MAX_RESOURCES {
            return Err(SyncError::Bounds);
        }
        let mut blobs = std::collections::HashSet::new();
        crate::crypto::validate_digest(&self.key_fingerprint)?;
        let mut items = std::collections::HashSet::new();
        let mut revisions = std::collections::HashSet::new();
        let mut total = 0usize;
        for body in std::iter::once(&self.root).chain(&self.resources) {
            if body.envelope_utf8.len() > 512 * 1024 {
                return Err(SyncError::Bounds);
            }
            body.context.validate()?;
            if body.context.library_id != self.root.context.library_id
                || body.context.key_id != self.root.context.key_id
                || crate::blob_digest(&body.envelope_utf8) != body.blob_id
                || crate::crypto::envelope_key_id(&body.envelope_utf8)? != body.context.key_id
                || !blobs.insert(&body.blob_id)
                || !items.insert(&body.context.item_id)
                || !revisions.insert(&body.context.revision_id)
            {
                return Err(SyncError::Invalid);
            }
            total = total
                .checked_add(body.envelope_utf8.len())
                .ok_or(SyncError::Bounds)?;
        }
        if total > 32 * 1024 * 1024 {
            return Err(SyncError::Bounds);
        }
        Ok(())
    }
}

/// Standard user-state root using the unchanged payload and envelope/AAD.
pub fn seal_user_state(
    scope: &crate::SyncScope,
    key: &crate::LibraryKey,
    context: &crate::EncryptionContext,
    state: &crate::codec::UserStatePayload,
) -> Result<SealedGraph> {
    context.validate()?;
    if context.library_id != scope.library_id() || context.key_id != key.key_id() {
        return Err(SyncError::Scope);
    }
    let plaintext = zeroize::Zeroizing::new(crate::codec::encode_user_state(state)?);
    let graph = SealedGraph {
        root: crate::encrypt(key, context, &plaintext)?,
        resources: Vec::new(),
        key_fingerprint: key.fingerprint(),
    };
    graph.validate()?;
    Ok(graph)
}
