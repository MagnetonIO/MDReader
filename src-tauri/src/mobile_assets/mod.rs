//! Explicit offline PNG/JPEG attachments for exact imported Markdown revisions.
//! This directory is private materialization, never an admitted crawl root.
//! Only native Markdown AST image targets are selectable; raw HTML and SVG are
//! intentionally unsupported. No original Markdown bytes are rewritten.
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::{ReaderStore, SourceRoot, SourceRootIdentity};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const MAX_FILE: u32 = 4 * 1024 * 1024;
const MAX_DOC_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LIBRARY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_CHUNK: usize = 64 * 1024;
const MAX_BASE64: usize = MAX_CHUNK.div_ceil(3) * 4;
const SESSION_TTL: Duration = Duration::from_secs(15);
type Result<T> = std::result::Result<T, String>;
fn unavailable() -> String {
    "The selected asset or document revision is unavailable.".into()
}
fn sql(error: rusqlite::Error) -> String {
    let _ = error;
    "Device asset metadata could not be updated.".into()
}
fn valid_id(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| unavailable())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

mod model;
mod paths;
mod runtime;
mod store;
mod types;
mod validation;

use model::*;
use paths::*;
use runtime::PreparedSnapshot;
pub use runtime::{AssetRuntime, ReadPermit, ValidationPermit};
pub use store::AssetStore;
pub use types::*;
pub use validation::{PreparedAsset, ValidatedAsset};

#[cfg(test)]
mod tests;
