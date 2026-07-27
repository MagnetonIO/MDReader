//! Native product boundary for the federated `Document` concept.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use catdb_connectors::{
    CancellationToken, DataSource, FilesystemSource, FrontmatterSource, GitSource, ReadContext,
};
use catdb_federation::{Completeness, ExecutionPolicy, Executor, Freshness, Outcome};
use mdreader_domain::federated::{
    document_schema, Federation, ProviderVerdict, SCHEMA as FEDERATED_SCHEMA,
};
use serde::Serialize;
use specta::Type;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::commands::{resolve_within, WorkspaceState};
use crate::overlay::SchemaSummary;

const QUERY_DEADLINE: Duration = Duration::from_secs(30);
const QUERY_WORK_BUDGET: usize = 250_000;
const QUERY_BYTE_BUDGET: usize = 32 * 1024 * 1024;
const RESPONSE_BYTE_BUDGET: usize = 4 * 1024 * 1024;
const QUERY_PAGE_SIZE: usize = 1_000;
const MAX_CONCURRENT_QUERIES: usize = 4;
const MAX_INVOCATION_BYTES: usize = 128;

#[derive(Debug, Clone)]
struct ActiveQuery {
    invocation: String,
    serial: u64,
    cancellation: CancellationToken,
}

impl ActiveQuery {
    fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

#[derive(Debug)]
struct ActiveEntry {
    serial: u64,
    request: String,
    cancellation: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueryPublicationError {
    Cancelled,
    Retired,
    Execution(String),
}

impl std::fmt::Display for QueryPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("federated document query was cancelled"),
            Self::Retired => {
                formatter.write_str("federated document query was retired by a newer request")
            }
            Self::Execution(error) => formatter.write_str(error),
        }
    }
}

/// Process-wide lifecycle for Semantic Inspector federation work.
///
/// A stable UI invocation key has at most one registered query. Replacing it
/// cancels the retired source jobs, while the semaphore bounds work across all
/// inspector instances. The registry is itself bounded so callers cannot turn
/// queued 30-second operations into unbounded retained state.
#[derive(Debug)]
pub(crate) struct FederationRuntime {
    gate: Arc<Semaphore>,
    active: Mutex<HashMap<String, ActiveEntry>>,
    next_serial: AtomicU64,
    max_active: usize,
}

impl Default for FederationRuntime {
    fn default() -> Self {
        Self::with_limit(MAX_CONCURRENT_QUERIES)
    }
}

impl FederationRuntime {
    fn with_limit(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            gate: Arc::new(Semaphore::new(limit)),
            active: Mutex::new(HashMap::new()),
            next_serial: AtomicU64::new(1),
            max_active: limit.saturating_mul(8),
        }
    }

    fn begin(&self, _workspace_generation: u64, invocation: &str, request: &str) -> ActiveQuery {
        let serial = self.next_serial.fetch_add(1, Ordering::SeqCst);
        let cancellation = CancellationToken::new();
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = active.remove(invocation) {
            previous.cancellation.cancel();
        } else if active.len() >= self.max_active {
            for retired in active.drain().map(|(_, entry)| entry) {
                retired.cancellation.cancel();
            }
        }
        active.insert(
            invocation.to_string(),
            ActiveEntry {
                serial,
                request: request.to_string(),
                cancellation: cancellation.clone(),
            },
        );
        ActiveQuery {
            invocation: invocation.to_string(),
            serial,
            cancellation,
        }
    }

    async fn acquire(
        &self,
        query: &ActiveQuery,
    ) -> Result<OwnedSemaphorePermit, QueryPublicationError> {
        let permit = tokio::select! {
            _ = query.cancellation.cancelled() => {
                return Err(QueryPublicationError::Cancelled);
            }
            permit = Arc::clone(&self.gate).acquire_owned() => {
                permit.map_err(|_| QueryPublicationError::Execution(
                    "federated document query executor is unavailable".to_string()
                ))?
            }
        };
        if query.cancellation.is_cancelled() {
            return Err(QueryPublicationError::Cancelled);
        }
        Ok(permit)
    }

    fn cancel(&self, invocation: &str, request: &str) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = active
            .get(invocation)
            .is_some_and(|entry| entry.request == request)
            .then(|| active.remove(invocation))
            .flatten();
        if let Some(active) = removed {
            active.cancellation.cancel();
        }
    }

    fn finish(&self, query: &ActiveQuery) -> Result<(), QueryPublicationError> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match active.get(&query.invocation) {
            Some(entry) if entry.serial == query.serial => {
                active.remove(&query.invocation);
                if query.cancellation.is_cancelled() {
                    Err(QueryPublicationError::Cancelled)
                } else {
                    Ok(())
                }
            }
            Some(_) => Err(QueryPublicationError::Retired),
            None if query.cancellation.is_cancelled() => Err(QueryPublicationError::Cancelled),
            None => Err(QueryPublicationError::Retired),
        }
    }

    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

