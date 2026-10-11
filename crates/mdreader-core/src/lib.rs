//! MDReader-owned parsing, filesystem authority and SQLite reader service.
//! No platform, provider credential, or CatDB type crosses this boundary.
pub mod device;
pub mod editor_drafts;
pub mod parse;
pub mod provider_reading_state;
mod query;
mod ranking;
mod relative_link;
pub use relative_link::{decode_markdown_fragment, RelativeMarkdownTarget};
pub mod scan;
pub mod source_root;
mod store;
pub(crate) use store::local_authority;
pub use parse::{
    parse_markdown, parse_markdown_with_limit, ParsedDocument, CHUNKER_VERSION, CHUNK_TOKEN_LIMIT,
};
pub use scan::{scan_root, scan_source_root, visit_source_root, ScanOptions, ScannedFile};
pub use source_root::{SourceRoot, SourceRootError, SourceRootIdentity};
pub use store::*;

#[cfg(feature = "local-embed")]
mod embed;
#[cfg(feature = "local-embed")]
pub use embed::BgeEmbedder;
