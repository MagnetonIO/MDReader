use super::EncryptedResource;
use crate::{
    blob_digest,
    codec::{decode_chunk, AssetDescriptor, ContentPayload, MAX_RESOURCES},
    crypto::{decode_canonical, envelope_key_id, validate_digest},
    decrypt, EncryptionContext, LibraryKey, Result, SyncError, SyncScope, MAX_ENVELOPE_BYTES,
};
use std::collections::{HashMap, HashSet};
use zeroize::Zeroizing;

// V1's 17 MiB maximum original resource bytes expand twice through base64/JSON.
// Admission is bounded independently of attacker-controlled declared byte counts.
const MAX_RESOURCE_WIRE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CHUNK_ENVELOPE_BYTES: usize = 512 * 1024;

pub(super) fn decrypt_blob(
    library: &str,
    item: &str,
    revision: &str,
    blob: &str,
    envelope: &[u8],
    keys: &[&LibraryKey],
) -> Result<Zeroizing<Vec<u8>>> {
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(SyncError::Bounds);
    }
    validate_digest(blob)?;
    if blob_digest(envelope) != blob {
        return Err(SyncError::Authentication);
    }
    let key_id = envelope_key_id(envelope)?;
    let key = keys
        .iter()
        .find(|key| key.key_id() == key_id)
        .ok_or(SyncError::NotReady)?;
    decrypt(
        key,
        &EncryptionContext {
            library_id: library.into(),
            item_id: item.into(),
            revision_id: revision.into(),
            key_id,
        },
        envelope,
    )
}

pub(super) struct Resources<'a> {
    bodies: HashMap<&'a str, &'a [u8]>,
    used: HashSet<&'a str>,
}
impl<'a> Resources<'a> {
    pub(super) fn new(resources: &[EncryptedResource<'a>]) -> Result<Self> {
        if resources.len() > MAX_RESOURCES {
            return Err(SyncError::Bounds);
        }
        let mut bodies = HashMap::new();
        let mut bytes = 0usize;
        for resource in resources {
            validate_digest(resource.blob_id)?;
            bytes = bytes
                .checked_add(resource.envelope_utf8.len())
                .ok_or(SyncError::Bounds)?;
            if bytes > MAX_RESOURCE_WIRE_BYTES
                || resource.envelope_utf8.len() > MAX_CHUNK_ENVELOPE_BYTES
            {
                return Err(SyncError::Bounds);
            }
            if bodies
                .insert(resource.blob_id, resource.envelope_utf8)
                .is_some()
            {
                return Err(SyncError::Invalid);
            }
        }
        Ok(Self {
            bodies,
            used: HashSet::new(),
        })
    }
    pub(super) fn assemble(
        &mut self,
        scope: &SyncScope,
        content: &ContentPayload,
        asset: Option<&AssetDescriptor>,
        keys: &[&LibraryKey],
    ) -> Result<Zeroizing<Vec<u8>>> {
        let (bytes, digest, references, role, path) = match asset {
            None => (
                content.source.bytes,
                &content.source.sha256,
                &content.source.chunks,
                "markdown",
                None,
            ),
            Some(asset) => (
                asset.bytes,
                &asset.sha256,
                &asset.chunks,
                "asset",
                Some(asset.path.as_str()),
            ),
        };
        // The caller has validated the descriptor before this allocation.
        let mut original = Zeroizing::new(Vec::with_capacity(bytes));
        for (index, reference) in references.iter().enumerate() {
            let (blob, envelope) = self
                .bodies
                .get_key_value(reference.blob.as_str())
                .ok_or(SyncError::NotReady)?;
            if !self.used.insert(*blob) {
                return Err(SyncError::Invalid);
            }
            let plaintext = decrypt_blob(
                scope.library_id(),
                &reference.item_id,
                &reference.revision_id,
                blob,
                envelope,
                keys,
            )?;
            let chunk = decode_chunk(&plaintext)?;
            if chunk.document_id != content.document_id
                || chunk.revision_id != content.revision_id
                || chunk.role != role
                || chunk.path.as_deref() != path
                || chunk.index != index
                || chunk.count != references.len()
            {
                return Err(SyncError::Authentication);
            }
            let data = Zeroizing::new(decode_canonical(&chunk.data)?);
            if data.len() != reference.bytes || blob_digest(&data) != reference.sha256 {
                return Err(SyncError::Authentication);
            }
            original.extend_from_slice(&data);
        }
        if original.len() != bytes || blob_digest(&original) != *digest {
            return Err(SyncError::Authentication);
        }
        Ok(original)
    }
    pub(super) fn finish(self) -> Result<()> {
        if self.used.len() != self.bodies.len() {
            return Err(SyncError::Invalid);
        }
        Ok(())
    }
}
