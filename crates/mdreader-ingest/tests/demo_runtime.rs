use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions};
use serde_json::Value;
use tempfile::TempDir;

struct BuiltWorkspace {
    _temp: TempDir,
    corpus: PathBuf,
    database: PathBuf,
    manifest: PathBuf,
    runtime_root: PathBuf,
    catalog_revision: String,
}

fn demo_binary() -> &'static str {
    env!("CARGO_BIN_EXE_catdb-demo-server")
}

fn curated_corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../catdb-demo/corpus")
        .canonicalize()
        .expect("curated corpus exists")
}

fn built_workspace() -> BuiltWorkspace {
    let temp = tempfile::tempdir().expect("temporary demo workspace");
    let database = temp.path().join("image/workspace/catdb.sqlite");
    let manifest = temp.path().join("image/metadata/deployment.json");
    let runtime_root = temp.path().join("runtime");
    std::fs::create_dir_all(database.parent().expect("database parent")).unwrap();
    std::fs::create_dir_all(manifest.parent().expect("manifest parent")).unwrap();
    std::fs::create_dir_all(&runtime_root).unwrap();

    let corpus = curated_corpus();
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(&database, Arc::new(HashEmbedder::new()))
            .with_domain(mdreader_ingest::registration()),
    )
    .expect("open image workspace");
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(engine.import(&corpus, ImportOptions::default()))
        .expect("import curated corpus");
    let catalog_revision = engine.pinned_catalog_revision().revision_id.to_string();
    drop(engine);

    BuiltWorkspace {
        _temp: temp,
        corpus,
        database,
        manifest,
        runtime_root,
        catalog_revision,
    }
}

fn assert_digest(value: &Value, prefix: &str) {
    let digest = value.as_str().expect("digest is a string");
    assert_eq!(digest.len(), prefix.len() + 64, "{digest}");
    assert!(digest.starts_with(prefix), "{digest}");
    assert!(
        digest[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{digest}"
    );
}

fn prepare_image(fixture: &BuiltWorkspace) -> Value {
    let output = Command::new(demo_binary())
        .arg("prepare-image")
        .arg("--database")
        .arg(&fixture.database)
        .arg("--corpus")
        .arg(&fixture.corpus)
        .arg("--manifest")
        .arg(&fixture.manifest)
        .output()
        .expect("run image preparation");
    assert!(
        output.status.success(),
        "prepare-image failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&std::fs::read(&fixture.manifest).unwrap()).unwrap()
}

#[test]
fn prepare_image_records_authoritative_identity_and_all_required_digests() {
    let fixture = built_workspace();
    let manifest = prepare_image(&fixture);
    assert_eq!(manifest["format"], "catdb-demo-deployment/v1");
    assert_eq!(
        manifest["catalog_revision"], fixture.catalog_revision,
        "the manifest must use the revision read from the opened engine"
    );
    let source_commit = manifest["source_commit"].as_str().unwrap();
    assert_eq!(source_commit.len(), 40);
    assert!(
        source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{source_commit}"
    );
    assert_digest(&manifest["corpus_sha256"], "sha256:");
    assert_digest(&manifest["logical_workspace_blake3"], "blake3:");
    assert_digest(&manifest["sqlite_sha256"], "sha256:");
    assert_digest(&manifest["deployment_fingerprint"], "blake3:");
}

struct RunningServer {
    child: Child,
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Value,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn decode_chunked(mut bytes: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    loop {
        let line_end = bytes
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("chunk size terminator");
        let size_text = std::str::from_utf8(&bytes[..line_end]).unwrap();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap(), 16).unwrap();
        bytes = &bytes[line_end + 2..];
        if size == 0 {
            break;
        }
        decoded.extend_from_slice(&bytes[..size]);
        bytes = &bytes[size + 2..];
    }
    decoded
}

fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> std::io::Result<HttpResponse> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let body = body.unwrap_or("");
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP header terminator");
    let header_text = std::str::from_utf8(&bytes[..split]).unwrap();
    let mut lines = header_text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .expect("HTTP status");
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(':').expect("HTTP response header");
            (name.to_ascii_lowercase(), value.trim().to_string())
        })
        .collect::<HashMap<_, _>>();
    let mut body_bytes = bytes[split + 4..].to_vec();
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        body_bytes = decode_chunked(&body_bytes);
    }
    let body = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body_bytes).expect("JSON response body")
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn runtime_command(fixture: &BuiltWorkspace, port: u16) -> Command {
    let mut command = Command::new(demo_binary());
    command
        .env("PORT", port.to_string())
        .env("CATDB_DEMO_IMAGE_DATABASE", &fixture.database)
        .env("CATDB_DEMO_IMAGE_CORPUS", &fixture.corpus)
        .env("CATDB_DEMO_IMAGE_MANIFEST", &fixture.manifest)
        .env("CATDB_DEMO_RUNTIME_ROOT", &fixture.runtime_root);
    command
}

