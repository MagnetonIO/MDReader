//! Windows configuration publication and canonical SQLite installer receipts.
//! Config files are replayable projections of committed phase intents, not the
//! durability boundary. No directory FlushFileBuffers equivalence is assumed.
mod files;
mod journal;
use crate::{IntegrationError as Error, Result};
pub(crate) use files::{
    create_config_parent, path_identity, read_optional, replace_config, safe_target,
};
pub(crate) use journal::Journal;
pub(crate) fn capture(error: mdreader_capture::CaptureError) -> Error {
    match error {
        mdreader_capture::CaptureError::Io(error) => Error::Io(error),
        mdreader_capture::CaptureError::TooLarge => Error::Bounds,
        _ => Error::Conflict,
    }
}
pub(crate) fn core(_: mdreader_core::SourceRootError) -> Error {
    Error::Conflict
}
pub(crate) fn directory(path: &std::path::Path) -> Result<mdreader_capture::SelectedDirectory> {
    mdreader_capture::SelectedDirectory::open(path).map_err(capture)
}
