//! MDReader's local document-library sources.
//!
//! Three adapters over **one local directory**, read three ways:
//!
//! ```text
//! Document.path        <- every adapter, as its natural key
//! Document.size        <- filesystem stat
//! Document.modifiedAt  <- filesystem mtime | git commit timestamp
//! Document.title       <- YAML frontmatter `title:` | the first H1
//! Document.author      <- git commit author | YAML frontmatter `author:`
//! Document.body        <- the markdown after the frontmatter block
//! ```
//!
//! These were part of `catdb-connectors` while MDReader and CatDB shared a
//! repository. They moved here with the split because they are not enterprise
//! connectors: they do not reach a second system, and the disagreements they
//! can stage (`mtime` versus commit time, git author versus a YAML `author:`)
//! are two readings of the same bytes rather than two systems of record.
//! CatDB Enterprise federates PostgreSQL, MySQL, SQL Server, Snowflake,
//! Redshift and DuckDB; a directory of markdown is MDReader's business.
//!
//! They implement [`DataSource`] against the published `catdb-sdk` facade from
//! outside the platform repository, which is what proves the facade complete.
//!
//! ## Read-only
//!
//! No adapter writes, ever. The filesystem and frontmatter adapters open files
//! for reading; the git adapter runs exactly three read-only plumbing commands
//! (`rev-parse`, `ls-files`, `log`) and touches neither the object store, the
//! index, nor the work tree.

#![deny(missing_docs)]

mod frontmatter;
mod fs;
mod git;
mod walk;

pub use frontmatter::FrontmatterSource;
pub use fs::FilesystemSource;
pub use git::GitSource;

use std::path::{Path, PathBuf};

// The contract these adapters implement, re-exported from the platform facade
// so the modules below can keep using unqualified names.
pub(crate) use catdb_sdk::connectors::{
    Availability, DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor,
    SourceError, SourceFreshness, SourceId, SourceModel, SourcePage, SourcePlan, SourceRow,
};

/// A persisted local-source body is not the exact closed shape emitted by the
/// corresponding connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("catalog local-source specification is invalid")]
pub struct CatalogSourceSpecError;

pub(crate) const LOCAL_SOURCE_FORMAT: &str = "catdb.local-source/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalRootIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows {
        volume_serial_number: u32,
        file_index: u64,
    },
}

pub(crate) fn observation_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

pub(crate) fn freshness_since(started_at_ms: i64) -> SourceFreshness {
    let observed_at_ms = observation_now_ms();
    SourceFreshness {
        observed_at_ms,
        freshness_ms: u64::try_from(observed_at_ms.saturating_sub(started_at_ms))
            .unwrap_or(u64::MAX),
    }
}

pub(crate) fn path_spec(path: &std::path::Path) -> serde_json::Value {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        return serde_json::json!({
            "platform": "unix",
            "encoding": "bytes",
            "bytes": path.as_os_str().as_bytes(),
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        return serde_json::json!({
            "platform": "windows",
            "encoding": "wide",
            "units": path.as_os_str().encode_wide().collect::<Vec<_>>(),
        });
    }
    #[allow(unreachable_code)]
    serde_json::Value::Null
}

pub(crate) fn source_spec_object<'a>(
    spec: &'a serde_json::Value,
    kind: &str,
    fields: &[&str],
) -> Result<&'a serde_json::Map<String, serde_json::Value>, CatalogSourceSpecError> {
    let object = spec.as_object().ok_or(CatalogSourceSpecError)?;
    if object.len() != fields.len()
        || fields.iter().any(|field| !object.contains_key(*field))
        || object.get("kind").and_then(serde_json::Value::as_str) != Some(kind)
        || object.get("format").and_then(serde_json::Value::as_str) != Some(LOCAL_SOURCE_FORMAT)
    {
        return Err(CatalogSourceSpecError);
    }
    Ok(object)
}

