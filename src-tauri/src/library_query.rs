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
use catdb_api_types::{EntityQueryRequest, EntityQueryResponse, SourceExecution, MAX_LIMIT};
use catdb_connectors::{
    CancellationToken, DataSource, ReadContext, SourceCapabilities, SourceCursor, SourceError,
    SourceFreshness, SourceModel, SourcePage, SourcePlan,
};
use catdb_core::Mapping;
use catdb_federation::{
    CatalogResolveError, CatalogResolver, ContinuationLimits, EntityQueryError, EntityQueryService,
    ResolvedCatalog, RuntimeCatalog, SourceBinding,
};
use catdb_policy::{
    Action, Effect, PolicyDocument, PolicyRule, Principal, ResourceScope, SubjectSelector,
};
use catdb_provenance::FieldProvenance;
use catdb_storage::{
    catalog_content_checksum, CatalogRevision, Compatibility, MappingCapability, MappingVersion,
    PolicyVersion, ProvenanceRuleVersion, SchemaContent, SchemaVersion, SourceVersion,
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
const SCOPED_KEY_PREFIX: &str = "library:";
const LIBRARY_IDENTITY_FIELD: &str = "path";

/// Stable query identity for one persisted Library membership record.
pub(crate) fn membership_id(registry_id: &str) -> Uuid {
    Uuid::new_v5(&PHYSICAL_SOURCE_NAMESPACE, registry_id.as_bytes())
}

fn scope_record_key(membership_id: Uuid, physical_key: &str) -> String {
    format!("{SCOPED_KEY_PREFIX}{membership_id}:{physical_key}")
}

fn split_scoped_record_key(value: &str) -> Option<(Uuid, &str)> {
    let rest = value.strip_prefix(SCOPED_KEY_PREFIX)?;
    let (membership, physical) = rest.split_once(':')?;
    let membership = Uuid::parse_str(membership).ok()?;
    (!physical.is_empty()).then_some((membership, physical))
}

struct MembershipScopedSource {
    membership_id: Uuid,
    source: Arc<dyn DataSource>,
}

struct DeclaredUnavailableSource {
    model: SourceModel,
    capabilities: SourceCapabilities,
    source_spec: serde_json::Value,
    reason: String,
}

pub(crate) fn declared_unavailable_source(
    model: SourceModel,
    source_spec: serde_json::Value,
    reason: impl Into<String>,
) -> Arc<dyn DataSource> {
    let reason = reason.into();
    Arc::new(DeclaredUnavailableSource {
        model,
        capabilities: SourceCapabilities::unavailable(reason.clone()),
        source_spec,
        reason,
    })
}

#[async_trait]
impl DataSource for DeclaredUnavailableSource {
    fn id(&self) -> &catdb_connectors::SourceId {
        self.model.id()
    }

    fn schema(&self) -> &catdb_core::Schema {
        self.model.schema()
    }

    fn capabilities(&self) -> &SourceCapabilities {
        &self.capabilities
    }

    fn source_spec(&self) -> serde_json::Value {
        self.source_spec.clone()
    }

    fn compile(
        &self,
        _query: &catdb_query::EntityQuery,
        _authorized_fields: &[String],
    ) -> Result<SourcePlan, SourceError> {
        Err(SourceError::Unavailable {
            source_id: self.model.id().clone(),
            reason: self.reason.clone(),
        })
    }

    async fn execute_page(
        &self,
        _plan: &SourcePlan,
        _cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        context.check()?;
        Err(SourceError::Unavailable {
            source_id: self.model.id().clone(),
            reason: self.reason.clone(),
        })
    }
}

#[async_trait]
impl DataSource for MembershipScopedSource {
    fn id(&self) -> &catdb_connectors::SourceId {
        self.source.id()
    }

    fn schema(&self) -> &catdb_core::Schema {
        self.source.schema()
    }

    fn capabilities(&self) -> &catdb_connectors::SourceCapabilities {
        self.source.capabilities()
    }

    fn source_spec(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": "mdreader-library-membership",
            "membership_id": self.membership_id,
            "source": self.source.source_spec(),
        })
    }

    async fn freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
        self.source.freshness(context).await
    }

    fn compile(
        &self,
        query: &catdb_query::EntityQuery,
        authorized_fields: &[String],
    ) -> Result<SourcePlan, SourceError> {
        self.source.compile(query, authorized_fields)
    }

    async fn execute_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        let mut page = self.source.execute_page(plan, cursor, context).await?;
        for row in &mut page.rows {
            row.key = scope_record_key(self.membership_id, &row.key);
        }
        Ok(page)
    }
}