fn start_server(fixture: &BuiltWorkspace, port: u16) -> RunningServer {
    let child = runtime_command(fixture, port)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start demo runtime");
    RunningServer { child }
}

fn wait_until_ready(server: &mut RunningServer, port: u16) -> HttpResponse {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(response) = request(port, "GET", "/v1/documents", None) {
            return response;
        }
        if let Some(status) = server.child.try_wait().unwrap() {
            panic!("demo runtime exited before it listened: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "demo runtime did not listen on $PORT"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn runtime_uses_a_unique_copy_and_serves_real_data_on_only_the_five_read_routes() {
    let fixture = built_workspace();
    let _manifest = prepare_image(&fixture);
    let snapshot_before = std::fs::read(&fixture.database).unwrap();
    let port = free_port();
    let mut server = start_server(&fixture, port);

    let documents = wait_until_ready(&mut server, port);
    assert_eq!(documents.status, 200);
    assert_eq!(documents.body["count"], 4);
    assert!(
        documents.body["documents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|document| document["file_path"] == "notes/provenance.md"),
        "{}",
        documents.body
    );
    assert_eq!(
        documents.headers.get("cache-control").map(String::as_str),
        Some("private, no-store")
    );
    assert_eq!(
        documents
            .headers
            .get("x-content-type-options")
            .map(String::as_str),
        Some("nosniff")
    );

    let sources = request(port, "GET", "/v1/sources", None).unwrap();
    assert_eq!(sources.status, 200);
    assert_eq!(sources.body["count"], 1);
    assert!(
        sources.body["sources"][0].get("root_path").is_none(),
        "{}",
        sources.body
    );

    let search = request(
        port,
        "POST",
        "/v1/search",
        Some(r#"{"q":"provenance","lexical_only":true,"explain":true,"limit":5}"#),
    )
    .unwrap();
    assert_eq!(search.status, 200, "{}", search.body);
    assert!(
        search.body["count"].as_u64().unwrap() > 0,
        "{}",
        search.body
    );
    let provenance = &search.body["hits"][0]["explain"]["provenance"];
    assert!(provenance["source_id"].is_string(), "{}", search.body);
    assert!(provenance["file_path"].is_string(), "{}", search.body);
    assert!(provenance.get("source_root").is_none(), "{}", search.body);
    assert!(
        search.body["hits"][0]["sem_rank"].is_null(),
        "{}",
        search.body
    );
    assert!(
        search.body["hits"][0]["explain"]["embedding_model"].is_null(),
        "{}",
        search.body
    );

    let read = request(
        port,
        "POST",
        "/v1/read",
        Some(r#"{"reference":"notes/provenance.md"}"#),
    )
    .unwrap();
    assert_eq!(read.status, 200, "{}", read.body);
    assert_eq!(read.body["document"]["file_path"], "notes/provenance.md");
    assert!(
        read.body["content"]
            .as_str()
            .unwrap()
            .contains("Typed output without provenance"),
        "{}",
        read.body
    );

    let related = request(
        port,
        "POST",
        "/v1/related",
        Some(r#"{"reference":"README.md","limit":5}"#),
    )
    .unwrap();
    assert_eq!(related.status, 200, "{}", related.body);
    assert_eq!(related.body["document"]["file_path"], "README.md");

    for (method, path, body) in [
        ("POST", "/v1/apply", Some(r#"{"operations":[]}"#)),
        ("POST", "/v1/index", Some(r#"{"root":"/tmp"}"#)),
        ("POST", "/v1/import", Some("{}")),
        ("POST", "/v1/validate", Some(r#"{"q":"x"}"#)),
        ("POST", "/v1/search/stream", Some(r#"{"q":"x"}"#)),
        ("GET", "/v1/index_status", None),
        ("GET", "/v1/status", None),
        ("GET", "/v1/runtime", None),
        ("POST", "/mcp", Some("{}")),
        ("POST", "/sql", Some("{}")),
        ("GET", "/admin", None),
    ] {
        let response = request(port, method, path, body).unwrap();
        assert_eq!(response.status, 404, "{method} {path} was published");
    }

    assert_eq!(std::fs::read(&fixture.database).unwrap(), snapshot_before);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = fixture.database.as_os_str().to_owned();
        sidecar.push(suffix);
        assert!(
            !PathBuf::from(sidecar).exists(),
            "{suffix} mutated image authority"
        );
    }
    let runtime_entries = std::fs::read_dir(&fixture.runtime_root)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(runtime_entries.len(), 1, "{runtime_entries:?}");
    let runtime_database = runtime_entries[0].path().join("catdb.sqlite");
    assert!(runtime_database.is_file(), "runtime copy was not created");
    assert_ne!(runtime_database, fixture.database);
}

fn assert_startup_refusal(fixture: &BuiltWorkspace, expected: &str) {
    let output = runtime_command(fixture, free_port())
        .output()
        .expect("run refusing demo runtime");
    assert!(
        !output.status.success(),
        "invalid image started successfully"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "expected startup refusal containing {expected:?}, got: {stderr}"
    );
    assert_eq!(
        std::fs::read_dir(&fixture.runtime_root).unwrap().count(),
        0,
        "startup copied an image before completing identity verification"
    );
}

#[test]
fn startup_verifies_every_image_identity_before_copying() {
    let fixture = built_workspace();
    let _manifest = prepare_image(&fixture);
    let mut bytes = std::fs::read(&fixture.database).unwrap();
    bytes.push(0);
    std::fs::write(&fixture.database, bytes).unwrap();

    assert_startup_refusal(&fixture, "SQLite snapshot digest mismatch");
}

#[test]
fn startup_rejects_mismatched_compiled_and_platform_commits_before_copying() {
    let compiled_fixture = built_workspace();
    let mut manifest = prepare_image(&compiled_fixture);
    manifest["source_commit"] = Value::String("b".repeat(40));
    std::fs::write(
        &compiled_fixture.manifest,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    assert_startup_refusal(
        &compiled_fixture,
        "deployment source commit does not match the compiled runtime",
    );

    let platform_fixture = built_workspace();
    let _manifest = prepare_image(&platform_fixture);
    let output = runtime_command(&platform_fixture, free_port())
        .env("VERCEL_GIT_COMMIT_SHA", "b".repeat(40))
        .output()
        .expect("run refusing demo runtime");
    assert!(
        !output.status.success(),
        "mismatched platform commit started"
    );
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("platform deployment commit does not match the compiled runtime"));
    assert_eq!(
        std::fs::read_dir(&platform_fixture.runtime_root)
            .unwrap()
            .count(),
        0
    );
}

#[cfg(unix)]
#[test]
fn startup_rejects_symlinked_image_database_and_manifest() {
    use std::os::unix::fs::symlink;

    let database_fixture = built_workspace();
    let _manifest = prepare_image(&database_fixture);
    let authority = database_fixture
        .database
        .with_file_name("catdb-authority.sqlite");
    std::fs::rename(&database_fixture.database, &authority).unwrap();
    symlink(&authority, &database_fixture.database).unwrap();
    assert_startup_refusal(&database_fixture, "regular file");

    let manifest_fixture = built_workspace();
    let _manifest = prepare_image(&manifest_fixture);
    let authority = manifest_fixture
        .manifest
        .with_file_name("deployment-authority.json");
    std::fs::rename(&manifest_fixture.manifest, &authority).unwrap();
    symlink(&authority, &manifest_fixture.manifest).unwrap();
    assert_startup_refusal(&manifest_fixture, "regular file");
}
