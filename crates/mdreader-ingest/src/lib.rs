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

pub mod deployment;
mod ingestor;
mod parse;
pub mod proposals;
mod scan;
mod source_root;
mod source_view;

use std::sync::Arc;

use catdb_engine::DomainRegistration;

pub use ingestor::MarkdownIngestor;
pub use parse::{
    parse_markdown, parse_markdown_with_limit, ParsedBlock, ParsedChunk, ParsedDocument,
    ParsedLink, ParsedSection, CHUNKER_VERSION, CHUNK_TOKEN_LIMIT,
};
pub use proposals::ProposalStore;
pub use scan::{scan_root, scan_source_root, ScanOptions, ScannedFile};
pub use source_root::{
    legacy_workspace_index_path_for_path_identity, workspace_index_path,
    workspace_index_path_for_stable_identity, SourceRoot, SourceRootError, SourceRootIdentity,
};
pub use source_view::{parse_source_document, source_document};

/// The fully composed MDReader domain registration: `mdreader-domain`'s
/// schemas and mappings plus this crate's [`MarkdownIngestor`] and chunker
/// version. This is what the `catdb` binary (and later the Tauri app) hands
/// to [`catdb_engine::EngineConfig`].
///
/// Overlay extraction does **not** run under this registration: there is
/// nowhere to put a proposal, and the one place a proposal must never go is
/// the authoritative graph. Use [`registration_with_proposals`] to enable it.
pub fn registration() -> DomainRegistration {
    mdreader_domain::engine_registration(vec![Arc::new(MarkdownIngestor::new())], CHUNKER_VERSION)
}

/// [`registration`] with overlay extraction wired to `store`: each import ends
/// by running the decision and architecture extractors over the whole corpus
/// and recording what they *propose* into the store — reviewable claims,
/// none of them asserted into the graph until a human approves them.
pub fn registration_with_proposals(store: Arc<ProposalStore>) -> DomainRegistration {
    mdreader_domain::engine_registration(
        vec![Arc::new(MarkdownIngestor::new().with_proposal_store(store))],
        CHUNKER_VERSION,
    )
}

/// [`registration`] with its markdown ingestor bound to a retained local root.
pub fn registration_with_source_root(root: SourceRoot) -> DomainRegistration {
    mdreader_domain::engine_registration(
        vec![Arc::new(MarkdownIngestor::new().with_source_root(root))],
        CHUNKER_VERSION,
    )
}

/// [`registration_with_proposals`] with every filesystem effect additionally
/// bound to `root`'s retained directory authority.
pub fn registration_with_proposals_and_source_root(
    store: Arc<ProposalStore>,
    root: SourceRoot,
) -> DomainRegistration {
    mdreader_domain::engine_registration(
        vec![Arc::new(
            MarkdownIngestor::new()
                .with_proposal_store(store)
                .with_source_root(root),
        )],
        CHUNKER_VERSION,
    )
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
        // Four pipeline schemas, three provenance cores, three overlays, and
        // the federated `Document`.
        assert_eq!(registration.schemas.len(), 11);
        assert_eq!(registration.mappings.len(), 3);
        assert_eq!(registration.ingestors.len(), 1);
        assert_eq!(registration.chunker_version, CHUNKER_VERSION);
    }
}
