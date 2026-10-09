//! Native offline migration. Current live-profile activation is never performed
//! inside the backup callback. Only selected, actually closed owners are admitted.
#[cfg(unix)]
mod archive_fence;
#[cfg(unix)]
mod archive_import;
pub(crate) mod commands;
#[cfg(unix)]
mod conversion;
pub(crate) mod imported_history;
#[cfg(unix)]
pub(super) mod legacy_applications;
#[cfg(unix)]
mod legacy_preferences;
#[cfg(unix)]
mod legacy_snapshot;
#[cfg(unix)]
mod legacy_source;
#[cfg(unix)]
mod owner;
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