/// One registered Library membership and the physical sources it authorizes.
#[derive(Clone)]
pub(crate) struct LibraryQueryMember {
    /// Stable identity presented in `EntityQueryRequest.source_ids`.
    pub membership_id: Uuid,
    /// Opaque persisted registry identity used for explicit local activation.
    pub registry_id: String,
    /// User-facing source label captured with membership.
    pub name: String,
    /// Source family, used only for presentation and open-original behavior.
    pub kind: LibraryQueryMemberKind,
    /// Whether records from this member can be edited by MDReader.
    pub editable: bool,
    /// Whether this local membership owns the current editor grant.
    pub active: bool,
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
    ConnectedCatdb,
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
    /// Stable source family used by the declared semantic precedence.
    precedence_source_id: catdb_connectors::SourceId,
}

impl LibraryPhysicalSource {
    /// Derive a stable physical identity from a membership and connector.
    pub(crate) fn new(membership_id: Uuid, source: Arc<dyn DataSource>, mapping: Mapping) -> Self {
        let precedence_source_id = source.id().clone();
        Self::new_as(membership_id, source, mapping, precedence_source_id)
    }

    /// Bind an account-specific source identity to its declared provider family.
    pub(crate) fn new_as(
        membership_id: Uuid,
        source: Arc<dyn DataSource>,
        mapping: Mapping,
        precedence_source_id: catdb_connectors::SourceId,
    ) -> Self {
        let mut identity = membership_id.as_bytes().to_vec();
        identity.extend_from_slice(source.id().as_str().as_bytes());
        Self {
            source_id: Uuid::new_v5(&PHYSICAL_SOURCE_NAMESPACE, &identity),
            source: Arc::new(MembershipScopedSource {
                membership_id,
                source,
            }),
            mapping,
            precedence_source_id,
        }
    }
}

