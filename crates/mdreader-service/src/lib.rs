//! Shared native service. Window, extension and MCP clients use one writer.
use mdreader_core::{ReaderError, ReaderStore, Result, SourceRoot, SourceRootIdentity};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(feature = "sync-native")]
pub mod accounts;
pub mod capture;
pub mod catalog;
pub mod daemon;
#[cfg(feature = "sync-native")]
pub mod device;
pub mod integrations;
mod ipc;
pub mod preservation;
pub mod provider;
pub mod reviews;
mod runtime;
pub mod sync;
pub mod workload;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Exact,
    Hybrid,
    Semantic,
    Recent,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutcome {
    pub hits: Vec<mdreader_core::Evidence>,
    pub semantic_unavailable: Option<String>,
}

const PROTOCOL: u32 = 1;
const MAX_REQUEST: u64 = 2 * 1024 * 1024;
const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
const ENDPOINT: &str = "reader-endpoint.json";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Request {
    Preservation { request: preservation::ControlRequest },
    #[cfg(feature = "sync-native")]
    Account { request: accounts::rpc::AccountRequest },
    #[cfg(feature = "sync-native")]
    Device { request: device::DeviceRequest },
    Catalog { request: catalog::CatalogRequest },
    Reviews {
        request: reviews::ReviewRequest,
    },
    Provider {
        request: provider::ProviderRequest,
    },
    Ping,
    EmbeddingPolicy,
    SetEmbeddingPolicy {
        policy: workload::EmbeddingPolicy,
    },
    Admit {
        name: String,
        path: PathBuf,
        identity: SourceRootIdentity,
    },
    Index {
        source_id: String,
    },
    ListSources,
    FindSource {
        path: PathBuf,
        identity: SourceRootIdentity,
    },
    Status {
        sources: Vec<String>,
    },
    Search {
        query: String,
        sources: Vec<String>,
        limit: usize,
    },
    Read {
        source_id: String,
        path: String,
        revision: Option<String>,
    },
    Resolve {
        source_id: String,
        from: String,
        target: String,
    },
    Related {
        source_id: String,
        path: String,
        limit: usize,
    },
    RelatedRanked {
        source_id: String,
        path: String,
        query: Option<String>,
        limit: usize,
    },
    Bookmark {
        source_id: String,
        path: String,
    },
    SetBookmark {
        source_id: String,
        path: String,
        line: u32,
    },
    StartIndex {
        source_id: String,
    },
    JobStatus {
        source_id: String,
    },
    SetSourceEnabled {
        source_id: String,
        enabled: bool,
    },
    ForgetSourceIndex {
        source_id: String,
    },
    SearchRanked {
        query: String,
        sources: Vec<String>,
        limit: usize,
        mode: SearchMode,
    },
    ListDocuments {
        source_id: String,
    },
    ConfigureCapture {
        policy: mdreader_capture::ProjectPolicy,
    },
    CaptureBackfill {
        project_id: String,
        request: capture::backfill::BackfillRequest,
    },
    CaptureStatus,
    ReaderOwnedSources,
    CaptureReaderSources,
    CapturePolicy {
        project_id: String,
    },
    PrepareIntegration {
        selection: integrations::Selection,
    },
    ApplyIntegration {
        preparation_id: String,
    },
    UninstallIntegration {
        preparation_id: String,
    },
    ListIntegrations {
        project_id: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    protocol: u32,
    port: u16,
    token: String,
    pid: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    protocol: u32,
    token: String,
    request: Request,
}
#[derive(Serialize, Deserialize)]
struct Reply {
    result: Value,
    error: Option<WireError>,
}
#[derive(Serialize, Deserialize)]
struct WireError {
    code: String,
    message: String,
}
fn protocol_error(message: &str) -> ReaderError {
    ReaderError::Invalid(message.into())
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| protocol_error("invalid protocol value"))
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn read_frame(reader: &mut impl BufRead, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_until(b'\n', &mut bytes)?;
    if bytes.len() as u64 > limit || bytes.last() != Some(&b'\n') {
        return Err(protocol_error("invalid or oversized frame"));
    }
    Ok(bytes)
}
fn token_matches(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}
fn wire_error(error: ReaderError) -> WireError {
    let (code, message) = match error {
        ReaderError::Conflict => ("stale_revision", "Document revision changed"),
        ReaderError::Cancelled => ("cancelled", "Request cancelled"),
        ReaderError::Unavailable => ("source_unavailable", "Source is unavailable"),
        ReaderError::ModelUnavailable => {
            ("model_unavailable", "Local embedding model is unavailable")
        }
        ReaderError::Source(_) => ("source_changed", "Source authority could not be verified"),
        ReaderError::Invalid(_) => ("invalid_request", "Invalid reader request"),
        ReaderError::NewerDatabase(_) => ("newer_database", "Database requires a newer reader"),
        _ => ("persistence", "Reader operation failed"),
    };
    WireError {
        code: code.into(),
        message: message.into(),
    }
}
/// Execute with the one service-owned store. Mobile embeds this same dispatcher.
pub fn execute(store: &mut ReaderStore, request: Request) -> Result<Value> {
    let result = match request {
        Request::Preservation { .. } => {
            return serde_json::to_value(preservation::ControlReply::Failure {
                error: preservation::Error::Unsupported,
            }).map_err(|_| protocol_error("response serialization failed"));
        }
        #[cfg(feature = "sync-native")]
        Request::Account { .. } => return Err(protocol_error("Accounts require the desktop service")),
        #[cfg(feature = "sync-native")]
        Request::Device { .. } => return Err(protocol_error("Device reading requires the desktop service")),
        Request::Catalog { .. } => return Err(protocol_error("Catalog requires the desktop service")),
        // Embedded dispatch has no composed private cache directories.
        Request::ReaderOwnedSources => serde_json::to_value(catalog::ReaderOwnedSources::default()),
        Request::CaptureReaderSources => Ok(serde_json::json!([])),
        Request::Reviews { .. }
        | Request::Provider { .. }
        | Request::ConfigureCapture { .. }
        | Request::CaptureBackfill { .. }
        | Request::CaptureStatus
        | Request::CapturePolicy { .. }
        | Request::PrepareIntegration { .. }
        | Request::ApplyIntegration { .. }
        | Request::UninstallIntegration { .. }
        | Request::ListIntegrations { .. } => {
            return Err(protocol_error("Capture requires the desktop service"))
        }
        Request::Ping => Ok(json!({"protocol":PROTOCOL,"pid":std::process::id(),"headless":true})),
        Request::EmbeddingPolicy => serde_json::to_value(workload::get(store)?),
        Request::SetEmbeddingPolicy { policy } => {
            workload::set(store, policy)?;
            serde_json::to_value(policy)
        }
        Request::Admit {
            name,
            path,
            identity,
        } => serde_json::to_value(
            store.add_local_source(&name, SourceRoot::restore(path, identity)?)?,
        ),
        Request::Index { source_id } => {
            store.index_source(&source_id)?;
            serde_json::to_value(store.index_status(&[source_id])?)
        }
        Request::ListSources => serde_json::to_value(store.list_sources()?),
        Request::FindSource { path, identity } => {
            serde_json::to_value(store.find_local_source(&path, &identity)?)
        }
        Request::Status { sources } => serde_json::to_value(store.index_status(&sources)?),
        Request::Search {
            query,
            sources,
            limit,
        } => serde_json::to_value(store.search(&query, &sources, limit)?),
        Request::Read {
            source_id,
            path,
            revision,
        } => serde_json::to_value(store.read_document(&source_id, &path, revision.as_deref())?),
        Request::Resolve {
            source_id,
            from,
            target,
        } => serde_json::to_value(store.resolve_reference(&source_id, &from, &target)?),
        Request::Related {
            source_id,
            path,
            limit,
        } => serde_json::to_value(store.related(&source_id, &path, limit)?),
        Request::RelatedRanked {
            source_id,
            path,
            query,
            limit,
        } => serde_json::to_value(match query {
            Some(query) => store.related_ranked_query(&source_id, &path, &query, limit)?,
            None => store.related_ranked(&source_id, &path, limit)?,
        }),
        Request::Bookmark { source_id, path } => {
            serde_json::to_value(store.bookmark(&source_id, &path)?)
        }
        Request::SetBookmark {
            source_id,
            path,
            line,
        } => {
            store.set_bookmark(&source_id, &path, line)?;
            Ok(json!({"saved":true}))
        }
        Request::StartIndex { source_id } => {
            store.queue_index(&source_id)?;
            serde_json::to_value(store.index_job(&source_id)?)
        }
        Request::JobStatus { source_id } => serde_json::to_value(store.index_job(&source_id)?),
        Request::ListDocuments { source_id } => {
            serde_json::to_value(store.list_documents(&source_id)?)
        }
        Request::SetSourceEnabled { source_id, enabled } => {
            store.set_source_enabled(&source_id, enabled)?;
            Ok(json!({"enabled":enabled}))
        }
        Request::ForgetSourceIndex { source_id } => {
            store.forget_source_index(&source_id)?;
            Ok(json!({"forgotten":true}))
        }
        Request::SearchRanked {
            query,
            sources,
            limit,
            mode,
        } => {
            let lexical = if matches!(mode, SearchMode::Recent) {
                store.search_recent(&query, &sources, limit)?
            } else {
                store.search(&query, &sources, limit)?
            };
            let wants_model = matches!(mode, SearchMode::Hybrid | SearchMode::Semantic)
                && !ReaderStore::semantic_query_text(&query)?.is_empty();
            serde_json::to_value(SearchOutcome {
                hits: if matches!(mode, SearchMode::Semantic) && wants_model {
                    Vec::new()
                } else {
                    lexical
                },
                semantic_unavailable: wants_model
                    .then(|| "Local embedding model is unavailable".into()),
            })
        }
    };
    // Ping is already JSON; keep every other response serialization fallible.
    match result {
        Ok(value) => Ok(value),
        Err(_) => Err(protocol_error("response serialization failed")),
    }
}
/// Blocking service entry point. The OS file lock is the single-writer barrier,
/// released automatically after a crash. The UI is never its lifetime owner.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServiceOptions {
    /// Run lexical-only without model initialization or network downloads.
    pub disable_model: bool,
}
pub fn serve(directory: &Path) -> Result<()> {
    serve_with_options(directory, ServiceOptions::default())
}
pub fn serve_with_options(directory: &Path, options: ServiceOptions) -> Result<()> {
    serve_configured(directory, options, NativeAdapters::default())
}
/// Native owner supplies registration/vault/transport custody. No credential
/// enumeration or provider I/O occurs merely because the service starts.
pub fn serve_with_provider_factory(
    directory: &Path,
    options: ServiceOptions,
    factory: Arc<dyn provider::ProviderFactory>,
) -> Result<()> {
    serve_configured(directory, options, NativeAdapters {
        factory: Some(factory),
        #[cfg(feature = "sync-native")]
        accounts: None,
    })
}
/// Trusted native registration and private cache layout, never renderer data.
#[cfg(feature = "sync-native")]
pub fn serve_with_native_accounts(
    directory: &Path,
    options: ServiceOptions,
    factory: Option<Arc<dyn provider::ProviderFactory>>,
    setup: accounts::Result<Option<accounts::NativeSetup>>,
    roots: accounts::CacheRoots,
) -> Result<()> {
    serve_configured(directory, options, NativeAdapters { factory, accounts: Some((setup, roots)) })
}
#[derive(Default)]
struct NativeAdapters {
    factory: Option<Arc<dyn provider::ProviderFactory>>,
    #[cfg(feature = "sync-native")]
    accounts: Option<(accounts::Result<Option<accounts::NativeSetup>>, accounts::CacheRoots)>,
}
fn serve_configured(
    directory: &Path,
    options: ServiceOptions,
    adapters: NativeAdapters,
) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let startup = daemon::StartupOwner::acquire(directory)
        .map_err(|_| protocol_error("reader service owner is unavailable"))?;
    let mut owner = ReaderStore::open(directory)?;
    workload::initialize(&mut owner)?;
    sync::initialize(&mut owner)
        .map_err(|_| protocol_error("Encrypted sync storage is unavailable"))?;
    let store = Arc::new(Mutex::new(owner));
    let runtime = Arc::new(runtime::Runtime::new_with_startup(
        store,
        directory.join("models"),
        options,
        adapters,
        Some(startup),
    )?);
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let endpoint = Endpoint {
        protocol: PROTOCOL,
        port: listener.local_addr()?.port(),
        token: token.clone(),
        pid: std::process::id(),
    };
    let temporary = directory.join(format!(".endpoint-{}", uuid::Uuid::new_v4()));
    let mut file = private_file(&temporary)?;
    file.write_all(&encode(&endpoint)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, directory.join(ENDPOINT))?;
    for stream in listener.incoming() {
        let stream = stream?;
        let token = token.clone();
        let runtime = Arc::clone(&runtime);
        // Bound the accepted handler count; stalled unauthenticated clients
        // cannot allocate an unbounded set of threads.
        static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if ACTIVE.fetch_add(1, std::sync::atomic::Ordering::AcqRel) >= 32 {
            ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            continue;
        }
        std::thread::spawn(move || {
            struct ActiveGuard;
            impl Drop for ActiveGuard {
                fn drop(&mut self) {
                    ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
            let _guard = ActiveGuard;
            let _ = ipc::connection(stream, &token, &runtime);
        });
    }
    Ok(())
}
pub fn call(directory: &Path, request: &Request) -> Result<Value> {
    let mut bytes = Vec::new();
    File::open(directory.join(ENDPOINT))?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(protocol_error("invalid service endpoint"));
    }
    let endpoint: Endpoint =
        serde_json::from_slice(&bytes).map_err(|_| protocol_error("invalid service endpoint"))?;
    if endpoint.protocol != PROTOCOL {
        return Err(protocol_error("service protocol requires restart"));
    }
    let mut stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port).into(),
        Duration::from_secs(1),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut frame = encode(&Envelope {
        protocol: PROTOCOL,
        token: endpoint.token,
        request: request.clone(),
    })?;
    if frame.len() as u64 > MAX_REQUEST {
        return Err(protocol_error("request exceeds budget"));
    }
    frame.push(b'\n');
    stream.write_all(&frame)?;
    let input = read_frame(&mut BufReader::new(stream), MAX_RESPONSE)?;
    let reply: Reply =
        serde_json::from_slice(&input).map_err(|_| protocol_error("invalid service response"))?;
    match (reply.result, reply.error) {
        (value, None) => Ok(value),
        (Value::Null, Some(error)) => Err(match error.code.as_str() {
            "stale_revision" => ReaderError::Conflict,
            "cancelled" => ReaderError::Cancelled,
            "source_unavailable" | "source_changed" => ReaderError::Unavailable,
            "model_unavailable" => ReaderError::ModelUnavailable,
            _ => protocol_error(&error.message),
        }),
        _ => Err(protocol_error("invalid service result")),
    }
}

/// Connect to the existing owner or launch a detached headless owner. Dropping
/// the returned child handle does not stop it; tests may retain it for cleanup.
pub fn ensure_running(
    directory: &Path,
    executable: &Path,
    prefix: &[std::ffi::OsString],
) -> Result<Option<std::process::Child>> {
    if call(directory, &Request::Ping).is_ok() {
        return Ok(None);
    }
    let mut command = std::process::Command::new(executable);
    command
        .args(prefix)
        .arg(directory)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200); // no console, new process group
    }
    let child = command.spawn()?;
    // Concurrent clients can both launch; the OS writer lock selects one. The
    // loser still waits for the winner's authenticated endpoint, without retry
    // storms or a second writer.
    for _ in 0..150 {
        if call(directory, &Request::Ping).is_ok() {
            return Ok(Some(child));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(protocol_error("reader service did not become ready"))
}
