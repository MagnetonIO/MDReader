//! Read-only MDReader demo composition root.
//!
//! The server is intentionally fail-closed in this integration branch. Task 3
//! owns the authoritative catalog revision API; until that reviewed dependency
//! lands, the binary will not bind a listener and therefore cannot serve a
//! deployment that merely *claims* to be pinned.

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!(
        "catdb-demo-server refused startup: authoritative catalog revision verification is unavailable"
    );
    ExitCode::FAILURE
}

// This code is type-checked for the eventual integration without making the
// unsafe transition deployable. Task 3 must replace the startup refusal with a
// real comparison between the requested pin and the opened catalog before it
// may call this function.
#[allow(dead_code)]
async fn serve_after_authoritative_catalog_verification() -> Result<(), String> {
    use std::sync::Arc;

    use catdb_embed::HashEmbedder;
    use catdb_engine::{EmbeddedCatDb, EngineConfig};

    let engine = EmbeddedCatDb::open(
        EngineConfig::new("/app/workspace/catdb.sqlite", Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration()),
    )
    .map_err(|error| error.to_string())?;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .map_err(|error| error.to_string())?;
    axum::serve(listener, catdb_server::read_only_router(Arc::new(engine)))
        .await
        .map_err(|error| error.to_string())
}
