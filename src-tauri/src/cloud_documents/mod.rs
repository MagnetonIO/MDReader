//! Provider-neutral, native-only Markdown document capability.

mod model;
pub(crate) mod source;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use model::{
    CloudDocumentError, CloudIoContext, MarkdownChangePage, MarkdownDocumentRevision,
    MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage, MarkdownWriteReceipt,
    ProviderCursor, ProviderRevision, SourceRecordKey,
};
pub(crate) use source::{
    map_drive_document_error, map_dropbox_document_error, map_lifecycle_error,
    MarkdownWriteCapability,
};
