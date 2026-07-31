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
use catdb_sdk::embed::HashEmbedder;
use catdb_sdk::engine::{EmbeddedCatDb, EngineConfig};

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

fn e2e_read_only_router(
    engine: Arc<dyn catdb_sdk::server::CatDbEngine>,
    run_identity: Arc<str>,
) -> axum::Router {
    catdb_sdk::server::hybrid_read_only_router_for_e2e_tests(
        engine,
        middleware::from_fn_with_state(RunIdentity(run_identity), require_run_identity),
    )
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
    let router = e2e_read_only_router(Arc::new(engine), Arc::from(run_identity));
    axum::serve(listener, router)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, X_CONTENT_TYPE_OPTIONS};
    use axum::http::{HeaderValue, Request};
    use serde_json::json;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;

    const RUN_IDENTITY: &str = "task-16-exact-run";
    const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

    fn test_router() -> (axum::Router, TempDir) {
        let temp = tempfile::tempdir().expect("temporary E2E workspace");
        let engine = EmbeddedCatDb::open(
            EngineConfig::new(
                temp.path().join("catdb.sqlite"),
                Arc::new(HashEmbedder::new()),
            )
            .with_domain(mdreader_ingest::registration()),
        )
        .expect("open E2E workspace");
        (
            e2e_read_only_router(Arc::new(engine), Arc::from(RUN_IDENTITY)),
            temp,
        )
    }

    async fn assert_shared_response_boundary(response: Response) -> Vec<u8> {
        assert_eq!(
            response.headers().get(CACHE_CONTROL),
            Some(&HeaderValue::from_static("private, no-store"))
        );
        assert_eq!(
            response.headers().get(X_CONTENT_TYPE_OPTIONS),
            Some(&HeaderValue::from_static("nosniff"))
        );
        let declared_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .expect("shared response cap records the bounded body length")
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert!(declared_length <= MAX_RESPONSE_BYTES);
        let body = to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
            .await
            .expect("bounded response body");
        assert_eq!(declared_length, body.len());
        body.to_vec()
    }

    #[tokio::test]
    async fn missing_run_identity_refusal_stays_inside_shared_response_boundary() {
        let (router, _temp) = test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/v1/documents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        let body = assert_shared_response_boundary(response).await;
        assert_eq!(
            body,
            b"request does not belong to this MDReader E2E invocation"
        );
    }

    #[tokio::test]
    async fn foreign_run_identity_refusal_stays_inside_shared_response_boundary() {
        let (router, _temp) = test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/v1/documents")
                    .header("x-mdreader-e2e-run", "another-invocation")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
        assert_shared_response_boundary(response).await;
    }

    #[tokio::test]
    async fn exact_run_identity_reaches_hybrid_handler_inside_shared_response_boundary() {
        let (router, _temp) = test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/search")
                    .header("content-type", "application/json")
                    .header("x-mdreader-e2e-run", RUN_IDENTITY)
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "q": "hybrid",
                            "limit": 10,
                            "explain": true
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = assert_shared_response_boundary(response).await;
        let search: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(search["count"], 0);
        assert_eq!(search["hits"], json!([]));
    }
}