/// How a returned physical source relates to the user-selected Library member.
#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct LibrarySourceExecution {
    pub membership_id: Uuid,
    pub registry_id: String,
    pub name: String,
    pub kind: LibraryQueryMemberKind,
    pub source_ids: Vec<Uuid>,
    pub editable: bool,
    pub active: bool,
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
        EntityQueryError::InvalidProvenance | EntityQueryError::InvalidSemanticResult => {
            LibraryQueryError::InvalidResponse
        }
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
    let mut source_policy_fields = Vec::new();
    let mut provider_versions = BTreeMap::<(String, String, String), Vec<(Uuid, Uuid)>>::new();
    let mut mapped_field_providers = BTreeMap::<String, BTreeSet<(Uuid, Uuid)>>::new();
    let mut source_mapping_pairs = BTreeSet::new();
    let mut bindings = Vec::new();
    let mut reports = Vec::new();

    for member in members {
        let mut member_sources = Vec::new();
        for physical in member.sources {
            let mapped_fields = physical
                .mapping
                .attr_map
                .values()
                .filter(|target| {
                    target.path.arrows.is_empty()
                        && target.path.start.as_str() == mdreader_domain::federated::ENTITY
                })
                .map(|target| target.attr.as_str().to_string())
                .collect::<BTreeSet<_>>();
            if mapped_fields.is_empty() {
                return Err(LibraryQueryError::InvalidCatalog);
            }
            let schema_content = SchemaContent::new(
                serde_json::to_value(physical.source.schema())
                    .map_err(|_| LibraryQueryError::InvalidCatalog)?,
            );
            let source_schema_version_id =
                version_id("physical-schema", Uuid::nil(), &schema_content.checksum);
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
            for (source_attr, target) in &physical.mapping.attr_map {
                if target.path.arrows.is_empty()
                    && target.path.start.as_str() == mdreader_domain::federated::ENTITY
                {
                    let provider = (source_version_id, mapping_version_id);
                    provider_versions
                        .entry((
                            physical.precedence_source_id.as_str().to_string(),
                            source_attr.as_str().to_string(),
                            target.attr.as_str().to_string(),
                        ))
                        .or_default()
                        .push(provider);
                    mapped_field_providers
                        .entry(target.attr.as_str().to_string())
                        .or_default()
                        .insert(provider);
                    source_mapping_pairs.insert(provider);
                }
            }
            mapping_version
                .verify()
                .and_then(|()| source_version.verify())
                .map_err(|_| LibraryQueryError::InvalidCatalog)?;

            if !schema_versions
                .iter()
                .any(|version| version.schema_version_id == source_schema_version_id)
            {
                schema_contents.push(schema_content);
                schema_versions.push(source_schema_version);
            }
            mapping_versions.push(mapping_version);
            source_versions.push(source_version);
            source_policy_fields.push((source_version_id, mapped_fields));
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
            registry_id: member.registry_id,
            name: member.name,
            kind: member.kind,
            source_ids: member_sources,
            editable: member.editable,
            active: member.active,
        });
    }

    let mut rule_fields = Vec::new();
    let mut declared_provider_pairs = BTreeSet::new();
    for declaration in mdreader_domain::federated::precedence().fields() {
        let mut providers = Vec::new();
        let mut provider_pairs = Vec::new();
        for provider in &declaration.order {
            if let Some(versions) = provider_versions.get(&(
                provider.source.as_str().to_string(),
                provider.attr.as_str().to_string(),
                declaration.field.as_str().to_string(),
            )) {
                for (source_version_id, mapping_version_id) in versions {
                    provider_pairs.push((*source_version_id, *mapping_version_id));
                    providers.push(serde_json::json!({
                        "source_version_id": source_version_id,
                        "mapping_version_id": mapping_version_id,
                    }));
                }
            }
        }
        if providers.is_empty() {
            continue;
        }
        if provider_pairs.iter().copied().collect::<BTreeSet<_>>()
            != mapped_field_providers
                .get(declaration.field.as_str())
                .cloned()
                .unwrap_or_default()
        {
            return Err(LibraryQueryError::InvalidCatalog);
        }
        declared_provider_pairs.extend(provider_pairs);
        rule_fields.push(serde_json::json!({
            "entity": "document.Document",
            "field": declaration.field.as_str(),
            "rationale": declaration.rationale,
            "providers": providers,
        }));
    }
    if declared_provider_pairs != source_mapping_pairs {
        return Err(LibraryQueryError::InvalidCatalog);
    }
    let provenance_rule_spec = serde_json::json!({ "fields": rule_fields });
    let provenance_rule_checksum = catalog_content_checksum(&provenance_rule_spec);
    let provenance_rule_version_id =
        version_id("provenance-rule", workspace_id, &provenance_rule_checksum);
    let provenance_rule_version = ProvenanceRuleVersion {
        provenance_rule_version_id,
        spec: provenance_rule_spec,
        checksum: provenance_rule_checksum,
    };
    provenance_rule_version
        .verify()
        .map_err(|_| LibraryQueryError::InvalidCatalog)?;

    let policy_document = PolicyDocument {
        rules: source_policy_fields
            .into_iter()
            .map(|(source_version_id, fields)| PolicyRule {
                effect: Effect::Allow,
                subjects: vec![SubjectSelector::Service {
                    service_id: LIBRARY_SERVICE_ID,
                }],
                action: Action::Query,
                workspaces: ResourceScope::Only(BTreeSet::from([workspace_id])),
                sources: ResourceScope::Only(BTreeSet::from([source_version_id])),
                entities: ResourceScope::Only(BTreeSet::from(["document.Document".to_string()])),
                fields: ResourceScope::Only(fields),
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
        "provenance_rule": provenance_rule_version_id,
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
        provenance_rule_version_ids: vec![provenance_rule_version_id],
    };
    let catalog = RuntimeCatalog {
        revision,
        schema_contents,
        schema_versions,
        mapping_versions,
        source_versions,
        policy_versions: vec![policy_version],
        provenance_rule_versions: vec![provenance_rule_version],
        default_schema_version_id: semantic_schema_version_id,
        deferred_integrity: Default::default(),
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

fn restore_membership_provenance(
    answer: &mut EntityQueryResponse,
    memberships: &[LibrarySourceExecution],
) -> Result<(), LibraryQueryError> {
    let source_memberships = memberships
        .iter()
        .flat_map(|membership| {
            membership
                .source_ids
                .iter()
                .map(move |source_id| (*source_id, membership.membership_id))
        })
        .collect::<BTreeMap<_, _>>();
    let version_memberships = answer
        .sources
        .iter()
        .map(|source| {
            source_memberships
                .get(&source.source_id)
                .copied()
                .map(|membership| (source.source_version_id, membership))
                .ok_or(LibraryQueryError::InvalidResponse)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    for row in &mut answer.rows {
        let (row_membership, _) =
            split_scoped_record_key(&row.record_key).ok_or(LibraryQueryError::InvalidResponse)?;
        let original = std::mem::take(&mut row.provenance);
        let mut restored = BTreeMap::new();
        for (field, evidence) in original {
            let selected = evidence.selected_index();
            let mut selected_restored = None;
            let mut considered = Vec::new();
            for (index, observation) in evidence.considered().iter().enumerate() {
                if version_memberships.get(&observation.source_version_id())
                    != Some(&row_membership)
                {
                    continue;
                }
                let (observation_membership, physical_key) =
                    split_scoped_record_key(observation.record_key())
                        .ok_or(LibraryQueryError::InvalidResponse)?;
                if observation_membership != row_membership {
                    return Err(LibraryQueryError::InvalidResponse);
                }
                if selected == Some(index) {
                    selected_restored = Some(considered.len());
                }
                considered.push(observation.clone().with_record_key(physical_key));
            }
            restored.insert(
                field.clone(),
                FieldProvenance::new(
                    evidence.provenance_rule_version_id(),
                    field,
                    considered,
                    selected_restored,
                )
                .map_err(|_| LibraryQueryError::InvalidResponse)?,
            );
        }
        row.provenance = restored;
    }
    Ok(())
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
    let pinned_request = request.clone();

    // Continuations are consumed inside this command while the immutable
    // runtime and per-execution secret remain alive. No opaque cursor crosses
    // into renderer authority or outlives its exact catalog snapshot.
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
    .map_err(|_| LibraryQueryError::InvalidCatalog)?
    .with_continuation_limits(ContinuationLimits {
        max_entries: 1,
        max_rows_per_entry: usize::try_from(MAX_LIMIT)
            .map_err(|_| LibraryQueryError::LimitExceeded)?,
        max_bytes_per_entry: QUERY_BYTE_BUDGET,
        ttl: QUERY_DEADLINE,
    })
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
    let mut answer = service
        .query_with_correlation_fields(
            &principal,
            request,
            &context,
            &[LIBRARY_IDENTITY_FIELD.to_string()],
        )
        .await
        .map_err(map_entity_query_error)?;
    restore_membership_provenance(&mut answer, &runtime.memberships)?;
    while let Some(cursor) = answer.next_cursor.take() {
        let mut continuation = pinned_request.clone();
        continuation.query_id = Some(answer.query_id);
        continuation.cursor = Some(cursor);
        let mut page = service
            .query_with_correlation_fields(
                &principal,
                continuation,
                &context,
                &[LIBRARY_IDENTITY_FIELD.to_string()],
            )
            .await
            .map_err(map_entity_query_error)?;
        restore_membership_provenance(&mut page, &runtime.memberships)?;
        if page.query_id != answer.query_id
            || page.pins != answer.pins
            || page.sources != answer.sources
            || page.completeness != answer.completeness
        {
            return Err(LibraryQueryError::InvalidResponse);
        }
        answer.rows.extend(page.rows);
        answer.next_cursor = page.next_cursor;
    }
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
    use catdb_api_types::{Completeness, EntityFilter, EntityQueryRequest, FilterOperator};
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

        fn drive_documents(count: usize) -> Self {
            let fixture = Self::drive();
            *fixture.rows.lock().expect("fixture rows") = (0..count)
                .map(|index| {
                    let key = format!("drive-note-{index:04}");
                    SourceRow {
                        source: SourceId::new("drive-account"),
                        entity: "document.Document".to_string(),
                        key: key.clone(),
                        value: json!({"id":key,"name":format!("Drive note {index:04}")}),
                    }
                })
                .collect();
            fixture
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
            let offset = cursor.map_or(Ok(0_usize), |cursor| {
                let bytes: [u8; 8] = cursor
                    .as_bytes()
                    .try_into()
                    .map_err(|_| SourceError::InvalidCursor)?;
                usize::try_from(u64::from_be_bytes(bytes)).map_err(|_| SourceError::InvalidCursor)
            })?;
            let all_rows = self.rows.lock().expect("fixture rows").clone();
            if offset > all_rows.len() {
                return Err(SourceError::InvalidCursor);
            }
            let end = offset
                .saturating_add(context.page_size())
                .min(all_rows.len());
            let mut rows = all_rows[offset..end].to_vec();
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
                next_cursor: (end < all_rows.len())
                    .then(|| {
                        u64::try_from(end)
                            .map_err(|_| SourceError::InvalidCursor)
                            .and_then(|offset| SourceCursor::new(offset.to_be_bytes().to_vec()))
                    })
                    .transpose()?,
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
            registry_id: membership_id.to_string(),
            name: root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Local folder")
                .to_string(),
            kind: LibraryQueryMemberKind::LocalFolder,
            editable: true,
            active: true,
            sources,
        }
    }

    fn unavailable_local_member(root: &Path, membership_id: Uuid) -> LibraryQueryMember {
        let source: Arc<dyn DataSource> = Arc::new(FilesystemSource::unavailable(
            root,
            "registered folder is unavailable",
        ));
        let mapping = mdreader_domain::federated::source_mapping(source.as_ref()).expect("mapping");
        LibraryQueryMember {
            membership_id,
            registry_id: membership_id.to_string(),
            name: "Unavailable folder".to_string(),
            kind: LibraryQueryMemberKind::LocalFolder,
            editable: true,
            active: false,
            sources: vec![LibraryPhysicalSource::new(membership_id, source, mapping)],
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
            registry_id: membership_id.to_string(),
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
            active: false,
            sources: vec![LibraryPhysicalSource::new_as(
                membership_id,
                source,
                mapping,
                SourceId::new(match provider {
                    mdreader_domain::library::CloudDocumentSource::GoogleDrive => "google-drive",
                    mdreader_domain::library::CloudDocumentSource::Dropbox => "dropbox",
                }),
            )],
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
            .any(|row| row.record_key == scope_record_key(local, "note.md")));
        assert!(answer
            .answer
            .rows
            .iter()
            .any(|row| row.record_key == scope_record_key(drive, "drive-note")));
        assert!(answer
            .answer
            .rows
            .iter()
            .any(|row| row.record_key == scope_record_key(dropbox, "/dropbox-note.md")));
    }

    #[tokio::test]
    async fn library_query_hydrates_every_continuation_page_beyond_one_thousand_rows() {
        let workspace = Uuid::new_v4();
        let drive = Uuid::new_v4();
        let mut query = request(workspace, vec![drive]);
        query.page_size = 1_000;
        query.limit = 1_001;
        let answer = execute_registered_library_query(
            query,
            vec![cloud_member(
                drive,
                Arc::new(CloudFixture::drive_documents(1_001)),
                mdreader_domain::library::CloudDocumentSource::GoogleDrive,
            )],
            CancellationToken::new(),
        )
        .await
        .expect("complete paged Library query");

        assert_eq!(answer.answer.rows.len(), 1_001);
        assert_eq!(answer.answer.next_cursor, None);
        assert_eq!(answer.answer.completeness, Completeness::Complete);
    }

    #[tokio::test]
    async fn frontmatter_author_precedes_the_git_commit_author() {
        let tree = tempfile::tempdir().expect("temp tree");
        std::fs::write(
            tree.path().join("note.md"),
            "---\nauthor: Document Author\n---\n# Note\n",
        )
        .expect("fixture");
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "Git Committer"],
            &["config", "user.email", "committer@example.com"],
            &["add", "note.md"],
            &["commit", "-qm", "fixture"],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(tree.path())
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let workspace = Uuid::new_v4();
        let membership = Uuid::new_v4();
        let member = local_member(tree.path(), membership);
        let frontmatter_source_id = member
            .sources
            .iter()
            .find(|source| source.source.id().as_str() == FrontmatterSource::ID)
            .expect("frontmatter source")
            .source_id;
        let mut query = request(workspace, vec![membership]);
        query.select = vec!["author".to_string()];

        let answer =
            execute_registered_library_query(query, vec![member], CancellationToken::new())
                .await
                .expect("local Library query");
        let frontmatter_version_id = answer
            .answer
            .sources
            .iter()
            .find(|source| source.source_id == frontmatter_source_id)
            .expect("frontmatter execution")
            .source_version_id;
        let row = answer.answer.rows.first().expect("document row");

        assert_eq!(row.fields["author"], serde_json::json!("Document Author"));
        assert_eq!(
            row.provenance["author"]
                .selected()
                .expect("selected author")
                .source_version_id(),
            frontmatter_version_id
        );

        let mut lower_priority_filter = request(workspace, vec![membership]);
        lower_priority_filter.select = vec!["path".to_string(), "author".to_string()];
        lower_priority_filter.filters = vec![EntityFilter {
            field: "author".to_string(),
            operator: FilterOperator::Eq,
            value: "Git Committer".to_string(),
        }];
        let rejected = execute_registered_library_query(
            lower_priority_filter,
            vec![local_member(tree.path(), membership)],
            CancellationToken::new(),
        )
        .await
        .expect("lower-priority filter query");
        assert!(
            rejected.answer.rows.is_empty(),
            "a lower-priority provider must not make a semantic filter match"
        );
        assert_eq!(rejected.answer.completeness, Completeness::Complete);

        let mut winning_filter = request(workspace, vec![membership]);
        winning_filter.select = vec!["path".to_string(), "author".to_string()];
        winning_filter.filters = vec![EntityFilter {
            field: "author".to_string(),
            operator: FilterOperator::Eq,
            value: "Document Author".to_string(),
        }];
        let accepted = execute_registered_library_query(
            winning_filter,
            vec![local_member(tree.path(), membership)],
            CancellationToken::new(),
        )
        .await
        .expect("winning filter query");
        assert_eq!(accepted.answer.rows.len(), 1);
        assert_eq!(
            accepted.answer.rows[0].fields["author"],
            serde_json::json!("Document Author")
        );
    }

    #[tokio::test]
    async fn partial_providers_union_over_the_shipping_document_projection() {
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
        let mut query = request(workspace, vec![local, drive, dropbox]);
        query.select = [
            "providerId",
            "title",
            "author",
            "path",
            "mediaType",
            "modifiedAt",
            "size",
            "revision",
            "checksum",
            "body",
        ]
        .map(str::to_string)
        .to_vec();

        let answer = execute_registered_library_query(
            query,
            vec![
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
            ],
            CancellationToken::new(),
        )
        .await
        .expect("partial providers jointly cover the shipping projection");

        assert_eq!(answer.answer.completeness, Completeness::Complete);
        assert_eq!(answer.answer.sources.len(), 5);
        assert_eq!(answer.answer.rows.len(), 3);
        for row in &answer.answer.rows {
            let (membership, _) =
                split_scoped_record_key(&row.record_key).expect("membership-scoped row");
            assert_eq!(row.provenance.len(), 10);
            let provider_counts = row
                .provenance
                .iter()
                .map(|(field, evidence)| (field.as_str(), evidence.considered().len()))
                .collect::<BTreeMap<_, _>>();
            let expected = if membership == local {
                BTreeMap::from([
                    ("author", 2),
                    ("body", 1),
                    ("checksum", 0),
                    ("mediaType", 0),
                    ("modifiedAt", 2),
                    ("path", 3),
                    ("providerId", 0),
                    ("revision", 0),
                    ("size", 1),
                    ("title", 1),
                ])
            } else if membership == drive {
                BTreeMap::from([
                    ("author", 0),
                    ("body", 1),
                    ("checksum", 1),
                    ("mediaType", 1),
                    ("modifiedAt", 1),
                    ("path", 1),
                    ("providerId", 0),
                    ("revision", 0),
                    ("size", 1),
                    ("title", 1),
                ])
            } else {
                BTreeMap::from([
                    ("author", 0),
                    ("body", 1),
                    ("checksum", 1),
                    ("mediaType", 0),
                    ("modifiedAt", 1),
                    ("path", 1),
                    ("providerId", 1),
                    ("revision", 1),
                    ("size", 1),
                    ("title", 1),
                ])
            };
            assert_eq!(provider_counts, expected);
        }
    }

    #[tokio::test]
    async fn body_only_projection_keeps_every_selected_local_provider_in_evidence() {
        let tree = tempfile::tempdir().expect("temp tree");
        std::fs::write(tree.path().join("note.md"), "# Local body\n").expect("fixture");
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tree.path())
            .output()
            .expect("git");
        assert!(output.status.success());

        let workspace = Uuid::new_v4();
        let local = Uuid::new_v4();
        let mut query = request(workspace, vec![local]);
        query.select = vec!["body".to_string()];
        let answer = execute_registered_library_query(
            query,
            vec![local_member(tree.path(), local)],
            CancellationToken::new(),
        )
        .await
        .expect("body-only Library query");

        assert_eq!(answer.answer.completeness, Completeness::Complete);
        assert_eq!(answer.answer.sources.len(), 3);
        assert!(answer
            .answer
            .sources
            .iter()
            .all(|source| source.outcome == catdb_api_types::SourceOutcome::Ran));
        let row = answer.answer.rows.first().expect("local row");
        assert_eq!(row.fields.len(), 1);
        assert_eq!(row.fields["body"], serde_json::json!("# Local body\n"));
        assert_eq!(row.provenance["body"].considered().len(), 1);
        assert_eq!(
            row.provenance["body"]
                .considered()
                .iter()
                .filter(|observation| {
                    observation.state().kind() == catdb_provenance::ObservationKind::NotAsked
                })
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn hidden_provider_correlation_does_not_add_path_only_rows_before_a_tight_limit() {
        let tree = tempfile::tempdir().expect("temp tree");
        std::fs::write(tree.path().join("00-path-only.txt"), "not a document")
            .expect("path-only fixture");
        std::fs::write(tree.path().join("10-document.md"), "# Selected document\n")
            .expect("document fixture");
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tree.path())
            .output()
            .expect("git");
        assert!(output.status.success());

        let workspace = Uuid::new_v4();
        let local = Uuid::new_v4();
        let mut query = request(workspace, vec![local]);
        query.select = vec!["body".to_string()];
        query.page_size = 1;
        query.limit = 1;
        let answer = execute_registered_library_query(
            query,
            vec![local_member(tree.path(), local)],
            CancellationToken::new(),
        )
        .await
        .expect("body-only Library query");

        assert_eq!(answer.answer.completeness, Completeness::Complete);
        assert_eq!(answer.answer.sources.len(), 3);
        assert_eq!(answer.answer.rows.len(), 1);
        assert_eq!(
            answer.answer.rows[0].record_key,
            scope_record_key(local, "10-document.md")
        );
        assert_eq!(
            answer.answer.rows[0].fields,
            BTreeMap::from([(
                "body".to_string(),
                serde_json::json!("# Selected document\n")
            )])
        );
        assert_eq!(
            answer.answer.rows[0].provenance["body"].considered().len(),
            1
        );
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

    #[tokio::test]
    async fn equal_relative_paths_in_different_memberships_remain_distinct_documents() {
        let first_tree = tempfile::tempdir().expect("first tree");
        let second_tree = tempfile::tempdir().expect("second tree");
        std::fs::write(first_tree.path().join("note.md"), "# First membership\n")
            .expect("first fixture");
        std::fs::write(second_tree.path().join("note.md"), "# Second membership\n")
            .expect("second fixture");
        for tree in [&first_tree, &second_tree] {
            let output = std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(tree.path())
                .output()
                .expect("git");
            assert!(output.status.success());
        }

        let workspace = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut query = request(workspace, vec![first, second]);
        query.select = vec!["path".to_string(), "title".to_string(), "body".to_string()];
        query.require_complete = Some(false);
        let answer = execute_registered_library_query(
            query,
            vec![
                local_member(first_tree.path(), first),
                local_member(second_tree.path(), second),
            ],
            CancellationToken::new(),
        )
        .await
        .expect("two Library memberships");

        assert_eq!(answer.answer.rows.len(), 2);
        assert_ne!(
            answer.answer.rows[0].record_key,
            answer.answer.rows[1].record_key
        );
        let titles = answer
            .answer
            .rows
            .iter()
            .filter_map(|row| row.fields.get("title").and_then(serde_json::Value::as_str))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            titles,
            BTreeSet::from(["First membership", "Second membership"])
        );
        for row in &answer.answer.rows {
            for (field_name, field) in &row.provenance {
                assert!(field
                    .considered()
                    .iter()
                    .all(|observation| observation.record_key() == "note.md"));
                assert_eq!(
                    field.considered().len(),
                    match field_name.as_str() {
                        "path" => 3,
                        "title" | "body" => 1,
                        other => panic!("unexpected field {other}"),
                    }
                );
            }
        }
    }

    #[tokio::test]
    async fn unavailable_membership_is_typed_partial_without_discarding_healthy_rows() {
        let healthy_tree = tempfile::tempdir().expect("healthy tree");
        std::fs::write(healthy_tree.path().join("note.md"), "# Healthy\n")
            .expect("healthy document");
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(healthy_tree.path())
            .output()
            .expect("git");
        assert!(output.status.success());

        let workspace = Uuid::new_v4();
        let healthy = Uuid::new_v4();
        let unavailable = Uuid::new_v4();
        let unavailable_member =
            unavailable_local_member(&healthy_tree.path().join("missing"), unavailable);
        let unavailable_source_id = unavailable_member.sources[0].source_id;
        let mut query = request(workspace, vec![healthy, unavailable]);
        query.require_complete = Some(false);

        let answer = execute_registered_library_query(
            query,
            vec![
                local_member(healthy_tree.path(), healthy),
                unavailable_member,
            ],
            CancellationToken::new(),
        )
        .await
        .expect("healthy source must survive an unavailable peer");

        assert_eq!(answer.answer.rows.len(), 1);
        assert_eq!(
            answer.answer.completeness,
            Completeness::Partial {
                excluded_source_ids: vec![unavailable_source_id],
            }
        );
        let unavailable_report = answer
            .answer
            .sources
            .iter()
            .find(|source| source.source_id == unavailable_source_id)
            .expect("unavailable source report");
        assert_eq!(
            unavailable_report.outcome,
            catdb_api_types::SourceOutcome::Refused
        );
        assert_eq!(unavailable_report.scanned, 0);
        assert_eq!(unavailable_report.returned, 0);
        assert!(
            answer.answer.rows.iter().all(|row| {
                row.provenance.values().all(|field| {
                    field.considered().iter().all(|observation| {
                        observation.source_version_id() != unavailable_report.source_version_id
                    })
                })
            }),
            "a source that could not observe any record must not appear in per-record provenance"
        );
    }

    #[tokio::test]
    async fn provenance_names_the_physical_attribute_selected_by_the_mapping() {
        let workspace = Uuid::new_v4();
        let drive = Uuid::new_v4();
        let mut query = request(workspace, vec![drive]);
        query.select = vec!["path".to_string(), "title".to_string()];
        let answer = execute_registered_library_query(
            query,
            vec![cloud_member(
                drive,
                Arc::new(CloudFixture::drive()),
                mdreader_domain::library::CloudDocumentSource::GoogleDrive,
            )],
            CancellationToken::new(),
        )
        .await
        .expect("Drive query");
        let row = answer.answer.rows.first().expect("Drive row");

        assert_eq!(
            row.provenance["path"]
                .selected()
                .and_then(|observation| observation.physical_attribute()),
            Some("id")
        );
        assert_eq!(
            row.provenance["title"]
                .selected()
                .and_then(|observation| observation.physical_attribute()),
            Some("name")
        );
    }
}
