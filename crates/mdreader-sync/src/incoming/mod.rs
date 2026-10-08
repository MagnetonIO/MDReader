//! Native-only, bounded preparation of authenticated encrypted incoming payloads.
//! This module performs no I/O or persistence and cannot advance an applied cursor.
//! Preparation is not an issuer/device/selection grant or a local projection receipt.
use crate::{
    codec::{self, UserStatePayload},
    RemoteChange, RemoteRevision, Result, SyncError, SyncScope,
};
use serde::Deserialize;
use zeroize::Zeroizing;

mod graph;
mod raster;
mod resources;
pub use graph::dependencies;
use resources::{decrypt_blob, Resources};

/// Already downloaded untrusted ciphertext. No path or arbitrary file access is accepted.
pub struct EncryptedResource<'a> {
    pub blob_id: &'a str,
    pub envelope_utf8: &'a [u8],
}
/// Private construction prevents accidental bypass of preparation. Deliberately neither
/// Debug nor Serialize: plaintext must not enter transport diagnostics or renderer RPC.
pub struct PreparedIncoming {
    scope: SyncScope,
    change: RemoteChange,
    payload: IncomingPayload,
}
pub enum IncomingPayload {
    Content(PreparedContent),
    UserState(UserStatePayload),
    /// The wire deletion has no payload kind. Only a durable owner item association may
    /// decide whether it deletes content or user state; an unknown item stays unresolved.
    OpaqueTombstone,
}
pub struct PreparedContent {
    title: Zeroizing<String>,
    markdown: Zeroizing<Vec<u8>>,
    sha256: String,
    assets: Vec<PreparedAsset>,
}
pub struct PreparedAsset {
    path: Zeroizing<String>,
    mime: String,
    sha256: String,
    bytes: Zeroizing<Vec<u8>>,
}
impl PreparedIncoming {
    pub fn scope(&self) -> &SyncScope {
        &self.scope
    }
    pub fn change(&self) -> &RemoteChange {
        &self.change
    }
    pub fn payload(&self) -> &IncomingPayload {
        &self.payload
    }
}
impl PreparedContent {
    pub fn title(&self) -> &str {
        &self.title
    }
    /// Exact validated UTF-8 bytes, including any BOM and original line endings.
    pub fn markdown(&self) -> &[u8] {
        &self.markdown
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn assets(&self) -> &[PreparedAsset] {
        &self.assets
    }
}
impl PreparedAsset {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn mime(&self) -> &str {
        &self.mime
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
#[derive(Deserialize)]
struct PayloadKind<'a> {
    kind: &'a str,
}

/// Native caller supplies its current scope, staged root, custody-held keys and exact
/// downloaded dependencies. A verified transport/session and owner selection barrier
/// are still required before publication; this function cannot establish those grants.
/// Missing resources/keys return NotReady, mismatches fail closed, and no partial value
/// escapes. All resources must be referenced once; input order has no authority.
pub fn prepare(
    scope: &SyncScope,
    revision: &RemoteRevision,
    keys: &[&crate::LibraryKey],
    resources: &[EncryptedResource<'_>],
) -> Result<PreparedIncoming> {
    let (change, plaintext) = authenticated_root(scope, revision, keys)?;
    let payload = if let Some(plaintext) = plaintext {
        let kind: PayloadKind<'_> =
            serde_json::from_slice(&plaintext).map_err(|_| SyncError::Invalid)?;
        match kind.kind {
            "mdreader-content" => {
                let content = codec::decode_content(&plaintext)?;
                if content.document_id != change.item_id
                    || content.revision_id != change.revision_id
                {
                    return Err(SyncError::Scope);
                }
                let mut bodies = Resources::new(resources)?;
                let markdown = bodies.assemble(scope, &content, None, keys)?;
                std::str::from_utf8(&markdown).map_err(|_| SyncError::Invalid)?;
                let mut assets = Vec::with_capacity(content.assets.len());
                for descriptor in &content.assets {
                    let bytes = bodies.assemble(scope, &content, Some(descriptor), keys)?;
                    raster::validate(&bytes, &descriptor.mime)?;
                    assets.push(PreparedAsset {
                        path: Zeroizing::new(descriptor.path.clone()),
                        mime: descriptor.mime.clone(),
                        sha256: descriptor.sha256.clone(),
                        bytes,
                    });
                }
                bodies.finish()?;
                assets.sort_by(|a, b| a.path.cmp(&b.path));
                IncomingPayload::Content(PreparedContent {
                    title: Zeroizing::new(content.title),
                    markdown,
                    sha256: content.source.sha256,
                    assets,
                })
            }
            "mdreader-user-state" => {
                if !resources.is_empty() {
                    return Err(SyncError::Invalid);
                }
                let state = codec::decode_user_state(&plaintext)?;
                if state.document_id == change.item_id {
                    return Err(SyncError::Scope);
                }
                IncomingPayload::UserState(state)
            }
            _ => return Err(SyncError::Invalid),
        }
    } else {
        if !resources.is_empty() {
            return Err(SyncError::Invalid);
        }
        IncomingPayload::OpaqueTombstone
    };
    Ok(PreparedIncoming {
        scope: scope.clone(),
        change,
        payload,
    })
}

/// Shared authentication admission for graph discovery and full preparation.
fn authenticated_root(
    scope: &SyncScope,
    revision: &RemoteRevision,
    keys: &[&crate::LibraryKey],
) -> Result<(RemoteChange, Option<Zeroizing<Vec<u8>>>)> {
    let mut change = revision.change.clone();
    change.validate()?;
    if keys.len() > 16 {
        return Err(SyncError::Bounds);
    }
    let mut ids = std::collections::HashSet::new();
    if keys.iter().any(|key| !ids.insert(key.key_id())) {
        return Err(SyncError::Invalid);
    }
    let plaintext = if change.deleted {
        if revision.envelope_utf8.is_some() {
            return Err(SyncError::Invalid);
        }
        None
    } else {
        let blob = change.blob.as_deref().ok_or(SyncError::Invalid)?;
        let envelope = revision
            .envelope_utf8
            .as_deref()
            .ok_or(SyncError::NotReady)?;
        Some(decrypt_blob(
            scope.library_id(),
            &change.item_id,
            &change.revision_id,
            blob,
            envelope,
            keys,
        )?)
    };
    Ok((change, plaintext))
}
