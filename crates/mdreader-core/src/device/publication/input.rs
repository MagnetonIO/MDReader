use super::types::*;
use crate::device::assets;
use sha2::{Digest, Sha256};

impl IncomingAsset {
    pub fn new(target: String, mime: String, bytes: Vec<u8>) -> PublicationResult<Self> {
        if bytes.is_empty()
            || bytes.len() > 4 * 1024 * 1024
            || !matches!(mime.as_str(), "image/png" | "image/jpeg")
            || assets::incoming_target(&target)? != target
        {
            return Err(PublicationError::InvalidInput);
        }
        Ok(Self {
            target,
            mime,
            bytes,
        })
    }
}
impl IncomingDocument {
    /// Exact original bytes: no newline, Unicode, BOM or path normalization.
    pub fn new(
        name: String,
        markdown: Vec<u8>,
        assets: Vec<IncomingAsset>,
    ) -> PublicationResult<Self> {
        if name.is_empty()
            || name.len() > 240
            || !name.to_ascii_lowercase().ends_with(".md")
            || name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            || markdown.len() > 1024 * 1024
            || assets.len() > 8
            || assets.iter().map(|a| a.bytes.len()).sum::<usize>() > 16 * 1024 * 1024
        {
            return Err(PublicationError::InvalidInput);
        }
        let text = std::str::from_utf8(&markdown).map_err(|_| PublicationError::InvalidInput)?;
        let targets = assets::incoming_targets(text);
        let mut selected = std::collections::HashSet::new();
        for asset in &assets {
            if !targets.contains(&asset.target) || !selected.insert(&asset.target) {
                return Err(PublicationError::InvalidInput);
            }
        }
        let revision = blake3::hash(&markdown).to_hex().to_string();
        let mut hash = blake3::Hasher::new();
        hash.update(b"mdreader-incoming-document-v1\0");
        for field in [name.as_bytes(), &markdown] {
            hash.update(&(field.len() as u64).to_le_bytes());
            hash.update(field);
        }
        for asset in &assets {
            for field in [
                asset.target.as_bytes(),
                asset.mime.as_bytes(),
                asset.bytes.as_slice(),
            ] {
                hash.update(&(field.len() as u64).to_le_bytes());
                hash.update(field);
            }
        }
        Ok(Self {
            name,
            markdown,
            assets,
            revision,
            digest: hash.finalize().to_hex().to_string(),
        })
    }
    pub(super) fn records(&self) -> Vec<AssetRecord> {
        self.assets
            .iter()
            .map(|a| AssetRecord {
                id: uuid::Uuid::new_v4().to_string(),
                target: a.target.clone(),
                bytes: a.bytes.len() as u32,
                hash: format!("{:x}", Sha256::digest(&a.bytes)),
                mime: a.mime.clone(),
            })
            .collect()
    }
}
