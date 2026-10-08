use super::*;
use crate::{codec, encrypt, EncryptionContext, LibraryKey, SyncScope};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zeroize::{Zeroize, Zeroizing};

/// Borrowed, explicitly selected native owner snapshot. No file paths or lookup.
pub struct OutgoingContent<'a> {
    pub title: &'a str,
    pub markdown: &'a [u8],
    pub assets: &'a [OutgoingAsset<'a>],
}
pub struct OutgoingAsset<'a> {
    pub path: &'a str,
    pub mime: &'a str,
    pub bytes: &'a [u8],
}

/// Existing payload/envelope/AAD only. Random chunk identities and ciphertext
/// remain in this graph; retries may never call this after first publication.
pub fn seal_content(
    scope: &SyncScope,
    key: &LibraryKey,
    context: &EncryptionContext,
    input: OutgoingContent<'_>,
) -> Result<SealedGraph> {
    context.validate()?;
    if context.library_id != scope.library_id() || context.key_id != key.key_id() {
        return Err(SyncError::Scope);
    }
    if input.markdown.len() > 1024 * 1024
        || input.assets.len() > 8
        || input.title.len() > 2048
        || input
            .assets
            .iter()
            .any(|a| a.bytes.len() > 4 * 1024 * 1024 || a.path.len() > 2048)
        || input
            .assets
            .iter()
            .try_fold(0usize, |total, a| total.checked_add(a.bytes.len()))
            .is_none_or(|total| total > 16 * 1024 * 1024)
    {
        return Err(SyncError::Bounds);
    }
    if input
        .assets
        .iter()
        .any(|a| !["image/png", "image/jpeg"].contains(&a.mime))
    {
        return Err(SyncError::Invalid);
    }
    std::str::from_utf8(input.markdown).map_err(|_| SyncError::Invalid)?;
    let mut resources = Vec::new();
    let source = chunks(key, context, None, input.markdown, &mut resources)?;
    let mut assets = Vec::with_capacity(input.assets.len());
    for asset in input.assets {
        let descriptor = chunks(key, context, Some(asset.path), asset.bytes, &mut resources)?;
        assets.push(codec::AssetDescriptor {
            path: asset.path.into(),
            mime: asset.mime.into(),
            bytes: descriptor.bytes,
            sha256: descriptor.sha256,
            chunks: descriptor.chunks,
        });
    }
    let mut payload = codec::ContentPayload {
        v: 1,
        kind: "mdreader-content".into(),
        document_id: context.item_id.clone(),
        revision_id: context.revision_id.clone(),
        title: input.title.into(),
        source,
        assets,
    };
    let encoded = codec::encode_content(&payload);
    payload.title.zeroize();
    for asset in &mut payload.assets {
        asset.path.zeroize();
    }
    let plaintext = Zeroizing::new(encoded?);
    let graph = SealedGraph {
        root: encrypt(key, context, &plaintext)?,
        resources,
        key_fingerprint: key.fingerprint(),
    };
    graph.validate()?;
    Ok(graph)
}

fn chunks(
    key: &LibraryKey,
    root: &EncryptionContext,
    path: Option<&str>,
    bytes: &[u8],
    encrypted: &mut Vec<PreparedRevision>,
) -> Result<codec::ResourceDescriptor> {
    let count = bytes.len().div_ceil(codec::CHUNK_BYTES).max(1);
    if encrypted.len().saturating_add(count) > codec::MAX_RESOURCES {
        return Err(SyncError::Bounds);
    }
    let mut references = Vec::with_capacity(count);
    for index in 0..count {
        let start = index * codec::CHUNK_BYTES;
        let data = &bytes[start..bytes.len().min(start + codec::CHUNK_BYTES)];
        let context = EncryptionContext {
            library_id: root.library_id.clone(),
            item_id: uuid::Uuid::new_v4().to_string(),
            revision_id: uuid::Uuid::new_v4().to_string(),
            key_id: key.key_id().into(),
        };
        let mut payload = codec::ChunkPayload {
            v: 1,
            kind: "mdreader-chunk".into(),
            document_id: root.item_id.clone(),
            revision_id: root.revision_id.clone(),
            role: if path.is_some() { "asset" } else { "markdown" }.into(),
            path: path.map(str::to_owned),
            index,
            count,
            data: URL_SAFE_NO_PAD.encode(data),
        };
        let encoded = codec::encode_chunk(&payload);
        payload.data.zeroize();
        if let Some(path) = &mut payload.path {
            path.zeroize();
        }
        let plaintext = Zeroizing::new(encoded?);
        let body = encrypt(key, &context, &plaintext)?;
        references.push(codec::ChunkReference {
            item_id: context.item_id,
            revision_id: context.revision_id,
            blob: body.blob_id().into(),
            bytes: data.len(),
            sha256: crate::blob_digest(data),
        });
        encrypted.push(body);
    }
    Ok(codec::ResourceDescriptor {
        bytes: bytes.len(),
        sha256: crate::blob_digest(bytes),
        chunks: references,
    })
}
