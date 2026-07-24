//! MDReader's markdown indexing pipeline: scanner, parser/chunker, and the
//! incremental [`MarkdownIngestor`] that emits `CatOperation`s and domain
//! rows through the engine's [`IngestContext`](catdb_engine::IngestContext).
//!
//! The crate also hosts the `catdb` binary — the composition root where the
//! MDReader domain (schemas + mappings from `mdreader-domain`, the markdown
//! ingestor from here) registers itself into the platform engine and hands
//! the composed engine to `catdb-cli`. The platform boundary demands this
//! direction: `catdb/*` never depends on `mdreader-*`, so the app side must
//! be where the two meet.

#![deny(missing_docs)]

mod ingestor;
mod parse;
mod scan;

use std::sync::Arc;

use catdb_engine::DomainRegistration;

pub use ingestor::MarkdownIngestor;
pub use parse::{
    parse_markdown, parse_markdown_with_limit, ParsedBlock, ParsedChunk, ParsedDocument,
    ParsedLink, ParsedSection, CHUNKER_VERSION, CHUNK_TOKEN_LIMIT,
};
pub use scan::{scan_root, ScanOptions, ScannedFile};

/// The fully composed MDReader domain registration: `mdreader-domain`'s
/// schemas and mappings plus this crate's [`MarkdownIngestor`] and chunker
/// version. This is what the `catdb` binary (and later the Tauri app) hands
/// to [`catdb_engine::EngineConfig`].
pub fn registration() -> DomainRegistration {
    mdreader_domain::engine_registration(vec![Arc::new(MarkdownIngestor::new())], CHUNKER_VERSION)
}

/// Returns this crate's package name.
pub fn crate_name() -> &'static str {
    "mdreader-ingest"
}

/// Returns the IR crate whose CatOperations this crate emits.
pub fn ir_crate() -> &'static str {
    catdb_ir::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_into_catdb_ir() {
        assert_eq!(crate_name(), env!("CARGO_PKG_NAME"));
        assert_eq!(ir_crate(), "catdb-ir");
    }

    #[test]
    fn registration_bundles_domain_and_ingestor() {
        let registration = registration();
        assert_eq!(registration.name, "mdreader-domain");
        // Four pipeline schemas, three provenance cores, three overlays.
        assert_eq!(registration.schemas.len(), 10);
        assert_eq!(registration.mappings.len(), 3);
        assert_eq!(registration.ingestors.len(), 1);
        assert_eq!(registration.chunker_version, CHUNKER_VERSION);
    }
}
