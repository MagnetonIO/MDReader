//! Read-only MDReader demo composition root.

use std::path::PathBuf;
use std::process::ExitCode;

use axum::http::{HeaderValue, Request};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::Router;
use mdreader_ingest::deployment::DemoDeploymentError;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("catdb-demo-server refused startup: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(|value| value.to_str()) {
        Some("prepare-image") => prepare_image(args.collect()).await,
        Some(command) => Err(format!("unknown command `{command}`")),
        None => serve().await,
    }
}

async fn prepare_image(args: Vec<std::ffi::OsString>) -> Result<(), String> {
    let mut database = None;
    let mut corpus = None;
    let mut manifest = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| "command arguments must be valid UTF-8".to_string())?;
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{flag} requires a value"))?;
        let slot = match flag {
            "--database" => &mut database,
            "--corpus" => &mut corpus,
            "--manifest" => &mut manifest,
            _ => return Err(format!("unknown prepare-image option `{flag}`")),
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(format!("duplicate prepare-image option `{flag}`"));
        }
        index += 2;
    }
    let database = database.ok_or_else(|| "--database is required".to_string())?;
    let corpus = corpus.ok_or_else(|| "--corpus is required".to_string())?;
    let manifest = manifest.ok_or_else(|| "--manifest is required".to_string())?;
    let identity = mdreader_ingest::deployment::prepare_demo_image(
        &database,
        &corpus,
        &manifest,
        env!("CATDB_DEMO_SOURCE_COMMIT"),
    )
    .await
    .map_err(safe_deployment_error)?;
    println!(
        "{}",
        serde_json::to_string(&identity).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn safe_deployment_error(error: DemoDeploymentError) -> String {
    match error {
        DemoDeploymentError::InvalidIdentity(message) => message,
        DemoDeploymentError::Snapshot(message) => format!("invalid SQLite snapshot: {message}"),
        DemoDeploymentError::Io { .. } => "filesystem identity verification failed".to_string(),
        DemoDeploymentError::Engine(_) => {
            "authoritative CatDB engine verification failed".to_string()
        }
        DemoDeploymentError::Serialization(_) => {
            "deployment manifest serialization failed".to_string()
        }
    }
}

fn runtime_path(variable: &str, default: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn with_deployment_identity(
    router: Router,
    source_commit: &str,
    deployment_fingerprint: &str,
) -> Result<Router, String> {
    let source_commit = HeaderValue::from_str(source_commit)
        .map_err(|_| "source commit cannot be represented as an HTTP header".to_string())?;
    let deployment_fingerprint = HeaderValue::from_str(deployment_fingerprint).map_err(|_| {
        "deployment fingerprint cannot be represented as an HTTP header".to_string()
    })?;
    Ok(router.layer(middleware::from_fn(
        move |request: Request<axum::body::Body>, next: Next| {
            let source_commit = source_commit.clone();
            let deployment_fingerprint = deployment_fingerprint.clone();
            async move {
                let mut response: Response = next.run(request).await;
                response
                    .headers_mut()
                    .insert("x-catdb-source-commit", source_commit);
                response
                    .headers_mut()
                    .insert("x-catdb-deployment-fingerprint", deployment_fingerprint);
                response
            }
        },
    )))
}

async fn serve() -> Result<(), String> {
    let port = std::env::var("PORT")
        .map_err(|_| "PORT is required".to_string())?
        .parse::<u16>()
        .map_err(|_| "PORT must be a nonzero TCP port".to_string())?;
    if port == 0 {
        return Err("PORT must be a nonzero TCP port".to_string());
    }
    let database = runtime_path("CATDB_DEMO_IMAGE_DATABASE", "/app/workspace/catdb.sqlite");
    let corpus = runtime_path("CATDB_DEMO_IMAGE_CORPUS", "/app/corpus");
    let manifest = runtime_path("CATDB_DEMO_IMAGE_MANIFEST", "/app/metadata/deployment.json");
    let runtime_root = runtime_path("CATDB_DEMO_RUNTIME_ROOT", "/tmp");
    let deployed_source_commit = std::env::var("VERCEL_GIT_COMMIT_SHA").ok();
    let runtime = mdreader_ingest::deployment::prepare_demo_runtime(
        &database,
        &corpus,
        &manifest,
        &runtime_root,
        env!("CATDB_DEMO_SOURCE_COMMIT"),
        deployed_source_commit.as_deref(),
    )
    .await
    .map_err(safe_deployment_error)?;
    let identity = runtime.identity();
    eprintln!(
        "catdb-demo-server verified source={} catalog={} corpus={} workspace={} sqlite={} deployment={}",
        identity.source_commit,
        identity.catalog_revision,
        identity.corpus_sha256,
        identity.logical_workspace_blake3,
        identity.sqlite_sha256,
        identity.deployment_fingerprint
    );

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|_| "binding the configured PORT failed".to_string())?;
    let router = with_deployment_identity(
        catdb_server::read_only_router(runtime.engine()),
        &identity.source_commit,
        &identity.deployment_fingerprint,
    )?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|_| "serving the read-only demo failed".to_string())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("installing SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;

    use super::with_deployment_identity;

    #[tokio::test]
    async fn every_runtime_response_exposes_only_public_immutable_identity() {
        let source_commit = "a".repeat(40);
        let fingerprint = format!("blake3:{}", "b".repeat(64));
        let app = with_deployment_identity(
            Router::new().route("/test", get(|| async { "ok" })),
            &source_commit,
            &fingerprint,
        )
        .expect("valid identity");

        let response = app
            .oneshot(Request::get("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("x-catdb-source-commit").unwrap(),
            source_commit.as_str()
        );
        assert_eq!(
            response
                .headers()
                .get("x-catdb-deployment-fingerprint")
                .unwrap(),
            fingerprint.as_str()
        );
        assert!(response.headers().get("x-catdb-catalog-revision").is_none());
        assert!(response.headers().get("x-catdb-workspace-path").is_none());
    }
}