/// One provider considered for one field, in declared precedence order.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FederatedProviderDto {
    /// Physical source name.
    pub source: String,
    /// Attribute name as the source declares it.
    pub attribute: String,
    /// `observed`, `observed-nothing`, `did-not-see-document`, or `not-asked`.
    pub verdict: String,
    /// Observed JSON value, when present.
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub value: Option<serde_json::Value>,
    /// Why the provider was not asked, when applicable.
    pub reason: Option<String>,
    /// Adapter-internal fallback declaration.
    pub declared_fallback: Vec<String>,
}

/// One semantic field and its complete source attribution.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FederatedFieldDto {
    /// Semantic field name.
    pub name: String,
    /// Selected JSON value, or `None` when every provider was absent.
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub value: Option<serde_json::Value>,
    /// Winning source, or `None` when the field is absent.
    pub winner: Option<String>,
    /// Every declared provider, including losers and absences.
    pub providers: Vec<FederatedProviderDto>,
    /// Human-readable explanation derived from the same attribution record.
    pub explanation: String,
}

/// The one assembled semantic `Document`.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FederatedDocumentDto {
    /// Workspace-relative natural key.
    pub path: String,
    /// All six schema fields in stable name order.
    pub fields: Vec<FederatedFieldDto>,
}

/// What became of one physical source.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FederatedSourceDto {
    /// Stable connector name.
    pub source: String,
    /// `ran` or `refused`.
    pub outcome: String,
    /// `virtual` or `materialized` when the source ran.
    pub execution: Option<String>,
    /// `fresh`, `cached`, or `None` when refused.
    pub freshness: Option<String>,
    /// Cached-answer age; zero for a fresh read.
    pub age_ms: Option<u32>,
    /// Physical candidates examined.
    pub scanned: u32,
    /// Rows surviving residual filtering.
    pub returned: u32,
    /// Filters executed by the source.
    pub pushed: Vec<String>,
    /// Filters executed by CatDB after the source answered.
    pub residual: Vec<String>,
    /// Typed refusal rendered for the user when the source did not run.
    pub refusal: Option<String>,
}

/// Catalog-persisted identity of the schema used for this response.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FederatedSchemaDto {
    /// Stable schema name.
    pub name: String,
    /// Persisted catalog version.
    pub version: u32,
    /// Persisted content checksum for that exact version.
    pub checksum: String,
    /// Entity names in the persisted schema.
    pub entities: Vec<String>,
    /// Arrow count.
    pub arrow_count: u32,
    /// Attribute count.
    pub attribute_count: u32,
    /// Equation count.
    pub equation_count: u32,
    /// Whether facts in the schema require approval.
    pub approval_required: bool,
    /// Objects currently committed to the graph.
    pub object_count: u32,
    /// Review proposals currently retained.
    pub proposal_count: u32,
}

/// The shipping response shown by MDReader's Semantic Inspector.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FederatedDocumentResponse {
    /// Stable identity of this execution.
    pub query_id: String,
    /// Persisted schema version/checksum when the engine has catalogued it.
    pub schema: FederatedSchemaDto,
    /// `complete` or `partial`; derived from the source reports.
    pub completeness: String,
    /// The exact requested document.
    pub document: FederatedDocumentDto,
    /// One report for filesystem, Git, and frontmatter, in execution order.
    pub sources: Vec<FederatedSourceDto>,
}

