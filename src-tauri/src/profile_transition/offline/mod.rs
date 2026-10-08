//! Native offline migration. Current live-profile activation is never performed
//! inside the backup callback. Only selected, actually closed owners are admitted.
pub(crate) mod commands;
#[cfg(unix)]
mod conversion;
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
