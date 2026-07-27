//! One operational entity-query boundary for the complete MDReader Library.
//!
//! The renderer selects Library memberships. This module expands each selected
//! membership into its registered physical sources, pins one immutable catalog
//! snapshot, and delegates the complete read to CatDB's shared entity service.
//! It never joins provider-specific responses in the application layer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use catdb_api_types::{EntityQueryRequest, EntityQueryResponse, SourceExecution};
use catdb_connectors::{CancellationToken, DataSource, ReadContext};
use catdb_core::Mapping;
use catdb_federation::{
    CatalogResolveError, CatalogResolver, EntityQueryError, EntityQueryService, ResolvedCatalog,
    RuntimeCatalog, SourceBinding,
};
use catdb_policy::{
    Action, Effect, PolicyDocument, PolicyRule, Principal, ResourceScope, SubjectSelector,
};
use catdb_storage::{
    catalog_content_checksum, CatalogRevision, Compatibility, MappingCapability, MappingVersion,
    PolicyVersion, SchemaContent, SchemaVersion, SourceVersion,
};
use serde::Serialize;
use specta::Type;
use uuid::Uuid;

const LIBRARY_DEPLOYMENT_ID: Uuid = Uuid::from_u128(0xdffda89b_9fb0_438d_93b3_285068ccb5f1);
const LIBRARY_SERVICE_ID: Uuid = Uuid::from_u128(0xa4d862e5_1efe_457b_9488_0db414effe9d);
const LIBRARY_RESOLVER_ID: Uuid = Uuid::from_u128(0x5ab56e98_b720_4913_86f7_84c646cc6e84);
const PHYSICAL_SOURCE_NAMESPACE: Uuid = Uuid::from_u128(0x6d7c4005_f53a_41af_9b6c_7c6ff1ca9b4e);
const VERSION_NAMESPACE: Uuid = Uuid::from_u128(0x7e60a029_b565_47a8_907e_820caf6e4d8a);
const QUERY_DEADLINE: Duration = Duration::from_secs(30);
const QUERY_WORK_BUDGET: usize = 100_000;
const QUERY_BYTE_BUDGET: usize = 32 * 1024 * 1024;
const QUERY_PAGE_SIZE: usize = 256;
const QUERY_CONCURRENCY: usize = 4;

/// Stable query identity for one persisted Library membership record.
pub(crate) fn membership_id(registry_id: &str) -> Uuid {
    Uuid::new_v5(&PHYSICAL_SOURCE_NAMESPACE, registry_id.as_bytes())
}

/// One registered Library membership and the physical sources it authorizes.
#[derive(Clone)]
pub(crate) struct LibraryQueryMember {
    /// Stable identity presented in `EntityQueryRequest.source_ids`.
    pub membership_id: Uuid,
    /// User-facing source label captured with membership.
    pub name: String,
    /// Source family, used only for presentation and open-original behavior.
    pub kind: LibraryQueryMemberKind,
    /// Whether records from this member can be edited by MDReader.
    pub editable: bool,
    /// Physical connectors admitted by the same registry snapshot.
    pub sources: Vec<LibraryPhysicalSource>,
}

/// Source family for one Library membership.
#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LibraryQueryMemberKind {
    LocalFolder,
    GoogleDrive,
    Dropbox,
}

/// One read-only physical source beneath a Library membership.
#[derive(Clone)]
pub(crate) struct LibraryPhysicalSource {
    /// Stable source identity within the immutable catalog.
    pub source_id: Uuid,
    /// Connector implementation.
    pub source: Arc<dyn DataSource>,
    /// Complete mapping into `document.Document`.
    pub mapping: Mapping,
}

impl LibraryPhysicalSource {
    /// Derive a stable physical identity from a membership and connector.
    pub(crate) fn new(membership_id: Uuid, source: Arc<dyn DataSource>, mapping: Mapping) -> Self {
        let mut identity = membership_id.as_bytes().to_vec();
        identity.extend_from_slice(source.id().as_str().as_bytes());
        Self {
            source_id: Uuid::new_v5(&PHYSICAL_SOURCE_NAMESPACE, &identity),
            source,
            mapping,
        }
    }
}