/// Query the active document through the three local physical sources.
#[tauri::command]
#[specta::specta]
pub async fn query_federated_document(
    path: String,
    invocation: String,
    request: String,
    state: tauri::State<'_, WorkspaceState>,
    runtime: tauri::State<'_, FederationRuntime>,
) -> Result<FederatedDocumentResponse, String> {
    validate_request_key("invocation", &invocation)?;
    validate_request_key("request", &request)?;
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    query_federated_document_registered(
        &state,
        &runtime,
        epoch,
        &path,
        &invocation,
        &request,
        || {},
    )
    .await
    .map_err(|error| error.to_string())
}

async fn query_federated_document_registered(
    state: &WorkspaceState,
    runtime: &FederationRuntime,
    epoch: Arc<crate::engine::Epoch>,
    path: &str,
    invocation: &str,
    request: &str,
    before_publication: impl FnOnce(),
) -> Result<FederatedDocumentResponse, QueryPublicationError> {
    let active = runtime.begin(epoch.generation, invocation, request);
    let permit = runtime.acquire(&active).await;
    let result = match permit {
        Ok(_permit) => query_federated_document_for_epoch(
            state,
            epoch,
            path,
            active.cancellation().clone(),
            || {},
            || {},
        )
        .await
        .map_err(QueryPublicationError::Execution),
        Err(error) => Err(error),
    };
    before_publication();
    runtime.finish(&active)?;
    result
}

/// Cancel one Inspector invocation, including a query waiting for a permit.
#[tauri::command]
#[specta::specta]
pub async fn cancel_federated_document(
    invocation: String,
    request: String,
    runtime: tauri::State<'_, FederationRuntime>,
) -> Result<(), String> {
    validate_request_key("invocation", &invocation)?;
    validate_request_key("request", &request)?;
    runtime.cancel(&invocation, &request);
    Ok(())
}

fn validate_request_key(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > MAX_INVOCATION_BYTES {
        return Err(format!(
            "federated document {label} must contain 1 to {MAX_INVOCATION_BYTES} bytes"
        ));
    }
    Ok(())
}

#[cfg(test)]
async fn query_federated_document_guarded(
    state: &WorkspaceState,
    path: &str,
    after_query: impl FnOnce(),
) -> Result<FederatedDocumentResponse, String> {
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    query_federated_document_for_epoch(
        state,
        epoch,
        path,
        CancellationToken::new(),
        after_query,
        || {},
    )
    .await
}

#[cfg(test)]
async fn query_federated_document_guarded_with_source_observer(
    state: &WorkspaceState,
    path: &str,
    before_sources: impl FnOnce() + Send + 'static,
) -> Result<FederatedDocumentResponse, String> {
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    query_federated_document_for_epoch(
        state,
        epoch,
        path,
        CancellationToken::new(),
        || {},
        before_sources,
    )
    .await
}

#[cfg(test)]
async fn query_federated_document_runtime_guarded(
    state: &WorkspaceState,
    runtime: &FederationRuntime,
    path: &str,
    invocation: &str,
    request: &str,
    before_publication: impl FnOnce(),
) -> Result<FederatedDocumentResponse, QueryPublicationError> {
    let epoch = state.current().ok_or_else(|| {
        QueryPublicationError::Execution("no workspace has been opened".to_string())
    })?;
    query_federated_document_registered(
        state,
        runtime,
        epoch,
        path,
        invocation,
        request,
        before_publication,
    )
    .await
}

async fn query_federated_document_for_epoch(
    state: &WorkspaceState,
    epoch: Arc<crate::engine::Epoch>,
    path: &str,
    cancellation: CancellationToken,
    after_query: impl FnOnce(),
    before_sources: impl FnOnce() + Send + 'static,
) -> Result<FederatedDocumentResponse, String> {
    let worker_epoch = Arc::clone(&epoch);
    let worker_path = path.to_string();
    let response = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("could not start the federation worker runtime: {error}"))?;
        runtime.block_on(query_federated_document_unpublished(
            &worker_epoch,
            &worker_path,
            cancellation,
            before_sources,
        ))
    })
    .await
    .map_err(|error| format!("federated document worker failed: {error}"))??;
    after_query();
    // EPOCH-GUARDED
    epoch
        .finish(state, response)
        .map_err(|error| error.to_string())
}