pub(crate) fn path_from_spec(
    value: &serde_json::Value,
) -> Result<std::path::PathBuf, CatalogSourceSpecError> {
    let object = value.as_object().ok_or(CatalogSourceSpecError)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;

        if object.len() != 3
            || object.get("platform").and_then(serde_json::Value::as_str) != Some("unix")
            || object.get("encoding").and_then(serde_json::Value::as_str) != Some("bytes")
        {
            return Err(CatalogSourceSpecError);
        }
        let bytes = object
            .get("bytes")
            .and_then(serde_json::Value::as_array)
            .ok_or(CatalogSourceSpecError)?
            .iter()
            .map(|byte| {
                byte.as_u64()
                    .and_then(|byte| u8::try_from(byte).ok())
                    .ok_or(CatalogSourceSpecError)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(std::path::PathBuf::from(std::ffi::OsString::from_vec(
            bytes,
        )));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt as _;

        if object.len() != 3
            || object.get("platform").and_then(serde_json::Value::as_str) != Some("windows")
            || object.get("encoding").and_then(serde_json::Value::as_str) != Some("wide")
        {
            return Err(CatalogSourceSpecError);
        }
        let units = object
            .get("units")
            .and_then(serde_json::Value::as_array)
            .ok_or(CatalogSourceSpecError)?
            .iter()
            .map(|unit| {
                unit.as_u64()
                    .and_then(|unit| u16::try_from(unit).ok())
                    .ok_or(CatalogSourceSpecError)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(std::path::PathBuf::from(std::ffi::OsString::from_wide(
            &units,
        )));
    }
    #[allow(unreachable_code)]
    Err(CatalogSourceSpecError)
}

pub(crate) fn root_authority_spec(
    path: &std::path::Path,
    identity: Option<&LocalRootIdentity>,
) -> serde_json::Value {
    serde_json::json!({
        "path": path_spec(path),
        "identity": identity.map(root_identity_spec),
    })
}

fn root_identity_spec(identity: &LocalRootIdentity) -> serde_json::Value {
    match identity {
        #[cfg(unix)]
        LocalRootIdentity::Unix { device, inode } => serde_json::json!({
            "platform": "unix",
            "device": device,
            "inode": inode,
        }),
        #[cfg(windows)]
        LocalRootIdentity::Windows {
            volume_serial_number,
            file_index,
        } => serde_json::json!({
            "platform": "windows",
            "volume_serial_number": volume_serial_number,
            "file_index": file_index,
        }),
    }
}

fn root_identity_from_spec(
    value: &serde_json::Value,
) -> Result<LocalRootIdentity, CatalogSourceSpecError> {
    let object = value.as_object().ok_or(CatalogSourceSpecError)?;
    #[cfg(unix)]
    {
        if object.len() != 3
            || object.get("platform").and_then(serde_json::Value::as_str) != Some("unix")
        {
            return Err(CatalogSourceSpecError);
        }
        return Ok(LocalRootIdentity::Unix {
            device: object
                .get("device")
                .and_then(serde_json::Value::as_u64)
                .ok_or(CatalogSourceSpecError)?,
            inode: object
                .get("inode")
                .and_then(serde_json::Value::as_u64)
                .ok_or(CatalogSourceSpecError)?,
        });
    }
    #[cfg(windows)]
    {
        if object.len() != 3
            || object.get("platform").and_then(serde_json::Value::as_str) != Some("windows")
        {
            return Err(CatalogSourceSpecError);
        }
        return Ok(LocalRootIdentity::Windows {
            volume_serial_number: object
                .get("volume_serial_number")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or(CatalogSourceSpecError)?,
            file_index: object
                .get("file_index")
                .and_then(serde_json::Value::as_u64)
                .ok_or(CatalogSourceSpecError)?,
        });
    }
    #[allow(unreachable_code)]
    Err(CatalogSourceSpecError)
}

pub(crate) fn contained_root_from_spec(
    value: &serde_json::Value,
) -> Result<crate::walk::ContainedRoot, CatalogSourceSpecError> {
    let object = value.as_object().ok_or(CatalogSourceSpecError)?;
    if object.len() != 2 || !object.contains_key("path") || !object.contains_key("identity") {
        return Err(CatalogSourceSpecError);
    }
    let path = path_from_spec(object.get("path").ok_or(CatalogSourceSpecError)?)?;
    if !path.is_absolute() {
        return Err(CatalogSourceSpecError);
    }
    let expected = root_identity_from_spec(object.get("identity").ok_or(CatalogSourceSpecError)?)?;
    let contained = crate::walk::ContainedRoot::open(&path).map_err(|_| CatalogSourceSpecError)?;
    if contained.canonical() != path || contained.identity() != &expected {
        return Err(CatalogSourceSpecError);
    }
    Ok(contained)
}

/// A retained, read-only directory handle that can be shared by local
/// connectors without resolving its pathname again.
///
/// `display_root` is retained for source specifications and diagnostics only.
/// Every filesystem effect remains anchored to `directory`, including when the
/// pathname is renamed or temporarily replaced during connector construction.
#[derive(Debug, Clone)]
pub struct DirectoryAuthority {
    display_root: PathBuf,
    contained_root: walk::ContainedRoot,
}

impl DirectoryAuthority {
    /// Adopt an already-open directory handle without reopening `display_root`.
    pub fn from_open_directory(
        display_root: impl AsRef<Path>,
        directory: std::fs::File,
    ) -> std::io::Result<Self> {
        let display_root = display_root.as_ref().to_path_buf();
        let contained_root =
            walk::ContainedRoot::from_open_directory(display_root.clone(), directory)
                .map_err(std::io::Error::other)?;
        Ok(Self {
            display_root,
            contained_root,
        })
    }

    /// Native display/provenance path associated with this authority.
    pub fn display_root(&self) -> &Path {
        &self.display_root
    }

    pub(crate) fn contained_root(&self) -> &walk::ContainedRoot {
        &self.contained_root
    }
}