/// How a returned physical source relates to the user-selected Library member.
#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct LibrarySourceExecution {
    pub membership_id: Uuid,
    pub name: String,
    pub kind: LibraryQueryMemberKind,
    pub source_ids: Vec<Uuid>,
    pub editable: bool,
}

/// One exact CatDB answer plus the membership boundary used to produce it.
#[derive(Debug, Clone, Serialize, Type, PartialEq)]
pub(crate) struct LibraryEntityQueryResponse {
    pub answer: EntityQueryResponse,
    pub memberships: Vec<LibrarySourceExecution>,
}

/// A Library query failed before it could return one exact answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LibraryQueryError {
    InvalidRequest,
    NoEnabledSources,
    UnknownSource,
    InvalidCatalog,
    AuthorizationDenied,
    Refused,
    Incomplete { source_ids: Vec<Uuid> },
    Stale { source_ids: Vec<Uuid> },
    SourceFailed { source_id: Uuid },
    SourceProtocol { source_id: Uuid },
    ExecutionFailed { sources: Vec<SourceExecution> },
    Cancelled,
    Timeout,
    LimitExceeded,
    InvalidResponse,
    QueryFailed,
}

impl std::fmt::Display for LibraryQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "invalid-library-query",
            Self::NoEnabledSources => "library-has-no-enabled-sources",
            Self::UnknownSource => "library-source-not-registered",
            Self::InvalidCatalog => "library-catalog-invalid",
            Self::AuthorizationDenied => "library-query-authorization-denied",
            Self::Refused => "library-query-refused",
            Self::Incomplete { .. } => "library-query-incomplete",
            Self::Stale { .. } => "library-query-stale",
            Self::SourceFailed { .. } => "library-query-source-failed",
            Self::SourceProtocol { .. } => "library-query-source-protocol-failed",
            Self::ExecutionFailed { .. } => "library-query-execution-failed",
            Self::Cancelled => "library-query-cancelled",
            Self::Timeout => "library-query-timed-out",
            Self::LimitExceeded => "library-query-limit-exceeded",
            Self::InvalidResponse => "library-query-invalid-response",
            Self::QueryFailed => "library-query-failed",
        })
    }
}

impl std::error::Error for LibraryQueryError {}

fn map_entity_query_error(error: EntityQueryError) -> LibraryQueryError {
    match error {
        EntityQueryError::InvalidRequest(_)
        | EntityQueryError::InvalidQuery(_)
        | EntityQueryError::UnknownRequestedSource
        | EntityQueryError::InvalidCursor
        | EntityQueryError::CursorMismatch
        | EntityQueryError::CursorStateExpired
        | EntityQueryError::CursorStateUnavailable
        | EntityQueryError::CursorReplay => LibraryQueryError::InvalidRequest,
        EntityQueryError::Catalog(_) => LibraryQueryError::InvalidCatalog,
        EntityQueryError::Authorization(_) => LibraryQueryError::AuthorizationDenied,
        EntityQueryError::Refused => LibraryQueryError::Refused,
        EntityQueryError::Incomplete { source_ids } => LibraryQueryError::Incomplete { source_ids },
        EntityQueryError::Stale { source_ids } => LibraryQueryError::Stale { source_ids },
        EntityQueryError::SourceFailed { source_id } => {
            LibraryQueryError::SourceFailed { source_id }
        }
        EntityQueryError::SourceProtocol { source_id } => {
            LibraryQueryError::SourceProtocol { source_id }
        }
        EntityQueryError::ExecutionFailed { sources } => {
            LibraryQueryError::ExecutionFailed { sources }
        }
        EntityQueryError::Cancelled => LibraryQueryError::Cancelled,
        EntityQueryError::Timeout => LibraryQueryError::Timeout,
        EntityQueryError::RowBudgetExceeded
        | EntityQueryError::ByteBudgetExceeded
        | EntityQueryError::ContinuationStateCapacityExceeded => LibraryQueryError::LimitExceeded,
        EntityQueryError::InvalidProvenance => LibraryQueryError::InvalidResponse,
    }
}

#[derive(Debug)]
struct LibraryCatalogResolver {
    catalog: RuntimeCatalog,
}

