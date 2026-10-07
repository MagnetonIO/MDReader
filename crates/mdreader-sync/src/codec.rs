//! Encrypted payload v1. No payload metadata is part of the opaque server manifest.
use crate::{
    crypto::{validate_digest, validate_uuid},
    Result, SyncError,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
pub const CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_RESOURCES: usize = 72;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChunkReference {
    pub item_id: String,
    pub revision_id: String,
    pub blob: String,
    pub bytes: usize,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDescriptor {
    pub bytes: usize,
    pub sha256: String,
    pub chunks: Vec<ChunkReference>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetDescriptor {
    pub path: String,
    pub mime: String,
    pub bytes: usize,
    pub sha256: String,
    pub chunks: Vec<ChunkReference>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentPayload {
    pub v: u8,
    pub kind: String,
    pub document_id: String,
    pub revision_id: String,
    pub title: String,
    pub source: ResourceDescriptor,
    pub assets: Vec<AssetDescriptor>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChunkPayload {
    pub v: u8,
    pub kind: String,
    pub document_id: String,
    pub revision_id: String,
    pub role: String,
    pub path: Option<String>,
    pub index: usize,
    pub count: usize,
    pub data: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ReadingPosition {
    #[serde(rename = "block-pixels")]
    BlockPixels {
        #[serde(rename = "revisionId")]
        revision_id: String,
        block: u32,
        #[serde(serialize_with = "pixel_offset")]
        offset: f64,
    },
    #[serde(rename = "line")]
    Line {
        #[serde(rename = "revisionId")]
        revision_id: String,
        line: u32,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserStatePayload {
    pub v: u8,
    pub kind: String,
    pub document_id: String,
    pub pinned: bool,
    pub position: Option<ReadingPosition>,
}
fn pixel_offset<S: serde::Serializer>(
    offset: &f64,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    if offset.fract() == 0.0 && *offset >= 0.0 {
        serializer.serialize_u64(*offset as u64)
    } else {
        serializer.serialize_f64(*offset)
    }
}
fn required(bytes: &[u8], keys: &[&str], max: usize) -> Result<()> {
    if bytes.len() > max {
        return Err(SyncError::Bounds);
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| SyncError::Invalid)?;
    let object = value.as_object().ok_or(SyncError::Invalid)?;
    require(object.len() == keys.len() && keys.iter().all(|k| object.contains_key(*k)))
}
fn require(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(SyncError::Invalid)
    }
}
fn path(value: &str) -> Result<()> {
    let parts: Vec<_> = value.split('/').collect();
    require(
        !value.is_empty()
            && value.encode_utf16().count() <= 512
            && parts.len() <= 8
            && parts
                .iter()
                .all(|p| !p.is_empty() && *p != "." && *p != "..")
            && !value
                .chars()
                .any(|c| c.is_control() || "\\?#:%".contains(c)),
    )
}
fn descriptor(bytes: usize, digest: &str, chunks: &[ChunkReference], max: usize) -> Result<()> {
    require(bytes <= max && chunks.len() == bytes.div_ceil(CHUNK_BYTES).max(1))?;
    validate_digest(digest)?;
    for (index, r) in chunks.iter().enumerate() {
        validate_uuid(&r.item_id)?;
        validate_uuid(&r.revision_id)?;
        validate_digest(&r.blob)?;
        validate_digest(&r.sha256)?;
        require(r.bytes == (bytes - index * CHUNK_BYTES).min(CHUNK_BYTES))?;
    }
    Ok(())
}
impl ContentPayload {
    pub fn validate(&self) -> Result<()> {
        require(self.v == 1 && self.kind == "mdreader-content")?;
        validate_uuid(&self.document_id)?;
        validate_uuid(&self.revision_id)?;
        require(
            !self.title.is_empty()
                && self.title.encode_utf16().count() <= 512
                && !self.title.contains('\0')
                && self.assets.len() <= 8,
        )?;
        descriptor(
            self.source.bytes,
            &self.source.sha256,
            &self.source.chunks,
            1024 * 1024,
        )?;
        let mut paths = HashSet::new();
        let mut total = 0;
        for a in &self.assets {
            path(&a.path)?;
            require(
                paths.insert(&a.path) && ["image/png", "image/jpeg"].contains(&a.mime.as_str()),
            )?;
            descriptor(a.bytes, &a.sha256, &a.chunks, 4 * 1024 * 1024)?;
            total += a.bytes;
        }
        require(total <= 16 * 1024 * 1024)?;
        let refs: Vec<_> = self
            .source
            .chunks
            .iter()
            .chain(self.assets.iter().flat_map(|a| a.chunks.iter()))
            .collect();
        require(
            refs.len() <= MAX_RESOURCES
                && refs
                    .iter()
                    .map(|r| &r.item_id)
                    .collect::<HashSet<_>>()
                    .len()
                    == refs.len()
                && refs
                    .iter()
                    .map(|r| &r.revision_id)
                    .collect::<HashSet<_>>()
                    .len()
                    == refs.len(),
        )
    }
}
impl ChunkPayload {
    pub fn validate(&self) -> Result<()> {
        require(
            self.v == 1
                && self.kind == "mdreader-chunk"
                && self.count > 0
                && self.count <= 16
                && self.index < self.count
                && self.data.len() <= CHUNK_BYTES.div_ceil(3) * 4,
        )?;
        validate_uuid(&self.document_id)?;
        validate_uuid(&self.revision_id)?;
        match (&self.role[..], &self.path) {
            ("markdown", None) => require(self.count <= 4)?,
            ("asset", Some(p)) => path(p)?,
            _ => return Err(SyncError::Invalid),
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&self.data)
            .map_err(|_| SyncError::Invalid)?;
        require(bytes.len() <= CHUNK_BYTES && URL_SAFE_NO_PAD.encode(&bytes) == self.data)
    }
}
impl UserStatePayload {
    pub fn validate(&self) -> Result<()> {
        require(self.v == 1 && self.kind == "mdreader-user-state")?;
        validate_uuid(&self.document_id)?;
        match &self.position {
            None => Ok(()),
            Some(ReadingPosition::BlockPixels {
                revision_id,
                block,
                offset,
            }) => {
                validate_uuid(revision_id)?;
                require(
                    *block <= 1000000
                        && offset.is_finite()
                        && *offset >= 0.0
                        && *offset <= 10000000.0,
                )
            }
            Some(ReadingPosition::Line { revision_id, line }) => {
                validate_uuid(revision_id)?;
                require(*line > 0 && *line <= 1000000)
            }
        }
    }
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T> {
    if bytes.len() > max {
        return Err(SyncError::Bounds);
    }
    serde_json::from_slice(bytes).map_err(|_| SyncError::Invalid)
}
pub fn decode_content(bytes: &[u8]) -> Result<ContentPayload> {
    let p: ContentPayload = decode(bytes, 128 * 1024)?;
    p.validate()?;
    Ok(p)
}
pub fn encode_content(p: &ContentPayload) -> Result<Vec<u8>> {
    p.validate()?;
    serde_json::to_vec(p).map_err(|_| SyncError::Invalid)
}
pub fn decode_chunk(bytes: &[u8]) -> Result<ChunkPayload> {
    required(
        bytes,
        &[
            "v",
            "kind",
            "documentId",
            "revisionId",
            "role",
            "path",
            "index",
            "count",
            "data",
        ],
        512 * 1024,
    )?;
    let p: ChunkPayload = decode(bytes, 512 * 1024)?;
    p.validate()?;
    Ok(p)
}
pub fn encode_chunk(p: &ChunkPayload) -> Result<Vec<u8>> {
    p.validate()?;
    serde_json::to_vec(p).map_err(|_| SyncError::Invalid)
}
pub fn decode_user_state(bytes: &[u8]) -> Result<UserStatePayload> {
    required(
        bytes,
        &["v", "kind", "documentId", "pinned", "position"],
        4096,
    )?;
    let p: UserStatePayload = decode(bytes, 4096)?;
    p.validate()?;
    Ok(p)
}
pub fn encode_user_state(p: &UserStatePayload) -> Result<Vec<u8>> {
    p.validate()?;
    serde_json::to_vec(p).map_err(|_| SyncError::Invalid)
}
