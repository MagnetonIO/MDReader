//! Native product boundary for the federated `Document` concept.

use std::time::{Duration, Instant};

use catdb_connectors::{
    CancellationToken, DataSource, FilesystemSource, FrontmatterSource, GitSource, ReadContext,
    SourceId,
};
use catdb_federation::{Completeness, ExecutionPolicy, Executor, Freshness, Outcome};
use mdreader_domain::federated::{Federation, ProviderVerdict, SCHEMA as FEDERATED_SCHEMA};
use serde::Serialize;
use specta::Type;
use uuid::Uuid;

use crate::commands::{resolve_within, WorkspaceState};
use crate::overlay::SchemaSummary;

const QUERY_DEADLINE: Duration = Duration::from_secs(30);
const QUERY_WORK_BUDGET: usize = 250_000;
const QUERY_BYTE_BUDGET: usize = 32 * 1024 * 1024;
const QUERY_PAGE_SIZE: usize = 1_000;

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
    pub value: Option<String>,
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
    pub value: Option<String>,
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

/// The shipping response shown by MDReader's Semantic Inspector.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FederatedDocumentResponse {
    /// Stable identity of this execution.
    pub query_id: String,
    /// Persisted schema version/checksum when the engine has catalogued it.
    pub schema: SchemaSummary,
    /// `complete` or `partial`; derived from the source reports.
    pub completeness: String,
    /// The exact requested document.
    pub document: FederatedDocumentDto,
    /// One report for filesystem, Git, and frontmatter, in execution order.
    pub sources: Vec<FederatedSourceDto>,
}

/// Query the active document through the three local physical sources.
#[tauri::command(async)]
#[specta::specta]
pub fn query_federated_document(
    path: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<FederatedDocumentResponse, String> {
    tauri::async_runtime::block_on(query_federated_document_guarded(&state, &path, || {}))
}

async fn query_federated_document_guarded(
    state: &WorkspaceState,
    path: &str,
    after_query: impl FnOnce(),
) -> Result<FederatedDocumentResponse, String> {
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    epoch.validate_root()?;
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
        CancellationToken::new(),
    )
    .map_err(|error| error.to_string())?;
    let executor = Executor::new(
        ExecutionPolicy::default()
            .materialize(SourceId::new(GitSource::ID), Duration::from_secs(300)),
    );
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
    let schema = crate::overlay::schema_summary_for(&epoch, FEDERATED_SCHEMA)
        .ok_or_else(|| "the federated Document schema is not registered".to_string())?;
    let response = FederatedDocumentResponse {
        query_id: context.query_id().to_string(),
        schema,
        completeness,
        document: document_dto(&document),
        sources,
    };
    after_query();
    // EPOCH-GUARDED
    epoch
        .finish(state, response)
        .map_err(|error| error.to_string())
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
                        ProviderVerdict::Observed(value) => (Some(value.to_string()), None),
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
                value: field.value().map(ToString::to_string),
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
    use std::sync::Arc;
    use std::time::Duration;

    use catdb_embed::{Embedder, HashEmbedder};
    use catdb_engine::EngineConfig;

    use super::{query_federated_document, query_federated_document_guarded};
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
        let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
        let mut config = EngineConfig::in_memory(embedder).with_actor("federation-product-test");
        config.domains = vec![mdreader_ingest::registration()];
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
        assert_eq!(title.value.as_deref(), Some("\"Declared title\""));
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
        let state = WorkspaceState::granted(
            std::fs::canonicalize(directory.path()).expect("canonical test workspace"),
        );

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

        let error = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect_err("an unversioned registration is not native schema acceptance");

        assert!(
            error.contains("persisted") && error.contains("checksum"),
            "{error}"
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
                    "invocation": "wrapper-boundary-test"
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
        let first = runtime.begin(7, "inspector");
        let first_permit = runtime.acquire(&first).await.expect("first permit");

        let replacement = runtime.begin(8, "inspector");
        assert!(
            first.cancellation().is_cancelled(),
            "a same-root generation change cancels backend work, not only UI publication"
        );
        assert_eq!(
            runtime.active_count(),
            1,
            "one invocation cannot accumulate retired 30-second jobs"
        );

        let other = runtime.begin(8, "other-inspector");
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
        runtime.finish(&replacement);
        runtime.finish(&other);
    }
}
