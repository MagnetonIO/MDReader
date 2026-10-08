//! Explicit selected-document association; never a second document/file owner.
//! Queue admission is native-only until self-echo/dirty/tombstone acceptance passes.
use super::incoming::{Error, Result};
use mdreader_core::device::publication::IncomingOrigin;
use mdreader_sync::{CommitOperation, SyncScope};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(in crate::sync) mod association;
pub(in crate::sync) mod schema;
mod selected;
pub(in crate::sync) mod snapshot;
pub use selected::SelectedDisposition;
pub(crate) use selected::{matches_selected, validate_selected};

/// Opaque native operation result. No body, private path, auth token or key.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedDocument {
    pub operation_id: Uuid,
    pub wire_item_id: Uuid,
    pub wire_revision: Uuid,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::sync) struct Intent {
    pub operation: CommitOperation,
    pub native_document: String,
    pub native_revision: String,
    pub generation: u64,
    pub markdown_sha256: String,
    pub assets_sha256: String,
    pub origin: IncomingOrigin,
    pub head: Option<super::incoming::Head>,
}
impl Intent {
    pub fn validate(&self, scope: &SyncScope) -> Result<()> {
        self.operation.wire_bytes()?;
        if self.operation.deleted
            || self.operation.blob.is_none()
            || self.native_document.len() != 36
            || !Uuid::parse_str(&self.native_document)
                .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == self.native_document)
            || self.generation > 9_007_199_254_740_991
            || self.origin.item_id.to_string() != self.operation.item_id
            || self.origin.wire_revision.to_string() != self.operation.revision_id
            || self.origin.session_epoch.get_version_num() != 4
            || self.origin.selection_epoch.get_version_num() != 4
            || self.origin.scope_digest != scope_digest(scope)?
        {
            return Err(Error::Scope);
        }
        for hash in [
            &self.native_revision,
            &self.markdown_sha256,
            &self.assets_sha256,
        ] {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::Scope);
            }
        }
        if let Some(head) = &self.head {
            if head.document != self.native_document
                || head.item != self.operation.item_id
                || head.kind != "content"
                || head.target.is_some()
                || head.tombstone.is_some()
                || head.generation.is_none_or(|n| n > 9_007_199_254_740_991)
                || self.operation.parents != [head.revision.clone()]
            {
                return Err(Error::Conflict);
            }
        } else if !self.operation.parents.is_empty() {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub fn encode(&self, scope: &SyncScope) -> Result<String> {
        self.validate(scope)?;
        let encoded = serde_json::to_string(self)?;
        if encoded.len() > 4096 {
            return Err(Error::Limit);
        }
        Ok(encoded)
    }
    pub fn result(&self) -> Result<QueuedDocument> {
        Ok(QueuedDocument {
            operation_id: Uuid::parse_str(&self.operation.operation_id)
                .map_err(|_| Error::Schema)?,
            wire_item_id: Uuid::parse_str(&self.operation.item_id).map_err(|_| Error::Schema)?,
            wire_revision: Uuid::parse_str(&self.operation.revision_id)
                .map_err(|_| Error::Schema)?,
        })
    }
}
pub(in crate::sync) fn scope_digest(scope: &SyncScope) -> Result<[u8; 32]> {
    let key = super::incoming::scope_key(scope)?;
    let mut digest = [0; 32];
    for (i, b) in digest.iter_mut().enumerate() {
        *b = u8::from_str_radix(&key[i * 2..i * 2 + 2], 16).map_err(|_| Error::Scope)?;
    }
    Ok(digest)
}
