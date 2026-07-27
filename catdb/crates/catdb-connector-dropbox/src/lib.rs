//! Read-only Dropbox source for CatDB.
//!
//! The standalone contract tests are intentionally committed before the
//! implementation. This module is populated by the GREEN commit.

#![deny(missing_docs)]

/// Dropbox metadata-read OAuth scope.
pub const DROPBOX_METADATA_READ_SCOPE: &str = "files.metadata.read";

/// Dropbox content-read OAuth scope.
pub const DROPBOX_CONTENT_READ_SCOPE: &str = "files.content.read";