async fn query_federated_document_unpublished(
    epoch: &Arc<crate::engine::Epoch>,
    path: &str,
    cancellation: CancellationToken,
    before_sources: impl FnOnce(),
) -> Result<FederatedDocumentResponse, String> {
    epoch.validate_root()?;
    let schema = persisted_document_schema(epoch)?;
    let source_root = epoch.source_root();
    let exact = resolve_within(&source_root.path, std::path::Path::new(path))?;
    if !exact.is_file() {
        return Err(format!("not a file: {}", exact.display()));
    }
    if exact
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_none_or(|extension| {
            !extension.eq_ignore_ascii_case("md") && !extension.eq_ignore_ascii_case("markdown")
        })
    {
        return Err(format!("not a markdown document: {}", exact.display()));
    }
    let relative = source_root
        .relative_path(&exact)
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .replace('\\', "/");

    before_sources();
    let filesystem = FilesystemSource::new(&source_root.path);
    let git = GitSource::open(&source_root.path);
    let frontmatter = FrontmatterSource::new(&source_root.path);
    let sources: Vec<&dyn DataSource> = vec![&filesystem, &git, &frontmatter];
    let federation = Federation::new(sources.clone(), &catdb_validation::EggProver::default())
        .map_err(|error| error.to_string())?;
    let query = federation
        .scope_exact_path(&relative)
        .map_err(|error| error.to_string())?;
    let context = ReadContext::new(
        Uuid::new_v4(),
        Instant::now()
            .checked_add(QUERY_DEADLINE)
            .ok_or_else(|| "federated query deadline overflow".to_string())?,
        QUERY_WORK_BUDGET,
        QUERY_BYTE_BUDGET,
        QUERY_PAGE_SIZE,
        cancellation,
    )
    .map_err(|error| error.to_string())?;
    let executor = Executor::new(ExecutionPolicy::default());
    let answer = executor
        .execute(&sources, &query, &context)
        .await
        .map_err(|error| error.to_string())?;
    let completeness = match answer.completeness() {
        Completeness::Complete => "complete".to_string(),
        Completeness::Partial { .. } => "partial".to_string(),
    };
    let documents = federation
        .assemble_from(&answer)
        .map_err(|error| error.to_string())?;
    let document = documents
        .into_iter()
        .find(|document| document.path() == relative)
        .ok_or_else(|| format!("the federated sources did not return `{relative}`"))?;
    let sources = answer
        .reports
        .iter()
        .map(source_dto)
        .collect::<Result<Vec<_>, _>>()?;
    let response = FederatedDocumentResponse {
        query_id: context.query_id().to_string(),
        schema,
        completeness,
        document: document_dto(&document),
        sources,
    };
    enforce_response_budget(&response)?;
    Ok(response)
}

fn document_dto(document: &mdreader_domain::federated::Document) -> FederatedDocumentDto {
    let fields = document
        .fields()
        .values()
        .map(|field| {
            let providers = field
                .verdicts
                .iter()
                .map(|(provider, verdict)| {
                    let (value, reason) = match verdict {
                        ProviderVerdict::Observed(value) => (Some(value.clone()), None),
                        ProviderVerdict::NotAsked { reason } => (None, Some(reason.clone())),
                        ProviderVerdict::ObservedNothing | ProviderVerdict::DidNotSeeDocument => {
                            (None, None)
                        }
                    };
                    FederatedProviderDto {
                        source: provider.source.to_string(),
                        attribute: provider.attr.to_string(),
                        verdict: verdict.as_str().to_string(),
                        value,
                        reason,
                        declared_fallback: provider.declared_fallback.clone(),
                    }
                })
                .collect();
            FederatedFieldDto {
                name: field.field.to_string(),
                value: field.value().cloned(),
                winner: field.source().map(ToString::to_string),
                providers,
                explanation: field.explain(),
            }
        })
        .collect();
    FederatedDocumentDto {
        path: document.path().to_string(),
        fields,
    }
}

