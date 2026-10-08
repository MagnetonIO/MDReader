//! Durable, explicitly selected device Markdown on the existing reader owner.
//! No platform runtime, profile discovery, secondary writer or sync application.
use crate::{ReaderStore, SourceRoot, SourceRootIdentity};
use rusqlite::{params, OptionalExtension};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

#[cfg(feature = "device-assets")]
pub mod assets;

pub mod publication;
pub mod state;
#[cfg(feature = "device-assets")]
pub mod catalog;

mod documents;
mod error;
mod imports;
mod recovery;
mod schema;
mod types;
pub use documents::{document, list, pin, position, read, rebuild, search};
pub use error::Error;
pub use imports::import;
use imports::{intent_root, publish_import};
#[cfg(test)]
use imports::{reserve_import, write_import};
#[cfg(test)]
use recovery::pending_imports;
pub use recovery::{discard_import, recover_imports};
use schema::initialize as schema;
use types::ImportIntent;
pub use types::{DeviceDocument, DeviceHit, DeviceLibrary, DeviceRead, PendingDeviceImport};

pub type Result<T> = std::result::Result<T, Error>;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_LIBRARY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DOCUMENTS: u64 = 1000;

pub(crate) fn source_authority(db: &rusqlite::Connection, id: &str) -> crate::Result<()> {
    schema::source_authority(db, id).map_err(|_| crate::ReaderError::Unavailable)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod schema_tests;

pub(crate) fn set_source_enabled(
    db: &mut rusqlite::Connection,
    id: &str,
    enabled: bool,
) -> crate::Result<bool> {
    state::source_enabled(db, id, enabled).map_err(|_| crate::ReaderError::Unavailable)
}