#[async_trait]
impl CatalogResolver for LibraryCatalogResolver {
    async fn resolve(
        &self,
        requested_revision_id: Option<Uuid>,
    ) -> Result<ResolvedCatalog, CatalogResolveError> {
        if requested_revision_id
            .is_some_and(|requested| requested != self.catalog.revision.revision_id)
        {
            return Err(CatalogResolveError::RevisionNotFound);
        }
        Ok(ResolvedCatalog {
            resolver_id: LIBRARY_RESOLVER_ID,
            catalog: self.catalog.clone(),
        })
    }
}

struct LibraryRuntime {
    catalog: RuntimeCatalog,
    bindings: Vec<SourceBinding>,
    memberships: Vec<LibrarySourceExecution>,
}

fn version_id(kind: &str, identity: Uuid, checksum: &str) -> Uuid {
    let mut name = kind.as_bytes().to_vec();
    name.extend_from_slice(identity.as_bytes());
    name.extend_from_slice(checksum.as_bytes());
    Uuid::new_v5(&VERSION_NAMESPACE, &name)
}

fn build_runtime(
    workspace_id: Uuid,
    members: Vec<LibraryQueryMember>,
) -> Result<LibraryRuntime, LibraryQueryError> {
    if members.is_empty() || members.iter().any(|member| member.sources.is_empty()) {
        return Err(LibraryQueryError::NoEnabledSources);
    }
    let mut membership_ids = BTreeSet::new();
    let mut physical_ids = BTreeSet::new();
    for member in &members {
        if member.membership_id.is_nil()
            || !membership_ids.insert(member.membership_id)
            || member
                .sources
                .iter()
                .any(|source| source.source_id.is_nil() || !physical_ids.insert(source.source_id))
        {
            return Err(LibraryQueryError::InvalidCatalog);
        }
    }

    let semantic = mdreader_domain::library::document_schema();
    let semantic_content = SchemaContent::new(
        serde_json::to_value(&semantic).map_err(|_| LibraryQueryError::InvalidCatalog)?,
    );
    let semantic_schema_version_id =
        version_id("semantic-schema", workspace_id, &semantic_content.checksum);
    let semantic_version = SchemaVersion {
        schema_version_id: semantic_schema_version_id,
        name: semantic.name.to_string(),
        content_id: semantic_content.content_id,
        compatibility: Compatibility::Backward,
    };

    let mut schema_contents = vec![semantic_content];
    let mut schema_versions = vec![semantic_version];
    let mut mapping_versions = Vec::new();
    let mut source_versions = Vec::new();
    let mut bindings = Vec::new();
    let mut reports = Vec::new();

    for member in members {
        let mut member_sources = Vec::new();
        for physical in member.sources {
            let schema_content = SchemaContent::new(
                serde_json::to_value(physical.source.schema())
                    .map_err(|_| LibraryQueryError::InvalidCatalog)?,
            );
            let source_schema_version_id = version_id(
                "physical-schema",
                physical.source_id,
                &schema_content.checksum,
            );
            let source_schema_version = SchemaVersion {
                schema_version_id: source_schema_version_id,
                name: physical.source.schema().name.to_string(),
                content_id: schema_content.content_id,
                compatibility: Compatibility::Backward,
            };
            let mapping_spec = serde_json::to_value(&physical.mapping)
                .map_err(|_| LibraryQueryError::InvalidCatalog)?;
            let mapping_checksum = catalog_content_checksum(&mapping_spec);
            let mapping_version_id = version_id("mapping", physical.source_id, &mapping_checksum);
            let mapping_version = MappingVersion {
                mapping_version_id,
                name: format!("mdreader.Document.{}", physical.source.id()),
                source_schema_version_id,
                target_schema_version_id: semantic_schema_version_id,
                spec: mapping_spec,
                checksum: mapping_checksum,
                capability: MappingCapability::ReadOnly,
            };
            let source_spec = physical.source.source_spec();
            let source_checksum = catalog_content_checksum(&source_spec);
            let source_version_id = version_id("source", physical.source_id, &source_checksum);
            let source_version = SourceVersion {
                source_version_id,
                source_id: physical.source_id,
                spec: source_spec,
                checksum: source_checksum,
            };
            mapping_version
                .verify()
                .and_then(|()| source_version.verify())
                .map_err(|_| LibraryQueryError::InvalidCatalog)?;

            schema_contents.push(schema_content);
            schema_versions.push(source_schema_version);
            mapping_versions.push(mapping_version);
            source_versions.push(source_version);
            bindings.push(SourceBinding {
                source_id: physical.source_id,
                source_version_id,
                mapping_version_id,
                source_schema_version_id,
                source: physical.source,
            });
            member_sources.push(physical.source_id);
        }
        reports.push(LibrarySourceExecution {
            membership_id: member.membership_id,
            name: member.name,
            kind: member.kind,
            source_ids: member_sources,
            editable: member.editable,
        });
    }

    let policy_document = PolicyDocument {
        rules: source_versions
            .iter()
            .map(|source| PolicyRule {
                effect: Effect::Allow,
                subjects: vec![SubjectSelector::Service {
                    service_id: LIBRARY_SERVICE_ID,
                }],
                action: Action::Query,
                workspaces: ResourceScope::Only(BTreeSet::from([workspace_id])),
                sources: ResourceScope::Only(BTreeSet::from([source.source_version_id])),
                entities: ResourceScope::Only(BTreeSet::from(["document.Document".to_string()])),
                fields: ResourceScope::All,
            })
            .collect(),
    };
    let policy_spec =
        serde_json::to_value(policy_document).map_err(|_| LibraryQueryError::InvalidCatalog)?;
    let policy_checksum = catalog_content_checksum(&policy_spec);
    let policy_version_id = version_id("policy", workspace_id, &policy_checksum);
    let policy_version = PolicyVersion {
        policy_version_id,
        name: "mdreader-library-read".to_string(),
        spec: policy_spec,
        checksum: policy_checksum,
    };
    policy_version
        .verify()
        .map_err(|_| LibraryQueryError::InvalidCatalog)?;

    let revision_spec = serde_json::json!({
        "workspace_id": workspace_id,
        "schemas": schema_versions.iter().map(|version| version.schema_version_id).collect::<Vec<_>>(),
        "mappings": mapping_versions.iter().map(|version| version.mapping_version_id).collect::<Vec<_>>(),
        "sources": source_versions.iter().map(|version| version.source_version_id).collect::<Vec<_>>(),
        "policy": policy_version_id,
    });
    let revision_checksum = catalog_content_checksum(&revision_spec);
    let revision_id = version_id("catalog", workspace_id, &revision_checksum);
    let timestamp = i64::from_be_bytes(
        blake3::hash(revision_checksum.as_bytes()).as_bytes()[..8]
            .try_into()
            .map_err(|_| LibraryQueryError::InvalidCatalog)?,
    )
    .unsigned_abs()
    .min(i64::MAX as u64) as i64;
    let timestamp = timestamp.max(1);
    let revision = CatalogRevision {
        revision_id,
        parent_revision_id: None,
        actor: "mdreader-library".to_string(),
        timestamp,
        schema_version_ids: schema_versions
            .iter()
            .map(|version| version.schema_version_id)
            .collect(),
        mapping_version_ids: mapping_versions
            .iter()
            .map(|version| version.mapping_version_id)
            .collect(),
        source_version_ids: source_versions
            .iter()
            .map(|version| version.source_version_id)
            .collect(),
        policy_version_ids: vec![policy_version_id],
    };
    let catalog = RuntimeCatalog {
        revision,
        schema_contents,
        schema_versions,
        mapping_versions,
        source_versions,
        policy_versions: vec![policy_version],
        default_schema_version_id: semantic_schema_version_id,
    };
    catalog
        .validate_control_plane_integrity()
        .map_err(|_| LibraryQueryError::InvalidCatalog)?;
    Ok(LibraryRuntime {
        catalog,
        bindings,
        memberships: reports,
    })
}

