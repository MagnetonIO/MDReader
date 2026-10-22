//! Exact references to recorded files. A file opens through the reader's
//! ordinary exact reference (`{sourceId, path, revision}`) at the location
//! `roots` admitted it: a containing source, or an "Agent files" root. Only
//! recorded paths resolve, and private namespaces stay closed except the
//! Claude Code plans carve-out.
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

fn revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Read a recorded file in full, re-checking privacy on the resolved path.
/// The file itself must be a plain single-link Markdown file: a symlink or
/// hard link named `x.md` could otherwise expose any other file (a private
/// transcript linked into the plans folder, say).
fn load(private: &[PathBuf], abs: &Path) -> std::result::Result<Vec<u8>, Unavailable> {
    if !abs.is_absolute() || !super::is_markdown(abs) {
        return Err(Unavailable::NotMarkdown);
    }
    match std::fs::symlink_metadata(abs) {
        Ok(m) if m.file_type().is_symlink() => return Err(Unavailable::Unreadable),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Unavailable::Missing),
        Err(_) => return Err(Unavailable::Unreadable),
    }
    let canonical = match std::fs::canonicalize(abs) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Unavailable::Missing),
        Err(_) => return Err(Unavailable::Unreadable),
    };
    if !super::is_markdown(&canonical) {
        return Err(Unavailable::NotMarkdown);
    }
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
    crate::source_root::file_stamp::single_link_stamp(&file)
        .map_err(|_| Unavailable::Unreadable)?;
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

/// The exact reference for a recorded file at its admitted location
/// (`source`, `relative`), or why it is unavailable. Reads the file, so
/// callers run it outside the owner lock.
pub fn file_reference(
    private: &[PathBuf],
    abs_path: &str,
    source: &str,
    relative: &str,
) -> std::result::Result<ExactReference, Unavailable> {
    let bytes = load(private, Path::new(abs_path))?;
    if std::str::from_utf8(&bytes).is_err() {
        return Err(Unavailable::Unreadable);
    }
    Ok(ExactReference {
        source_id: source.to_owned(),
        path: relative.to_owned(),
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
    let bytes = load(private, Path::new(abs_path)).map_err(|_| ReaderError::Unavailable)?;
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

    /// The recorded file behind `path` in an "Agent files" root. `None` for
    /// any other source, so the caller reads it through its usual scope.
    pub fn agent_file_at(&mut self, source: &str, path: &str) -> Result<Option<String>> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            Ok(db
                .query_row(
                    "SELECT l.abs_path FROM reader_agent_activity_locations l JOIN reader_agent_activity_roots r ON r.source_id=l.source_id WHERE l.source_id=?1 AND l.path=?2",
                    [source, path],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }
}
