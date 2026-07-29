//! Provider-neutral, native-only Markdown document capability.

mod model;
pub(crate) mod source;
pub(crate) mod store;
pub(crate) mod sync;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use model::{
    CloudDocumentError, CloudIoContext, KnownMarkdownMetadata, MarkdownChangePage,
    MarkdownDocumentRevision, MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage,
    MarkdownWriteReceipt, ProviderCursor, ProviderRevision, SourceRecordKey,
};
#[cfg(all(test, feature = "google-drive-connector"))]
pub(crate) use source::map_drive_document_error;
#[cfg(all(test, feature = "dropbox-connector"))]
pub(crate) use source::map_dropbox_document_error;
#[cfg(test)]
pub(crate) use source::map_lifecycle_error;
#[allow(unused_imports)]
pub(crate) use sync::{CloudSyncCoordinator, CloudSyncError, CloudSyncPhase};