/// Execute exactly one bounded query over one immutable Library snapshot.
pub(crate) async fn execute_registered_library_query(
    mut request: EntityQueryRequest,
    members: Vec<LibraryQueryMember>,
    cancellation: CancellationToken,
) -> Result<LibraryEntityQueryResponse, LibraryQueryError> {
    request
        .validate()
        .map_err(|_| LibraryQueryError::InvalidRequest)?;
    if request.entity != "document.Document"
        || request.cursor.is_some()
        || request.query_id.is_some()
        || request.page_size < request.limit
    {
        return Err(LibraryQueryError::InvalidRequest);
    }
    let runtime = build_runtime(request.workspace_id, members)?;
    let by_membership = runtime
        .memberships
        .iter()
        .map(|member| (member.membership_id, member.source_ids.clone()))
        .collect::<BTreeMap<_, _>>();
    let selected_memberships = request
        .source_ids
        .clone()
        .unwrap_or_else(|| by_membership.keys().copied().collect());
    if selected_memberships.is_empty() {
        return Err(LibraryQueryError::NoEnabledSources);
    }
    let mut selected_physical = Vec::new();
    for source_id in &selected_memberships {
        let Some(physical) = by_membership.get(source_id) else {
            return Err(LibraryQueryError::UnknownSource);
        };
        selected_physical.extend(physical.iter().copied());
    }
    if request
        .catalog_revision_id
        .is_some_and(|requested| requested != runtime.catalog.revision.revision_id)
        || request
            .schema_version_id
            .is_some_and(|requested| requested != runtime.catalog.default_schema_version_id)
    {
        return Err(LibraryQueryError::InvalidCatalog);
    }
    request.source_ids = Some(selected_physical);
    request.catalog_revision_id = Some(runtime.catalog.revision.revision_id);
    request.schema_version_id = Some(runtime.catalog.default_schema_version_id);

    // This operation returns one bounded, fully hydrated page and deliberately
    // rejects continuation. Use a per-execution secret so a public catalog
    // revision can never double as cursor authentication material.
    let mut cursor_seed = Uuid::new_v4().as_bytes().to_vec();
    cursor_seed.extend_from_slice(Uuid::new_v4().as_bytes());
    let cursor_key = *blake3::hash(&cursor_seed).as_bytes();
    let service = EntityQueryService::new(
        LIBRARY_DEPLOYMENT_ID,
        Arc::new(LibraryCatalogResolver {
            catalog: runtime.catalog,
        }),
        runtime.bindings,
        cursor_key,
        QUERY_CONCURRENCY,
    )
    .map_err(|_| LibraryQueryError::InvalidCatalog)?;
    let query_id = Uuid::new_v4();
    let context = ReadContext::new(
        query_id,
        Instant::now()
            .checked_add(QUERY_DEADLINE)
            .ok_or(LibraryQueryError::QueryFailed)?,
        QUERY_WORK_BUDGET,
        QUERY_BYTE_BUDGET,
        QUERY_PAGE_SIZE,
        cancellation,
    )
    .map_err(|_| LibraryQueryError::QueryFailed)?;
    let principal = Principal::service(
        LIBRARY_SERVICE_ID,
        LIBRARY_DEPLOYMENT_ID,
        [request.workspace_id],
    );
    let answer = service
        .query(&principal, request, &context)
        .await
        .map_err(map_entity_query_error)?;
    let selected = selected_memberships.into_iter().collect::<BTreeSet<_>>();
    Ok(LibraryEntityQueryResponse {
        answer,
        memberships: runtime
            .memberships
            .into_iter()
            .filter(|member| selected.contains(&member.membership_id))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use catdb_api_types::{Completeness, EntityQueryRequest};
    use catdb_connectors::{
        DataSource, FilesystemSource, FrontmatterSource, GitSource, ReadContext, SourceAttr,
        SourceCapabilities, SourceCursor, SourceError, SourceFreshness, SourceId, SourceModel,
        SourcePage, SourcePlan, SourceRow,
    };
    use catdb_core::BaseType;
    use serde_json::json;

    use super::*;

    #[derive(Debug)]
    struct CloudFixture {
        model: SourceModel,
        rows: Mutex<Vec<SourceRow>>,
        spec: serde_json::Value,
    }

    impl CloudFixture {
        fn drive() -> Self {
            Self::new(
                "drive-account",
                vec![
                    SourceAttr::text("id"),
                    SourceAttr::text("name"),
                    SourceAttr::text("mimeType"),
                    SourceAttr::parsed("modifiedAt", BaseType::Timestamp),
                    SourceAttr::parsed("size", BaseType::Int),
                    SourceAttr::text("checksum"),
                    SourceAttr::text("body"),
                ],
                "drive-note",
                json!({"id":"drive-note","name":"Drive note"}),
            )
        }

        fn dropbox() -> Self {
            Self::new(
                "dropbox-account",
                vec![
                    SourceAttr::text("id"),
                    SourceAttr::text("name"),
                    SourceAttr::text("path"),
                    SourceAttr::parsed("modifiedAt", BaseType::Timestamp),
                    SourceAttr::parsed("size", BaseType::Int),
                    SourceAttr::text("rev"),
                    SourceAttr::text("contentHash"),
                    SourceAttr::text("body"),
                ],
                "/dropbox-note.md",
                json!({"id":"dbx","name":"Dropbox note","path":"/dropbox-note.md"}),
            )
        }

        fn new(id: &str, attrs: Vec<SourceAttr>, key: &str, value: serde_json::Value) -> Self {
            let model = SourceModel::new(SourceId::new(id), "document", "Document", attrs);
            Self {
                rows: Mutex::new(vec![SourceRow {
                    source: SourceId::new(id),
                    entity: "document.Document".to_string(),
                    key: key.to_string(),
                    value,
                }]),
                spec: json!({"kind":"fixture-cloud","connector_version":"v1","id":id}),
                model,
            }
        }
    }

    #[async_trait]
    impl DataSource for CloudFixture {
        fn id(&self) -> &SourceId {
            self.model.id()
        }

        fn schema(&self) -> &catdb_core::Schema {
            self.model.schema()
        }

        fn capabilities(&self) -> &SourceCapabilities {
            self.model.capabilities()
        }

        fn source_spec(&self) -> serde_json::Value {
            self.spec.clone()
        }

        async fn freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
            context.check()?;
            Ok(SourceFreshness {
                observed_at_ms: 1,
                freshness_ms: 0,
            })
        }

        fn compile(
            &self,
            query: &catdb_query::EntityQuery,
            authorized_fields: &[String],
        ) -> Result<SourcePlan, SourceError> {
            self.model.compile(query, authorized_fields)
        }

        async fn execute_page(
            &self,
            plan: &SourcePlan,
            cursor: Option<&SourceCursor>,
            context: &ReadContext,
        ) -> Result<SourcePage, SourceError> {
            context.check()?;
            self.model.admit(plan)?;
            if cursor.is_some() {
                return Err(SourceError::InvalidCursor);
            }
            let mut rows = self.rows.lock().expect("fixture rows").clone();
            for row in &mut rows {
                row.value
                    .as_object_mut()
                    .expect("object row")
                    .retain(|field, _| plan.projected.contains(field));
            }
            Ok(SourcePage {
                source: self.model.id().clone(),
                scanned: rows.len(),
                rows,
                next_cursor: None,
                observed_at_ms: 1,
                freshness_ms: 0,
            })
        }
    }

    fn local_member(root: &Path, membership_id: Uuid) -> LibraryQueryMember {
        let fs: Arc<dyn DataSource> = Arc::new(FilesystemSource::new(root));
        let git: Arc<dyn DataSource> = Arc::new(GitSource::open(root));
        let front: Arc<dyn DataSource> = Arc::new(FrontmatterSource::new(root));
        let sources = [fs, git, front]
            .into_iter()
            .map(|source| {
                let mapping =
                    mdreader_domain::federated::source_mapping(source.as_ref()).expect("mapping");
                LibraryPhysicalSource::new(membership_id, source, mapping)
            })
            .collect();
        LibraryQueryMember {
            membership_id,
            name: root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Local folder")
                .to_string(),
            kind: LibraryQueryMemberKind::LocalFolder,
            editable: true,
            sources,
        }
    }

    fn cloud_member(
        membership_id: Uuid,
        source: Arc<dyn DataSource>,
        provider: mdreader_domain::library::CloudDocumentSource,
    ) -> LibraryQueryMember {
        let mapping = mdreader_domain::library::cloud_document_mapping(source.as_ref(), provider)
            .expect("cloud mapping");
        LibraryQueryMember {
            membership_id,
            name: match provider {
                mdreader_domain::library::CloudDocumentSource::GoogleDrive => {
                    "Google Drive".to_string()
                }
                mdreader_domain::library::CloudDocumentSource::Dropbox => "Dropbox".to_string(),
            },
            kind: match provider {
                mdreader_domain::library::CloudDocumentSource::GoogleDrive => {
                    LibraryQueryMemberKind::GoogleDrive
                }
                mdreader_domain::library::CloudDocumentSource::Dropbox => {
                    LibraryQueryMemberKind::Dropbox
                }
            },
            editable: false,
            sources: vec![LibraryPhysicalSource::new(membership_id, source, mapping)],
        }
    }

    fn request(workspace_id: Uuid, memberships: Vec<Uuid>) -> EntityQueryRequest {
        EntityQueryRequest {
            workspace_id,
            catalog_revision_id: None,
            schema_version_id: None,
            entity: "document.Document".to_string(),
            select: vec!["path".to_string()],
            filters: Vec::new(),
            order_by: Vec::new(),
            source_ids: Some(memberships),
            max_staleness_ms: None,
            require_complete: Some(true),
            page_size: 100,
            limit: 100,
            query_id: None,
            cursor: None,
        }
    }

    #[tokio::test]
    async fn registered_local_and_cloud_members_execute_as_one_entity_query() {
        let tree = tempfile::tempdir().expect("temp tree");
        std::fs::write(tree.path().join("note.md"), "# Local\n").expect("fixture");
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tree.path())
            .output()
            .expect("git");
        assert!(output.status.success());

        let workspace = Uuid::new_v4();
        let local = Uuid::new_v4();
        let drive = Uuid::new_v4();
        let dropbox = Uuid::new_v4();
        let members = vec![
            local_member(tree.path(), local),
            cloud_member(
                drive,
                Arc::new(CloudFixture::drive()),
                mdreader_domain::library::CloudDocumentSource::GoogleDrive,
            ),
            cloud_member(
                dropbox,
                Arc::new(CloudFixture::dropbox()),
                mdreader_domain::library::CloudDocumentSource::Dropbox,
            ),
        ];
        let answer = execute_registered_library_query(
            request(workspace, vec![local, drive, dropbox]),
            members,
            CancellationToken::new(),
        )
        .await
        .expect("one Library query");

        assert_eq!(answer.answer.sources.len(), 5);
        assert_eq!(answer.answer.pins.source_version_ids.len(), 5);
        assert_eq!(answer.answer.pins.mapping_version_ids.len(), 5);
        assert_eq!(answer.answer.pins.physical_schema_version_ids.len(), 5);
        assert_eq!(answer.memberships.len(), 3);
        assert_eq!(
            answer
                .memberships
                .iter()
                .map(|member| (member.membership_id, member.editable))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from([(local, true), (drive, false), (dropbox, false)])
        );
        assert_eq!(answer.answer.completeness, Completeness::Complete);
        assert!(answer
            .answer
            .rows
            .iter()
            .any(|row| row.record_key == "note.md"));
        assert!(answer
            .answer
            .rows
            .iter()
            .any(|row| row.record_key == "drive-note"));
        assert!(answer
            .answer
            .rows
            .iter()
            .any(|row| row.record_key == "/dropbox-note.md"));
    }

    #[tokio::test]
    async fn source_selection_is_registry_membership_not_connector_choice() {
        let workspace = Uuid::new_v4();
        let registered = Uuid::new_v4();
        let unknown = Uuid::new_v4();
        let result = execute_registered_library_query(
            request(workspace, vec![unknown]),
            vec![cloud_member(
                registered,
                Arc::new(CloudFixture::drive()),
                mdreader_domain::library::CloudDocumentSource::GoogleDrive,
            )],
            CancellationToken::new(),
        )
        .await;
        assert_eq!(result, Err(LibraryQueryError::UnknownSource));
    }

    #[tokio::test]
    async fn service_cancellation_remains_a_typed_library_outcome() {
        let workspace = Uuid::new_v4();
        let membership = Uuid::new_v4();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = execute_registered_library_query(
            request(workspace, vec![membership]),
            vec![cloud_member(
                membership,
                Arc::new(CloudFixture::drive()),
                mdreader_domain::library::CloudDocumentSource::GoogleDrive,
            )],
            cancellation,
        )
        .await;
        assert_eq!(result, Err(LibraryQueryError::Cancelled));
    }
}
