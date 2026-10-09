//! Native offline migration. Current live-profile activation is never performed
//! inside the backup callback. Only selected, actually closed owners are admitted.
#[cfg(unix)]
mod archive_fence;
#[cfg(unix)]
mod archive_import;
#[cfg(any(unix, windows))]
mod closed_database;
pub(crate) mod commands;
#[cfg(any(unix, windows))]
mod conversion;
pub(crate) mod imported_history;
#[cfg(unix)]
pub(super) mod legacy_applications;
#[cfg(unix)]
mod legacy_launch;
#[cfg(unix)]
mod legacy_preferences;
#[cfg(unix)]
mod legacy_return;
#[cfg(unix)]
mod legacy_snapshot;
#[cfg(unix)]
mod legacy_source;
#[cfg(unix)]
mod modern_copies;
#[cfg(any(unix, windows))]
mod owner;
#[cfg(any(unix, windows))]
mod shutdown_handoff;
#[cfg(windows)]
mod windows_workspace;
#[cfg(unix)]
pub(crate) mod workspace;
use mdreader_profile::{Error, FrozenProfile, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Selection {
    pub document_id: String,
    pub revision: String,
}

/// Read-only format recognition shared by startup and the explicit import flow.
#[cfg(unix)]
pub(crate) fn legacy_profile(root: &mdreader_core::SourceRoot) -> Result<bool> {
    legacy_source::recognized(root)
}

/// Native controller startup verifies durable identities, not a closure claim.
#[cfg(all(unix, not(mobile)))]
pub(crate) fn verify_return_candidate(path: &Path) -> Result<()> {
    legacy_return::resume(path)?.ok_or(Error::Unsupported)?;
    Ok(())
}
