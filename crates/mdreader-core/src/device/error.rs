/// Sanitized portable device-owner failures; no private path/body or raw SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Select a UTF-8 Markdown file up to 1 MiB with a plain filename")]
    InvalidSelection,
    #[error("Document is unavailable")]
    Unavailable,
    #[error("Document revision changed")]
    Conflict,
    #[error("Reading position is outside this revision")]
    InvalidPosition,
    #[error("Device state generation reached its supported limit")]
    StateLimit,
    #[error("Device library is full (1,000 documents or 64 MiB), including interrupted imports")]
    Quota,
    #[error("Device directory identity changed or is unavailable")]
    AuthorityChanged,
    #[error("Interrupted import copy is missing, incomplete or changed")]
    IncompleteImport,
    #[error("Unsupported device document schema")]
    UnsupportedSchema,
    #[error("Device document schema is malformed")]
    InvalidSchema,
    #[error("Device metadata operation failed")]
    Storage,
    #[error("Device file operation failed")]
    Filesystem,
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Storage
    }
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Filesystem
    }
}
impl From<crate::SourceRootError> for Error {
    fn from(_: crate::SourceRootError) -> Self {
        Self::AuthorityChanged
    }
}
impl From<crate::ReaderError> for Error {
    fn from(error: crate::ReaderError) -> Self {
        match error {
            crate::ReaderError::Conflict => Self::Conflict,
            crate::ReaderError::Unavailable => Self::Unavailable,
            crate::ReaderError::Invalid(_) => Self::InvalidSelection,
            crate::ReaderError::Io(_) => Self::Filesystem,
            crate::ReaderError::Source(_) => Self::AuthorityChanged,
            _ => Self::Storage,
        }
    }
}