fn persisted_document_schema(
    epoch: &Arc<crate::engine::Epoch>,
) -> Result<FederatedSchemaDto, String> {
    let engine = epoch.db().ok_or_else(|| {
        "the federated Document schema has no persisted catalog version/checksum yet".to_string()
    })?;
    let stored = engine
        .latest_schema_version(FEDERATED_SCHEMA)
        .map_err(|error| format!("could not resolve the persisted Document schema: {error}"))?
        .ok_or_else(|| {
            "the federated Document schema has no persisted catalog version/checksum yet"
                .to_string()
        })?;
    if stored.version == 0 {
        return Err(
            "the federated Document schema has no persisted catalog version/checksum yet"
                .to_string(),
        );
    }
    let persisted = stored
        .schema()
        .map_err(|error| format!("could not reconstruct the persisted Document schema: {error}"))?;
    let actual_checksum = catdb_engine::schema_checksum(&persisted);
    if actual_checksum != stored.checksum {
        return Err(format!(
            "the persisted Document schema v{} records checksum {}, but its stored body hashes to \
             {}; source execution is refused",
            stored.version, stored.checksum, actual_checksum
        ));
    }
    let active_checksum = catdb_engine::schema_checksum(&document_schema());
    if actual_checksum != active_checksum {
        return Err(format!(
            "the persisted Document schema v{} ({}) does not match the active domain schema ({}); \
             source execution is refused",
            stored.version, actual_checksum, active_checksum
        ));
    }

    let summary = crate::overlay::schema_summary_for(epoch, FEDERATED_SCHEMA)
        .ok_or_else(|| "the federated Document schema is not registered".to_string())?;
    let response = persisted_schema(summary)?;
    if response.version != stored.version || response.checksum != stored.checksum {
        return Err(
            "the persisted Document schema identity changed while the query was being resolved"
                .to_string(),
        );
    }
    Ok(response)
}

fn persisted_schema(summary: SchemaSummary) -> Result<FederatedSchemaDto, String> {
    let version = summary.version.ok_or_else(|| {
        "the federated Document schema has no persisted catalog version/checksum yet".to_string()
    })?;
    let checksum = summary
        .checksum
        .filter(|checksum| !checksum.is_empty())
        .ok_or_else(|| {
            "the federated Document schema has no persisted catalog version/checksum yet"
                .to_string()
        })?;
    Ok(FederatedSchemaDto {
        name: summary.name,
        version,
        checksum,
        entities: summary.entities,
        arrow_count: summary.arrow_count,
        attribute_count: summary.attribute_count,
        equation_count: summary.equation_count,
        approval_required: summary.approval_required,
        object_count: summary.object_count,
        proposal_count: summary.proposal_count,
    })
}

fn enforce_response_budget(response: &FederatedDocumentResponse) -> Result<(), String> {
    let mut writer = BoundedWriter {
        written: 0,
        maximum: RESPONSE_BYTE_BUDGET,
    };
    serde_json::to_writer(&mut writer, response)
        .map_err(|_| "federated document response byte budget exceeded".to_string())
}

