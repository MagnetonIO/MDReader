//! Opaque offline preservation, never source membership or remote authority.
//! Only the historical discriminator and collision ID are interpreted. The
//! complete raw object survives, including unknown fields and numeric lexemes.
use super::{valid_library_id, LibraryError, LibrarySource, PendingCleanup, MAX_LIBRARY_SOURCES};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::fs;
use std::{
    io::{Read, Write},
    path::Path,
};

const RETIRED_KIND: &str = "connected-catdb";
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_REGISTRY_BYTES: usize = 16 * 1024 * 1024;

/// Only exact root-level registry-upgrade snapshots belong to this archive.
pub(crate) fn is_registry_backup(path: &Path) -> bool {
    path.components().count() == 1
        && path.to_str().is_some_and(|name| {
            name.strip_prefix("library.pre-v10-")
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(valid_library_id)
        })
}

pub(crate) fn verify_registry_backup(
    root: &mdreader_core::SourceRoot,
    relative: &Path,
) -> Result<(), LibraryError> {
    if !is_registry_backup(relative) {
        return Err(LibraryError::CorruptRegistry);
    }
    let file = root
        .open_optional_file_relative(relative)
        .map_err(|_| LibraryError::PersistFailed)?
        .ok_or(LibraryError::PersistFailed)?;
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LibraryError::PersistFailed)?;
    if bytes.len() > MAX_REGISTRY_BYTES
        || relative
            != Path::new(&format!(
                "library.pre-v10-{:x}.json",
                Sha256::digest(&bytes)
            ))
    {
        return Err(LibraryError::CorruptRegistry);
    }
    #[derive(Deserialize)]
    struct Version {
        version: u32,
    }
    let version: Version =
        serde_json::from_slice(&bytes).map_err(|_| LibraryError::CorruptRegistry)?;
    if !(5..=9).contains(&version.version) {
        return Err(LibraryError::CorruptRegistry);
    }
    root.verify().map_err(|_| LibraryError::PersistFailed)
}

// Older registry rows are decoded only at the archival boundary. Live source
// types no longer contain retired remote server contracts or grants.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredLegacyLibrary {
    pub(super) version: u32,
    pub(super) active_source_id: Option<String>,
    #[serde(default)]
    pub(super) restore_suppressed_source_ids: Vec<String>,
    pub(super) sources: Vec<Box<serde_json::value::RawValue>>,
    #[serde(default, rename = "connected_catdb_rediscovery_required")]
    pub(super) retired_connections_present: bool,
    #[serde(default)]
    pub(super) cleanup_pending: Vec<PendingCleanup>,
}

#[derive(Deserialize)]
struct Header {
    kind: String,
    id: String,
}

#[derive(Clone)]
pub(super) struct RetiredMetadata {
    id: String,
    raw: Box<RawValue>,
}

impl std::fmt::Debug for RetiredMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetiredMetadata([private archive])")
    }
}

impl RetiredMetadata {
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    fn from_raw(raw: Box<RawValue>) -> Result<Self, LibraryError> {
        if raw.get().len() > MAX_RECORD_BYTES {
            return Err(LibraryError::CorruptRegistry);
        }
        let header: Header =
            serde_json::from_str(raw.get()).map_err(|_| LibraryError::CorruptRegistry)?;
        if header.kind != RETIRED_KIND || !valid_library_id(&header.id) {
            return Err(LibraryError::CorruptRegistry);
        }
        Ok(Self { id: header.id, raw })
    }
}

impl Serialize for RetiredMetadata {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RetiredMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_raw(Box::<RawValue>::deserialize(deserializer)?)
            .map_err(|_| serde::de::Error::custom("invalid archived metadata"))
    }
}

pub(super) fn partition(
    rows: Vec<Box<RawValue>>,
) -> Result<(Vec<LibrarySource>, Vec<RetiredMetadata>), LibraryError> {
    if rows.len() > MAX_LIBRARY_SOURCES {
        return Err(LibraryError::CorruptRegistry);
    }
    let mut sources = Vec::new();
    let mut retired = Vec::new();
    for raw in rows {
        let header: Header =
            serde_json::from_str(raw.get()).map_err(|_| LibraryError::CorruptRegistry)?;
        if header.kind == RETIRED_KIND {
            retired.push(RetiredMetadata::from_raw(raw)?);
        } else {
            sources
                .push(serde_json::from_str(raw.get()).map_err(|_| LibraryError::CorruptRegistry)?);
        }
    }
    Ok((sources, retired))
}

/// Retain exact pre-upgrade bytes alongside the private registry. No overwrite:
/// a content-addressed backup must either match or block the upgrade. The new
/// archive schema is not readable by an older candidate, so rollback needs this
/// snapshot rather than recreating a retired live connection.
pub(super) fn preserve_registry(path: &Path, bytes: &[u8]) -> Result<(), LibraryError> {
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err(LibraryError::CorruptRegistry);
    }
    let parent = path.parent().ok_or(LibraryError::PersistFailed)?;
    let root = mdreader_core::SourceRoot::capture(parent.to_owned())
        .map_err(|_| LibraryError::PersistFailed)?;
    let backup = parent.join(format!("library.pre-v10-{:x}.json", Sha256::digest(bytes)));
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| LibraryError::PersistFailed)?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| LibraryError::PersistFailed)?;
    match temporary.persist_noclobber(&backup) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(LibraryError::PersistFailed),
    }
    verify_registry_backup(
        &root,
        Path::new(backup.file_name().ok_or(LibraryError::PersistFailed)?),
    )
    .map_err(|_| LibraryError::PersistFailed)?;
    // Retain the archive's directory entry before replacing the primary. If a
    // host cannot synchronize its directory handle, refuse the upgrade.
    root.try_clone_directory_handle()
        .map_err(|_| LibraryError::PersistFailed)?
        .sync_all()
        .map_err(|_| LibraryError::PersistFailed)?;
    root.verify().map_err(|_| LibraryError::PersistFailed)
}

#[cfg(test)]
mod tests;
