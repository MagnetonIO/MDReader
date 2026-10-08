//! Explicit offline PNG/JPEG attachments for exact imported Markdown revisions.
//! This directory is private materialization, never an admitted crawl root.
//! Only Markdown AST image targets are selectable; raw HTML and SVG are
//! intentionally unsupported. No original Markdown bytes are rewritten.
//!
//! Enable `device-assets` for the bounded PNG/JPEG decoder. Asset stores and
//! runtimes belong to one live `ReaderStore`; reopen requires fresh instances.
//! Prepare under owner access, validate/verify outside its lock, then publish
//! or install with final authority revalidation. Publication commits the existing
//! asset transaction; this API does not apply incoming document revisions.
use crate::{ReaderStore, SourceRoot, SourceRootIdentity};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
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
pub type Result<T> = std::result::Result<T, Error>;
fn unavailable() -> Error {
    Error::Unavailable
}
fn sql(_: rusqlite::Error) -> Error {
    Error::Metadata
}
fn valid_id(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| unavailable())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

mod error;
mod model;
mod paths;
mod runtime;
mod store;
mod types;
mod validation;

pub use error::Error;
use model::*;
use paths::*;
pub use runtime::{AssetRuntime, PreparedSnapshot, ReadPermit, ValidationPermit, VerifiedSnapshot};
pub use store::AssetStore;
pub use types::*;
pub use validation::{PreparedAsset, ValidatedAsset};

#[cfg(test)]
mod tests;