struct BoundedWriter {
    written: usize,
    maximum: usize,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .written
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("response byte budget overflow"))?;
        if next > self.maximum {
            return Err(std::io::Error::other("response byte budget exceeded"));
        }
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn source_dto(report: &catdb_federation::SourceReport) -> Result<FederatedSourceDto, String> {
    match &report.outcome {
        Outcome::Ran(ran) => {
            let (freshness, age_ms) = match ran.execution {
                catdb_federation::Execution::Virtual
                | catdb_federation::Execution::Materialized(Freshness::Fresh) => {
                    ("fresh".to_string(), 0_u32)
                }
                catdb_federation::Execution::Materialized(Freshness::Cached { age_ms }) => (
                    "cached".to_string(),
                    u32::try_from(age_ms)
                        .map_err(|_| "cached answer age exceeds the application contract")?,
                ),
            };
            Ok(FederatedSourceDto {
                source: report.source.to_string(),
                outcome: "ran".to_string(),
                execution: Some(ran.execution.mode().to_string()),
                freshness: Some(freshness),
                age_ms: Some(age_ms),
                scanned: u32::try_from(ran.scanned).map_err(|_| {
                    "source scan count exceeds the application contract".to_string()
                })?,
                returned: u32::try_from(ran.returned).map_err(|_| {
                    "source return count exceeds the application contract".to_string()
                })?,
                pushed: ran.pushed.clone(),
                residual: ran.residual.clone(),
                refusal: None,
            })
        }
        Outcome::Refused { reason } => Ok(FederatedSourceDto {
            source: report.source.to_string(),
            outcome: "refused".to_string(),
            execution: None,
            freshness: None,
            age_ms: None,
            scanned: 0,
            returned: 0,
            pushed: Vec::new(),
            residual: Vec::new(),
            refusal: Some(reason.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use catdb_embed::{Embedder, HashEmbedder};
    use catdb_engine::EngineConfig;

    use super::{
        query_federated_document, query_federated_document_guarded,
        query_federated_document_guarded_with_source_observer,
        query_federated_document_runtime_guarded, QueryPublicationError,
    };
    use crate::commands::WorkspaceState;

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fingerprint(root: &Path) -> BTreeMap<String, (u64, blake3::Hash, std::time::SystemTime)> {
        walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(path).expect("read source bytes");
                let metadata = std::fs::metadata(path).expect("read source metadata");
                (
                    relative,
                    (
                        metadata.len(),
                        blake3::hash(&bytes),
                        metadata.modified().expect("source mtime"),
                    ),
                )
            })
            .collect()
    }

    fn committed_workspace() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temporary workspace");
        std::fs::write(
            directory.path().join("a file.md"),
            "---\ntitle: Declared title\nauthor: Ada\n---\n# Different heading\n\nBody.\n",
        )
        .expect("write markdown");
        git(directory.path(), &["init", "-q"]);
        git(directory.path(), &["config", "user.name", "Committer"]);
        git(
            directory.path(),
            &["config", "user.email", "committer@example.invalid"],
        );
        git(directory.path(), &["add", "a file.md"]);
        git(directory.path(), &["commit", "-qm", "seed"]);
        directory
    }

    fn catalogued_state(root: &Path) -> WorkspaceState {
        catalogued_state_with_registration(root, mdreader_ingest::registration())
    }

    fn catalogued_state_with_registration(
        root: &Path,
        registration: catdb_engine::DomainRegistration,
    ) -> WorkspaceState {
        let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
        let mut config = EngineConfig::in_memory(embedder).with_actor("federation-product-test");
        config.domains = vec![registration];
        let db = Arc::new(catdb_engine::EmbeddedCatDb::open(config).expect("open engine"));
        let state = WorkspaceState::granted(root.to_path_buf());
        state
            .current()
            .expect("active epoch")
            .install_engine_for_test(db);
        state
    }

    #[tokio::test]
    async fn one_native_query_returns_three_source_reports_and_per_field_attribution() {
        let directory = committed_workspace();
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);
        let document = root.join("a file.md");
        let before = fingerprint(&root);

        let answer = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect("federated document query");

        assert_eq!(answer.completeness, "complete");
        assert_eq!(answer.sources.len(), 3);
        assert!(answer.sources.iter().all(|source| source.outcome == "ran"));
        assert_eq!(answer.document.path, "a file.md");
        assert_eq!(answer.document.fields.len(), 6);
        let title = answer
            .document
            .fields
            .iter()
            .find(|field| field.name == "title")
            .expect("title field");
        assert_eq!(title.winner.as_deref(), Some("frontmatter"));
        assert_eq!(
            title.value.as_ref(),
            Some(&serde_json::json!("Declared title"))
        );
        let modified = answer
            .document
            .fields
            .iter()
            .find(|field| field.name == "modifiedAt")
            .expect("modifiedAt field");
        assert_eq!(modified.winner.as_deref(), Some("filesystem"));
        assert!(
            modified
                .providers
                .iter()
                .any(|provider| provider.source == "git" && provider.verdict == "observed"),
            "the losing git observation stays visible"
        );
        assert_eq!(
            fingerprint(&root),
            before,
            "the product query may not change bytes, mtimes, git metadata, or membership"
        );
    }

    #[tokio::test]
    async fn native_query_refuses_a_document_outside_the_active_library() {
        let directory = committed_workspace();
        let outsider = tempfile::tempdir().expect("outside directory");
        let outside_path = outsider.path().join("outside.md");
        std::fs::write(&outside_path, "# Outside").expect("write outsider");
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);

        let error = query_federated_document_guarded(
            &state,
            outside_path.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect_err("outside path must not cross the workspace grant");

        assert!(error.contains("escapes the workspace"), "{error}");
    }

    #[tokio::test]
    async fn native_query_discards_an_answer_when_the_library_switches_before_publish() {
        let directory = committed_workspace();
        let replacement = tempfile::tempdir().expect("replacement workspace");
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let replacement_root =
            std::fs::canonicalize(replacement.path()).expect("canonical replacement workspace");
        let state = catalogued_state(&root);
        let document = root.join("a file.md");

        let error = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {
                state
                    .switch(replacement_root)
                    .expect("switch to replacement workspace");
            },
        )
        .await
        .expect_err("an answer from the previous library must never publish");

        assert!(
            error.contains("workspace changed during the operation"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn native_query_discards_an_answer_when_the_same_library_reopens() {
        let directory = committed_workspace();
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);
        let document = root.join("a file.md");

        let error = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {
                state
                    .switch(root.clone())
                    .expect("reopen the same workspace as a new epoch");
            },
        )
        .await
        .expect_err("same-root reopen must revoke the captured generation");

        assert!(
            error.contains("workspace changed during the operation"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn native_query_accepts_the_markdown_extension_used_by_the_reader() {
        let directory = committed_workspace();
        let old = directory.path().join("a file.md");
        let document = directory.path().join("a file.markdown");
        std::fs::rename(&old, &document).expect("rename to long Markdown extension");
        git(directory.path(), &["add", "-A"]);
        git(
            directory.path(),
            &["commit", "-qm", "use markdown extension"],
        );
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);

        let answer = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect("reader-supported .markdown document");

        assert_eq!(answer.document.path, "a file.markdown");
    }

    #[tokio::test]
    async fn native_query_requires_the_catalogued_schema_version_and_checksum() {
        let directory = committed_workspace();
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = WorkspaceState::granted(root.clone());
        let document = root.join("a file.md");
        let source_boundaries = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&source_boundaries);

        let error = query_federated_document_guarded_with_source_observer(
            &state,
            document.to_str().expect("utf-8 test path"),
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await
        .expect_err("an unversioned registration is not native schema acceptance");

        assert!(
            error.contains("persisted") && error.contains("checksum"),
            "{error}"
        );
        assert_eq!(
            source_boundaries.load(Ordering::SeqCst),
            0,
            "an unversioned schema must not reach filesystem, Git, or frontmatter"
        );
    }

    #[tokio::test]
    async fn invalid_persisted_schema_is_refused_before_any_source_boundary() {
        let directory = committed_workspace();
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let mut registration = mdreader_ingest::registration();
        let document_schema = registration
            .schemas
            .iter_mut()
            .find(|schema| schema.name.as_str() == mdreader_domain::federated::SCHEMA)
            .expect("document schema registration");
        document_schema.add_entity("UnregisteredMeaning");
        let state = catalogued_state_with_registration(&root, registration);
        let document = root.join("a file.md");
        let source_boundaries = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&source_boundaries);

        let error = query_federated_document_guarded_with_source_observer(
            &state,
            document.to_str().expect("utf-8 test path"),
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await
        .expect_err("a persisted schema outside the active domain meaning must be refused");

        assert!(
            error.contains("persisted") && error.contains("active"),
            "{error}"
        );
        assert_eq!(
            source_boundaries.load(Ordering::SeqCst),
            0,
            "schema identity must resolve before filesystem, Git, or frontmatter is constructed, \
             planned, or read"
        );
    }

    #[tokio::test]
    async fn native_response_keeps_json_values_typed_instead_of_json_in_strings() {
        let directory = committed_workspace();
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);
        let document = root.join("a file.md");

        let answer = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect("federated document query");
        let wire = serde_json::to_value(answer).expect("serialize wire response");
        let title = wire["document"]["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|field| field["name"] == "title")
            .expect("title field");
        assert_eq!(title["value"], serde_json::json!("Declared title"));
        assert_eq!(
            title["providers"][0]["value"],
            serde_json::json!("Declared title")
        );
        let modified = wire["document"]["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|field| field["name"] == "modifiedAt")
            .expect("modified field");
        assert!(
            modified["value"].is_number(),
            "timestamps remain JSON numbers: {}",
            modified["value"]
        );
    }

    #[tokio::test]
    async fn native_response_bounds_the_duplicated_serialized_payload() {
        let directory = committed_workspace();
        let document = directory.path().join("a file.md");
        let body = "x".repeat(2_500_000);
        std::fs::write(
            &document,
            format!("---\ntitle: Declared title\nauthor: Ada\n---\n# Heading\n\n{body}"),
        )
        .expect("write large bounded source");
        git(directory.path(), &["add", "-A"]);
        git(directory.path(), &["commit", "-qm", "large source"]);
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);

        let result = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {},
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("wire duplication must have its own serialized-output bound"),
        };

        assert!(error.contains("response byte budget"), "{error}");
    }

    #[tokio::test]
    async fn product_queries_do_not_claim_a_cross_command_git_cache_they_cannot_reuse() {
        let directory = committed_workspace();
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);
        let document = root.join("a file.md");

        let answer = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect("federated document query");

        assert!(
            answer
                .sources
                .iter()
                .all(|source| source.execution.as_deref() == Some("virtual")),
            "a command-local executor cannot advertise a five-minute reusable cache: {:?}",
            answer
                .sources
                .iter()
                .map(|source| (&source.source, &source.execution))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn generated_tauri_wrapper_executes_on_the_runtime_without_nested_block_on() {
        let app = tauri::test::mock_builder()
            .manage(WorkspaceState::default())
            .manage(super::FederationRuntime::default())
            .invoke_handler(tauri::generate_handler![query_federated_document])
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock application");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("build mock webview");
        let response = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "query_federated_document".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .expect("test URL"),
                body: serde_json::json!({
                    "path": "/missing.md",
                    "invocation": "wrapper-boundary-test",
                    "request": "request-1"
                })
                .into(),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        let error = response.expect_err("no workspace is a typed command error");
        assert!(
            error.to_string().contains("no workspace has been opened"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn runtime_cancels_superseded_invocations_and_bounds_global_work() {
        let runtime = super::FederationRuntime::with_limit(1);
        let first = runtime.begin(7, "inspector", "request-1");
        let first_permit = runtime.acquire(&first).await.expect("first permit");

        let replacement = runtime.begin(8, "inspector", "request-2");
        assert!(
            first.cancellation().is_cancelled(),
            "a same-root generation change cancels backend work, not only UI publication"
        );
        assert_eq!(
            runtime.active_count(),
            1,
            "one invocation cannot accumulate retired 30-second jobs"
        );

        runtime.cancel("inspector", "request-1");
        assert!(
            !replacement.cancellation().is_cancelled(),
            "late cleanup from a retired request cannot cancel its replacement"
        );

        let other = runtime.begin(8, "other-inspector", "request-1");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), runtime.acquire(&other))
                .await
                .is_err(),
            "the process-wide permit bounds source jobs across invocations"
        );
        drop(first_permit);
        let _other_permit = runtime
            .acquire(&other)
            .await
            .expect("permit becomes available");
        runtime
            .finish(&replacement)
            .expect("replacement is still current");
        runtime
            .finish(&other)
            .expect("other request is still current");
    }

    #[tokio::test]
    async fn a_real_completed_query_replaced_before_ipc_publication_is_retired() {
        let directory = committed_workspace();
        let root = std::fs::canonicalize(directory.path()).expect("canonical test workspace");
        let state = catalogued_state(&root);
        let runtime = super::FederationRuntime::with_limit(1);
        let document = root.join("a file.md");

        let error = query_federated_document_runtime_guarded(
            &state,
            &runtime,
            document.to_str().expect("utf-8 test path"),
            "inspector",
            "request-1",
            || {
                runtime.begin(state.generation(), "inspector", "request-2");
            },
        )
        .await
        .expect_err("a superseded serial cannot return a successful IPC response");

        assert_eq!(error, QueryPublicationError::Retired);
        runtime.cancel("inspector", "request-2");
    }
}
