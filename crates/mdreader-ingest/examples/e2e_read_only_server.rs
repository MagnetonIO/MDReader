//! Loopback-only, read-only HTTP surface for MDReader browser acceptance.
//!
//! Production `catdb serve` accepts only the resource-explicit authenticated
//! entity-query API. The older search/read DTOs cannot carry V1 source and
//! field authorization, so production correctly refuses them. This executable
//! is an example target used only by the local Playwright harness: it binds
//! loopback and mounts CatDB's explicitly test-scoped bounded hybrid read
//! router, whose route set contains no mutation, index, MCP, or
//! server-filesystem surface.
//! Opening an embedded engine can perform catalog/rotation maintenance, so this
//! claims only an HTTP read boundary, not a physically read-only database open.
//! Its deterministic hash embedder makes the corpus reproducible; it is test
//! evidence only and does not establish semantic-provider parity.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig};

#[derive(Clone)]
struct RunIdentity(Arc<str>);

async fn require_run_identity(
    State(expected): State<RunIdentity>,
    request: Request,
    next: Next,
) -> Response {
    let matches = request
        .headers()
        .get("x-mdreader-e2e-run")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected.0.as_ref());
    if !matches {
        return (
            StatusCode::PRECONDITION_FAILED,
            "request does not belong to this MDReader E2E invocation",
        )
            .into_response();
    }
    next.run(request).await
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let addr = args
        .next()
        .ok_or_else(|| {
            "usage: e2e_read_only_server <loopback-addr> <workspace-db> <run-identity>".to_string()
        })?
        .parse::<SocketAddr>()
        .map_err(|error| format!("invalid listener address: {error}"))?;
    let db = PathBuf::from(
        args.next()
            .ok_or_else(|| "workspace database path is required".to_string())?,
    );
    let run_identity = args
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "run identity is required".to_string())?;
    if args.next().is_some() {
        return Err("unexpected extra argument".to_string());
    }
    if !addr.ip().is_loopback() {
        return Err("the E2E read-only server may bind only to loopback".to_string());
    }

    let engine = EmbeddedCatDb::open(
        EngineConfig::new(db, Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration()),
    )
    .map_err(|error| error.to_string())?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| format!("cannot bind E2E read-only server: {error}"))?;
    println!(
        "MDReader E2E read-only server listening on http://{}",
        listener
            .local_addr()
            .map_err(|error| format!("cannot inspect E2E listener: {error}"))?
    );
    let router = catdb_server::hybrid_read_only_router_for_e2e_tests(Arc::new(engine)).layer(
        middleware::from_fn_with_state(RunIdentity(Arc::from(run_identity)), require_run_identity),
    );
    axum::serve(listener, router)
        .await
        .map_err(|error| error.to_string())
}
