//! "Agent files": an internal read-only exact-document scope. Each recorded
//! file gets its own opaque source id, so the reader's ordinary exact
//! reference (`{sourceId, path, revision}`) opens it without admitting its
//! folder. Only recorded paths resolve, and private namespaces stay closed
//! except the Claude Code plans carve-out.
use super::schema;
use crate::{Document, ReaderError, ReaderStore, Result, MAX_DOCUMENT_BYTES};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// A reader reference for one recorded file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactReference {
    pub source_id: String,
    pub path: String,
    pub revision: String,
}

/// Why a recorded file cannot be opened right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    Missing,
    Private,
    NotMarkdown,
    TooLarge,
    Unreadable,
}

pub(super) fn exact_id(abs_path: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-agent-file-v1\0");
    hasher.update(abs_path.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Read a recorded file in full, re-checking privacy on the resolved path.
fn load(private: &[PathBuf], abs: &Path) -> std::result::Result<Vec<u8>, Unavailable> {
    if !abs.is_absolute() || !super::is_markdown(abs) {
        return Err(Unavailable::NotMarkdown);
    }
    let canonical = match std::fs::canonicalize(abs) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Unavailable::Missing),
        Err(_) => return Err(Unavailable::Unreadable),
    };
    if crate::local_authority::private_document(private, abs)
        || crate::local_authority::private_document(private, &canonical)
    {
        return Err(Unavailable::Private);
    }
    let file = std::fs::File::open(&canonical).map_err(|_| Unavailable::Unreadable)?;
    let metadata = file.metadata().map_err(|_| Unavailable::Unreadable)?;
    if !metadata.is_file() {
        return Err(Unavailable::Missing);
    }
    if metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(Unavailable::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Unavailable::Unreadable)?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(Unavailable::TooLarge);
    }
    // A path swapped during the read no longer names what was checked.
    if std::fs::canonicalize(abs).ok().as_deref() != Some(canonical.as_path()) {
        return Err(Unavailable::Unreadable);
    }
    Ok(bytes)
}

fn file_name(abs: &Path) -> Option<&str> {
    abs.file_name().and_then(|n| n.to_str())
}

/// The exact reference for a recorded file, or why it is unavailable. Reads
/// the file, so callers run it outside the owner lock.
pub fn file_reference(
    private: &[PathBuf],
    abs_path: &str,
    exact: &str,
) -> std::result::Result<ExactReference, Unavailable> {
    let abs = Path::new(abs_path);
    let bytes = load(private, abs)?;
    if std::str::from_utf8(&bytes).is_err() {
        return Err(Unavailable::Unreadable);
    }
    Ok(ExactReference {
        source_id: exact.to_owned(),
        path: file_name(abs).ok_or(Unavailable::NotMarkdown)?.to_owned(),
        revision: revision(&bytes),
    })
}

/// Read a recorded file for its exact reference. A stale revision is a
/// conflict, never a silent reload.
pub fn read_file(
    private: &[PathBuf],
    abs_path: &str,
    source: &str,
    path: &str,
    expected: Option<&str>,
) -> Result<Document> {
    let abs = Path::new(abs_path);
    if file_name(abs) != Some(path) {
        return Err(ReaderError::Unavailable);
    }
    let bytes = load(private, abs).map_err(|_| ReaderError::Unavailable)?;
    let revision = revision(&bytes);
    if expected.is_some_and(|r| r != revision) {
        return Err(ReaderError::Conflict);
    }
    let markdown = String::from_utf8(bytes)
        .map_err(|_| ReaderError::Invalid("Markdown must be UTF-8".into()))?;
    Ok(Document {
        source_id: source.to_owned(),
        path: path.to_owned(),
        revision,
        markdown,
    })
}

impl ReaderStore {
    /// Private namespaces that agent files may not be opened from.
    pub fn agent_private_paths(&mut self) -> Result<Vec<PathBuf>> {
        self.with_library_connection(|db| crate::local_authority::private_paths(db))
    }

    /// The recorded path behind an "Agent files" source id. `None` means the
    /// id is not an agent file, so the caller tries its other scopes.
    pub fn agent_file_path(&mut self, source: &str) -> Result<Option<String>> {
        if source.len() != 64 || !source.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            Ok(db
                .query_row(
                    "SELECT abs_path FROM reader_agent_activity_files WHERE exact_id=?1 LIMIT 1",
                    [source],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }
}
