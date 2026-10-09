//! MDReader-owned parsing, filesystem authority and SQLite reader service.
//! No platform, provider credential, or CatDB type crosses this boundary.
pub mod device;
pub mod parse;
mod query;
mod ranking;
pub mod scan;
pub mod source_root;
mod store;
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
