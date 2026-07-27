//! The *semantic* half of the command surface: the four reads and two writes
//! the showcase UI drives.
//!
//! | Surface | Commands |
//! |---|---|
//! | **View As** | [`list_schemas`], [`project_corpus`] |
//! | **Semantic Inspector** | [`inspect_span`], [`approve_fact`], [`reject_fact`] |
//! | **Schema View** | [`describe_schema`] |
//! | **Constraint Explorer** | [`check_constraints`] |
//! | **Query Explanation** | [`crate::search::explain_query`] |
//!
//! ## Two graphs, never conflated
//!
//! Everything here reads from one of two places, and which one is never
//! fudged:
//!
//! * the **overlay graph** — the engine's committed objects and morphisms.
//!   Authoritative. An overlay fact is in here only because a human approved
//!   it (the engine refuses anything else).
//! * the **proposal store** — reviewable claims extraction produced during
//!   ingest. Durable, but not authoritative.
//!
//! [`InspectedFact::in_graph`] is the join between them, and it is *measured*
//! against storage rather than inferred from the review state — so a write
//! that landed in one store and not the other shows up as the disagreement it
//! is, instead of being papered over.
//!
//! ## Nothing is re-rendered into prose
//!
//! [`check_constraints`] returns the engine's [`LocatedViolation`] field for
//! field, span included. A violation that knows which markdown line caused it
//! is the whole point; flattening it to a sentence would throw that away at
//! the last hop.
//!
//! ## Epoch-guarded, like everything else
//!
//! Every command captures the current [`Epoch`] once and funnels its result
//! through [`Epoch::finish`]: a workspace switch mid-read discards the
//! previous workspace's facts. The two *writes* are guarded before they touch
//! anything.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use uuid::Uuid;

use catdb_core::Schema;
use catdb_engine::{EmbeddedCatDb, LocatedViolation, SourceSpan, ViolatingObject};
use mdreader_ingest::proposals::{
    approve_and_promote, reject_and_retract, ProposalFilter, ProposalStore, StoredFact,
    StoredProposal,
};

use crate::commands::WorkspaceState;
use crate::engine::Epoch;

// --------------------------------------------------------------------------
// Wire DTOs
// --------------------------------------------------------------------------

/// One registered schema, as the **View As** picker lists it.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaSummary {
    /// Schema name (e.g. `decision_overlay`).
    pub name: String,
    /// The engine's persisted version ordinal for this schema, counting from
    /// 1 in order of first appearance — `None` before the index has been
    /// opened, when the shape is known from the registration but no version
    /// has been assigned to it yet.
    pub version: Option<u32>,
    /// `blake3:` checksum of the persisted schema. Two versions are identical
    /// exactly when these match. `None` under the same condition as
    /// [`Self::version`].
    pub checksum: Option<String>,
    /// Its entities, sorted.
    pub entities: Vec<String>,
    /// How many generating arrows it declares.
    pub arrow_count: u32,
    /// How many attributes it declares.
    pub attribute_count: u32,
    /// How many path equations (laws) it imposes.
    pub equation_count: u32,
    /// Whether the engine requires human approval before a fact may be
    /// asserted into this schema — true exactly for the overlays.
    pub approval_required: bool,
    /// How many objects of this schema are currently committed to the graph.
    /// For an approval-required schema this is the count of *approved* facts,
    /// because nothing else can be in there.
    pub object_count: u32,
    /// How many claims the proposal store holds for this schema, in any review
    /// state. Always `0` for a schema that carries no reviewable assertions.
    pub proposal_count: u32,
}

/// One object in a projection of the corpus through a schema.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectedObject {
    /// Object id (uuid string).
    pub id: String,
    /// Qualified entity name (e.g. `decision_overlay.Decision`).
    pub entity: String,
    /// The object's payload, as compact JSON (specta has no `Value` type).
    pub value: String,
    /// Where the fact came from.
    pub provenance: FactProvenanceDto,
}

/// One typed edge in a projection.
///
/// It carries no provenance and no id: the engine's morphism read
/// (`morphisms_with_arrow_prefix`) returns `(arrow, source, target)` and
/// nothing else. Rather than invent the missing fields, the projection says
/// only what storage actually knows.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProjectedMorphism {
    /// Qualified arrow name (e.g. `architecture_overlay.exposes`).
    pub arrow: String,
    /// Source object id (uuid string).
    pub source: String,
    /// Target object id (uuid string).
    pub target: String,
}

/// The corpus seen through one schema.
#[derive(Debug, Clone, Serialize, Type)]
pub struct Projection {
    /// The schema projected through.
    pub schema: String,
    /// Its objects, ordered by entity then id.
    pub objects: Vec<ProjectedObject>,
    /// Its morphisms, ordered by arrow then endpoints.
    pub morphisms: Vec<ProjectedMorphism>,
    /// True when `limit` cut the object list short.
    pub truncated: bool,
}

/// A fact's derivation record — every field the extractor stamped, unmodified.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FactProvenanceDto {
    /// Repo-relative source file.
    pub file: Option<String>,
    /// First cited line (1-based, inclusive).
    pub start_line: Option<u32>,
    /// Last cited line (1-based, inclusive).
    pub end_line: Option<u32>,
    /// blake3 of exactly the lines the range names.
    pub content_hash: Option<String>,
    /// Version of the schema the fact is typed against.
    pub schema_version: Option<String>,
    /// Extractor id and version (e.g. `mdreader.decision@1`).
    pub extractor: Option<String>,
    /// Extractor confidence in `[0, 1]`.
    pub confidence: Option<f32>,
    /// The reviewer stamped at approval time; `None` until a human accepts it.
    pub approved_by: Option<String>,
}

impl From<&catdb_core::Provenance> for FactProvenanceDto {
    fn from(provenance: &catdb_core::Provenance) -> Self {
        Self {
            file: provenance.source.clone(),
            start_line: provenance.range.map(|(start, _)| start),
            end_line: provenance.range.map(|(_, end)| end),
            content_hash: provenance.content_hash.clone(),
            schema_version: provenance.schema_version.clone(),
            extractor: provenance.extractor.clone(),
            confidence: provenance.confidence,
            approved_by: provenance.approved_by.clone(),
        }
    }
}

impl From<&catdb_ir::Provenance> for FactProvenanceDto {
    fn from(provenance: &catdb_ir::Provenance) -> Self {
        Self {
            file: provenance.source.clone(),
            start_line: provenance.range.map(|[start, _]| start),
            end_line: provenance.range.map(|[_, end]| end),
            content_hash: provenance.content_hash.clone(),
            schema_version: provenance.schema_version.clone(),
            extractor: provenance.extractor.clone(),
            confidence: provenance.confidence,
            approved_by: provenance.approved_by.clone(),
        }
    }
}

/// One extracted fact derived from a span of a document, with everything the
/// Inspector renders about it.
#[derive(Debug, Clone, Serialize, Type)]
pub struct InspectedFact {
    /// The fact's id — the object id or the morphism id (uuid string).
    pub fact_id: String,
    /// `"object"` or `"morphism"`.
    pub kind: String,
    /// The overlay schema the fact is typed against.
    pub schema: String,
    /// Qualified entity (objects) or arrow (morphisms) name.
    pub name: String,
    /// The object's payload as compact JSON; `None` for a morphism.
    pub value: Option<String>,
    /// Source endpoint (uuid string); `None` for an object.
    pub source_id: Option<String>,
    /// Target endpoint (uuid string); `None` for an object.
    pub target_id: Option<String>,
    /// The rule that fired (e.g. `D1`), so a reviewer can see *why*.
    pub rule: String,
    /// Extractor id and version that proposed it.
    pub extractor: String,
    /// Full derivation record.
    pub provenance: FactProvenanceDto,
    /// `"proposed"`, `"approved"`, or `"rejected"`.
    pub state: String,
    /// Who set that state, when anyone has.
    pub reviewer: Option<String>,
    /// Whether the fact is *actually* committed to the overlay graph right
    /// now — read from storage, not inferred from [`Self::state`].
    pub in_graph: bool,
    /// blake3 of the cited text **as it stood when the reviewer ruled on it**;
    /// `None` while the fact is unreviewed.
    pub reviewed_content_hash: Option<String>,
    /// The cited text has changed since the review was made.
    ///
    /// Only ever true for a **preserved rejection**: a re-import keeps a
    /// rejection rather than silently re-proposing the claim a human ruled
    /// out, and raises this instead. Render it as an explicit affordance —
    /// *"previously rejected; the source has since changed — review again?"* —
    /// so un-rejecting stays a deliberate act. (An approval whose text changed
    /// is reset to `proposed` outright, which clears the review, so this stays
    /// false for it.)
    pub source_changed_since_review: bool,
}

/// What approving a fact wrote.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ApprovalOutcomeDto {
    /// The fact that was approved.
    pub fact_id: String,
    /// Ids newly asserted into the overlay graph. More than one when
    /// approving an object also released morphisms it was the missing
    /// endpoint of; empty when the fact is an edge still waiting on an
    /// endpoint.
    pub promoted: Vec<String>,
}

/// What rejecting a fact wrote.
#[derive(Debug, Clone, Serialize, Type)]
pub struct RejectionOutcomeDto {
    /// The fact that was rejected.
    pub fact_id: String,
    /// Ids retracted from the overlay graph. Empty when the fact had never
    /// been approved — nothing was written, so nothing is undone.
    pub retracted: Vec<String>,
    /// Morphisms rejected alongside an object rejection (an edge whose
    /// endpoint is rejected cannot stand).
    pub cascaded: Vec<String>,
}

/// One attribute of an entity.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaAttributeDto {
    /// Attribute name.
    pub name: String,
    /// Typeside type of its values.
    pub ty: String,
    /// Whether it may be absent on a row.
    pub nullable: bool,
}

/// One entity, with the attributes observed on it.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaEntityDto {
    /// Entity name (schema-local, as the schema declares it).
    pub name: String,
    /// Its attributes, sorted by name.
    pub attributes: Vec<SchemaAttributeDto>,
}

/// One generating arrow, with its domain and codomain.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaArrowDto {
    /// Arrow name.
    pub name: String,
    /// Domain entity.
    pub dom: String,
    /// Codomain entity.
    pub cod: String,
}

/// One path equation the schema imposes.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaEquationDto {
    /// The law as the kernel renders it (`Dependency.via.exposes = Dependency.provider`).
    pub law: String,
    /// Entity both sides start at.
    pub start: String,
    /// Arrows composed on the left, in application order.
    pub lhs: Vec<String>,
    /// Arrows composed on the right, in application order.
    pub rhs: Vec<String>,
}

/// A registered schema, fully introspected.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaDetail {
    /// Schema name.
    pub name: String,
    /// The engine's persisted version ordinal; `None` before the index has
    /// been opened (see [`SchemaSummary::version`]).
    pub version: Option<u32>,
    /// `blake3:` checksum of the persisted schema; `None` under the same
    /// condition.
    pub checksum: Option<String>,
    /// Typeside type names the attributes draw from, sorted.
    pub typeside: Vec<String>,
    /// Entities with their attributes.
    pub entities: Vec<SchemaEntityDto>,
    /// Generating arrows with domain and codomain.
    pub arrows: Vec<SchemaArrowDto>,
    /// Path equations.
    pub equations: Vec<SchemaEquationDto>,
    /// Whether the engine requires approval for facts in this schema.
    pub approval_required: bool,
}

/// A source span, exactly as the engine located it.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SourceSpanDto {
    /// Repo-relative file.
    pub file: String,
    /// First line (1-based, inclusive).
    pub start_line: Option<u32>,
    /// Last line (1-based, inclusive).
    pub end_line: Option<u32>,
    /// blake3 of exactly those lines.
    pub content_hash: Option<String>,
}

impl From<SourceSpan> for SourceSpanDto {
    fn from(span: SourceSpan) -> Self {
        Self {
            file: span.file,
            start_line: span.start_line,
            end_line: span.end_line,
            content_hash: span.content_hash,
        }
    }
}

/// An object named by a violation.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ViolatingObjectDto {
    /// Object id (uuid string).
    pub id: String,
    /// Qualified entity name.
    pub entity: String,
    /// The markdown the object was derived from, when it carries a span.
    pub source: Option<SourceSpanDto>,
}

impl From<ViolatingObject> for ViolatingObjectDto {
    fn from(object: ViolatingObject) -> Self {
        Self {
            id: object.id.to_string(),
            entity: object.entity,
            source: object.source.map(Into::into),
        }
    }
}

/// One violated law, field for field from the engine.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ViolationDto {
    /// The schema whose law failed.
    pub schema: String,
    /// The law, as the kernel renders it.
    pub law: String,
    /// Arrows composed on the left.
    pub lhs_path: Vec<String>,
    /// Arrows composed on the right.
    pub rhs_path: Vec<String>,
    /// The object the two paths disagree on — located in the markdown.
    pub subject: ViolatingObjectDto,
    /// Where the left path lands; `None` when a morphism on that side is
    /// missing, which is itself the failure.
    pub lhs: Option<ViolatingObjectDto>,
    /// Where the right path lands; `None` as above.
    pub rhs: Option<ViolatingObjectDto>,
}

impl From<LocatedViolation> for ViolationDto {
    fn from(violation: LocatedViolation) -> Self {
        Self {
            schema: violation.schema,
            law: violation.law,
            lhs_path: violation.lhs_path,
            rhs_path: violation.rhs_path,
            subject: violation.subject.into(),
            lhs: violation.lhs.map(Into::into),
            rhs: violation.rhs.map(Into::into),
        }
    }
}

/// The result of running every registered schema's laws over the graph.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ConstraintReport {
    /// How many schemas were checked.
    pub schemas_checked: u32,
    /// Every violation found, unmodified.
    pub violations: Vec<ViolationDto>,
}

/// A document span to inspect. Absolute path (the shape the reader holds) plus
/// an inclusive 1-based line range; a fact is returned when its cited span
/// **overlaps** the range.
#[derive(Debug, Clone, Deserialize, Type)]
pub struct SpanQuery {
    /// Absolute path of the document, inside the granted workspace.
    pub path: String,
    /// First line of the selection (1-based, inclusive).
    pub start_line: u32,
    /// Last line of the selection (1-based, inclusive).
    pub end_line: u32,
}

// --------------------------------------------------------------------------
// Schema introspection (registration-derived)
// --------------------------------------------------------------------------

/// The domain registration the app hands the engine at open — the same call
/// [`crate::engine`] composes the engine from, so introspecting it describes
/// the schemas the engine actually validated.
///
/// A schema the surfaces can describe: its kernel shape, plus the version
/// identity the engine persisted for it.
struct KnownSchema {
    schema: Schema,
    /// Persisted version ordinal and content checksum, once the engine has
    /// stored this schema. `None` before the index has been opened.
    version: Option<u32>,
    checksum: Option<String>,
    approval_required: bool,
}

/// The schemas this workspace can be viewed through.
///
/// The **shape** comes from what the engine persisted
/// ([`EmbeddedCatDb::stored_schemas`], newest version per name, reconstructed
/// through [`StoredSchema::schema`]) — so what the Schema View draws is the
/// engine's own stored bytes, not a re-derivation that merely ought to match.
/// Each carries its persisted version ordinal and content checksum.
///
/// Two things still come from the registration, because the schema catalog
/// does not persist them:
///
/// * **Order.** Listing follows the domain's declaration order (`fs`, `md`,
///   `kn`, `rx`, then the provenance cores, then the overlays) rather than the
///   catalog's, because that order is the pipeline and the picker reads
///   top-down. The catalog orders by name.
/// * **`approval_required`.** It is a property of the *registration*
///   ([`DomainRegistration::approval_required_schemas`]), not of the schema,
///   and the stored spec envelope carries only the kernel schema body.
///
/// Before the engine is installed (no import yet) the shape falls back to the
/// registration's own copy with no version — the schemas are genuinely known,
/// they have simply not been versioned yet, and saying so beats inventing a
/// number.
///
/// A schema the catalog holds but the live domain no longer registers is not
/// listed: nothing here could answer `approval_required` for it honestly.
fn known_schemas(db: Option<&EmbeddedCatDb>) -> Vec<KnownSchema> {
    let registration = mdreader_ingest::registration();
    let approval: Vec<String> = registration
        .approval_required_schemas
        .iter()
        .map(ToString::to_string)
        .collect();

    // Newest persisted version per name. A read failure degrades to the
    // registration shape rather than blanking the picker.
    let mut latest: std::collections::HashMap<String, catdb_engine::StoredSchema> =
        std::collections::HashMap::new();
    for stored in db
        .and_then(|db| db.stored_schemas().ok())
        .unwrap_or_default()
    {
        let newer = latest
            .get(&stored.name)
            .is_none_or(|held| stored.version > held.version);
        if newer {
            latest.insert(stored.name.clone(), stored);
        }
    }

    registration
        .schemas
        .into_iter()
        .map(|declared| {
            let name = declared.name.to_string();
            let approval_required = approval.contains(&name);
            match latest.get(&name).and_then(|stored| {
                stored
                    .schema()
                    .ok()
                    .map(|schema| (schema, stored.version, stored.checksum.clone()))
            }) {
                Some((schema, version, checksum)) => KnownSchema {
                    schema,
                    version: Some(version),
                    checksum: Some(checksum),
                    approval_required,
                },
                None => KnownSchema {
                    schema: declared,
                    version: None,
                    checksum: None,
                    approval_required,
                },
            }
        })
        .collect()
}

fn schema_summary(
    known: &KnownSchema,
    engine: Option<&EmbeddedCatDb>,
    store: Option<&ProposalStore>,
) -> SchemaSummary {
    let schema = &known.schema;
    let prefix = format!("{}.", schema.name);
    let object_count = engine
        .and_then(|db| db.store().objects_with_entity_prefix(&prefix).ok())
        .map(|objects| objects.len() as u32)
        .unwrap_or(0);
    let proposal_count = store
        .and_then(|store| {
            store
                .list(&ProposalFilter {
                    schema: Some(schema.name.to_string()),
                    ..ProposalFilter::default()
                })
                .ok()
        })
        .map(|proposals| proposals.len() as u32)
        .unwrap_or(0);
    SchemaSummary {
        name: schema.name.to_string(),
        version: known.version,
        checksum: known.checksum.clone(),
        entities: schema.entities.iter().map(ToString::to_string).collect(),
        arrow_count: schema.arrows.len() as u32,
        attribute_count: schema.attributes.len() as u32,
        equation_count: schema.equations.len() as u32,
        approval_required: known.approval_required,
        object_count,
        proposal_count,
    }
}

fn schema_detail(known: &KnownSchema) -> SchemaDetail {
    let schema = &known.schema;
    let approval_required = known.approval_required;
    let entities = schema
        .entities
        .iter()
        .map(|entity| SchemaEntityDto {
            name: entity.to_string(),
            attributes: schema
                .attributes
                .values()
                .filter(|attribute| attribute.dom == *entity)
                .map(|attribute| SchemaAttributeDto {
                    name: attribute.name.to_string(),
                    ty: attribute.ty.to_string(),
                    nullable: attribute.nullable,
                })
                .collect(),
        })
        .collect();
    SchemaDetail {
        name: schema.name.to_string(),
        version: known.version,
        checksum: known.checksum.clone(),
        typeside: schema
            .typeside
            .types
            .keys()
            .map(ToString::to_string)
            .collect(),
        entities,
        arrows: schema
            .arrows
            .values()
            .map(|arrow| SchemaArrowDto {
                name: arrow.name.to_string(),
                dom: arrow.dom.to_string(),
                cod: arrow.cod.to_string(),
            })
            .collect(),
        equations: schema
            .equations
            .iter()
            .map(|equation| SchemaEquationDto {
                law: equation.to_string(),
                start: equation.lhs.start.to_string(),
                lhs: equation
                    .lhs
                    .arrows
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                rhs: equation
                    .rhs
                    .arrows
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            })
            .collect(),
        approval_required,
    }
}

// --------------------------------------------------------------------------
// View As
// --------------------------------------------------------------------------

/// Every schema this workspace can be viewed through — persisted version and
/// checksum included — with live counts of what the graph and the proposal
/// store hold for each.
///
/// Before the engine is installed (no import yet) the shape is still complete
/// — the schemas are known from the registration — while the version, the
/// checksum, and the counts are absent or `0`, which is the truth rather than
/// a placeholder. See [`known_schemas`].
#[tauri::command(async)]
#[specta::specta]
pub fn list_schemas(state: tauri::State<'_, WorkspaceState>) -> Result<Vec<SchemaSummary>, String> {
    list_schemas_guarded(&state, || {})
}

fn list_schemas_guarded(
    state: &WorkspaceState,
    gate: impl FnOnce(),
) -> Result<Vec<SchemaSummary>, String> {
    let epoch = current(state)?;
    let db = epoch.db();
    let store = epoch.proposals();
    let summaries = known_schemas(db.as_deref())
        .iter()
        .map(|known| schema_summary(known, db.as_deref(), store.as_deref()))
        .collect();
    gate();
    // EPOCH-GUARDED
    epoch
        .finish(state, summaries)
        .map_err(|error| error.to_string())
}

/// The corpus projected through one schema: its committed objects (with
/// provenance) and typed edges.
///
/// This is the *graph*, not the proposal store — so for an overlay it shows
/// exactly the facts a human approved, which is what "the semantic view of
/// this corpus" means. `limit` caps the object list; [`Projection::truncated`]
/// says whether it bit.
#[tauri::command(async)]
#[specta::specta]
pub fn project_corpus(
    schema: String,
    limit: u32,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<Projection, String> {
    project_corpus_guarded(&state, &schema, limit, || {})
}

fn project_corpus_guarded(
    state: &WorkspaceState,
    schema: &str,
    limit: u32,
    gate: impl FnOnce(),
) -> Result<Projection, String> {
    let epoch = current(state)?;
    let db = epoch.db();
    let schemas = known_schemas(db.as_deref());
    let known = schemas
        .iter()
        .find(|known| known.schema.name.to_string() == schema)
        .ok_or_else(|| format!("no registered schema named `{schema}`"))?;
    let prefix = format!("{schema}.");
    let mut projection = Projection {
        schema: schema.to_string(),
        objects: Vec::new(),
        morphisms: Vec::new(),
        truncated: false,
    };
    if let Some(db) = db {
        // Provenance and payload live in two different reads; join by id
        // rather than pick one and leave the other field empty.
        let mut headers = db
            .store()
            .objects_with_entity_prefix(&prefix)
            .map_err(|err| format!("read objects: {err}"))?;
        headers.sort_by(|left, right| (&left.entity, left.id).cmp(&(&right.entity, right.id)));
        projection.truncated = headers.len() > limit as usize;
        headers.truncate(limit as usize);
        let mut values = std::collections::HashMap::new();
        for entity in known
            .schema
            .entities
            .iter()
            .map(|entity| format!("{schema}.{entity}"))
        {
            for (id, value) in db
                .store()
                .objects_with_entity(&entity)
                .map_err(|err| format!("read values: {err}"))?
            {
                values.insert(id, value);
            }
        }
        projection.objects = headers
            .into_iter()
            .map(|header| ProjectedObject {
                id: header.id.to_string(),
                entity: header.entity,
                value: values
                    .get(&header.id)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "null".to_string()),
                provenance: (&header.provenance).into(),
            })
            .collect();

        let mut edges = db
            .store()
            .morphisms_with_arrow_prefix(&prefix)
            .map_err(|err| format!("read morphisms: {err}"))?;
        edges.sort_by(|left, right| {
            (&left.arrow, left.source, left.target).cmp(&(&right.arrow, right.source, right.target))
        });
        projection.morphisms = edges
            .into_iter()
            .map(|edge| ProjectedMorphism {
                arrow: edge.arrow,
                source: edge.source.to_string(),
                target: edge.target.to_string(),
            })
            .collect();
    }
    gate();
    // EPOCH-GUARDED
    epoch
        .finish(state, projection)
        .map_err(|error| error.to_string())
}

// --------------------------------------------------------------------------
// Schema View
// --------------------------------------------------------------------------

/// Introspect one registered schema: entities with their attributes, arrows
/// with domain and codomain, and the path equations it imposes — read back
/// from what the engine persisted, and carrying that version's ordinal and
/// checksum (see [`known_schemas`]).
#[tauri::command(async)]
#[specta::specta]
pub fn describe_schema(
    schema: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<SchemaDetail, String> {
    describe_schema_guarded(&state, &schema, || {})
}

fn describe_schema_guarded(
    state: &WorkspaceState,
    schema: &str,
    gate: impl FnOnce(),
) -> Result<SchemaDetail, String> {
    let epoch = current(state)?;
    let schemas = known_schemas(epoch.db().as_deref());
    let found = schemas
        .iter()
        .find(|known| known.schema.name.to_string() == schema)
        .ok_or_else(|| format!("no registered schema named `{schema}`"))?;
    let detail = schema_detail(found);
    gate();
    // EPOCH-GUARDED
    epoch
        .finish(state, detail)
        .map_err(|error| error.to_string())
}

// --------------------------------------------------------------------------
// Semantic Inspector
// --------------------------------------------------------------------------

/// The extracted facts derived from a span of a document — every object and
/// typed morphism whose cited range overlaps `query`'s selection.
///
/// Each carries its full derivation record (file, lines, content hash,
/// schema version, extractor, confidence), the rule that fired, its approval
/// state and reviewer, and whether it is currently in the graph.
#[tauri::command(async)]
#[specta::specta]
pub fn inspect_span(
    query: SpanQuery,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<Vec<InspectedFact>, String> {
    inspect_span_guarded(&state, &query, || {})
}

fn inspect_span_guarded(
    state: &WorkspaceState,
    query: &SpanQuery,
    gate: impl FnOnce(),
) -> Result<Vec<InspectedFact>, String> {
    let epoch = current(state)?;
    let Some(store) = epoch.proposals() else {
        // No index built yet: honestly empty rather than an error, so the
        // Inspector renders "nothing extracted here" instead of a failure.
        gate();
        // EPOCH-GUARDED
        return epoch
            .finish(state, Vec::new())
            .map_err(|error| error.to_string());
    };
    let relative = relative_to_root(&epoch, &query.path);
    let (start, end) = (
        query.start_line.min(query.end_line),
        query.start_line.max(query.end_line),
    );
    let proposals = store
        .list(&ProposalFilter {
            source_path: Some(relative),
            line_range: Some((start, end)),
            ..ProposalFilter::default()
        })
        .map_err(|err| format!("read proposals: {err}"))?;
    let db = epoch.db();
    let facts = proposals
        .iter()
        .map(|proposal| inspected(proposal, db.as_deref()))
        .collect();
    gate();
    // EPOCH-GUARDED
    epoch
        .finish(state, facts)
        .map_err(|error| error.to_string())
}

/// Accept a proposed fact and promote it into the overlay graph.
///
/// The promotion runs through `Extraction::approved_operations` — the only
/// method that yields engine operations, and only for facts a human accepted
/// — so the engine's approval check passes because the fact genuinely *was*
/// approved, with `reviewer` stamped on its provenance.
///
/// A blank reviewer is refused here rather than at the engine: a
/// whitespace-only name counts as unapproved, so accepting it would produce a
/// confusing rejection two layers down.
#[tauri::command]
#[specta::specta]
pub async fn approve_fact(
    fact_id: String,
    reviewer: String,
    workspace: State<'_, WorkspaceState>,
) -> Result<ApprovalOutcomeDto, String> {
    let target = review_target(&workspace, &fact_id, &reviewer)?;
    let outcome = approve_and_promote(&target.store, &target.db, target.fact_id, reviewer.trim())
        .await
        .map_err(|err| err.to_string())?;
    // EPOCH-GUARDED
    target
        .epoch
        .finish(
            &workspace,
            ApprovalOutcomeDto {
                fact_id: outcome.fact_id.to_string(),
                promoted: outcome.promoted.iter().map(ToString::to_string).collect(),
            },
        )
        .map_err(|error| error.to_string())
}

/// Reject a fact, and retract it if a previous approval had already committed
/// it.
///
/// Rejecting an approved object cascades to every approved morphism it
/// anchors: an edge whose endpoint a human ruled out cannot stand.
#[tauri::command]
#[specta::specta]
pub async fn reject_fact(
    fact_id: String,
    reviewer: String,
    workspace: State<'_, WorkspaceState>,
) -> Result<RejectionOutcomeDto, String> {
    let target = review_target(&workspace, &fact_id, &reviewer)?;
    let outcome = reject_and_retract(&target.store, &target.db, target.fact_id, reviewer.trim())
        .await
        .map_err(|err| err.to_string())?;
    // EPOCH-GUARDED
    target
        .epoch
        .finish(
            &workspace,
            RejectionOutcomeDto {
                fact_id: outcome.fact_id.to_string(),
                retracted: outcome.retracted.iter().map(ToString::to_string).collect(),
                cascaded: outcome.cascaded.iter().map(ToString::to_string).collect(),
            },
        )
        .map_err(|error| error.to_string())
}

// --------------------------------------------------------------------------
// Constraint Explorer
// --------------------------------------------------------------------------

/// Run every registered schema's path equations over the committed graph and
/// return the violations **as the engine located them** — schema, law, both
/// paths, the offending object, and the markdown span it came from.
///
/// Nothing is re-rendered to prose: a violation that knows which line of which
/// file caused it is the entire value of the surface.
#[tauri::command]
#[specta::specta]
pub async fn check_constraints(
    workspace: State<'_, WorkspaceState>,
) -> Result<ConstraintReport, String> {
    let epoch = current(&workspace)?;
    let Some(db) = epoch.db() else {
        // EPOCH-GUARDED
        return epoch
            .finish(
                &workspace,
                ConstraintReport {
                    schemas_checked: 0,
                    violations: Vec::new(),
                },
            )
            .map_err(|error| error.to_string());
    };
    let report = db.check_equations().await.map_err(|err| err.to_string())?;
    let report = ConstraintReport {
        schemas_checked: report.schemas_checked as u32,
        violations: report.violations.into_iter().map(Into::into).collect(),
    };
    // EPOCH-GUARDED
    epoch
        .finish(&workspace, report)
        .map_err(|error| error.to_string())
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

fn current(state: &WorkspaceState) -> Result<Arc<Epoch>, String> {
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    epoch.validate_root()?;
    Ok(epoch)
}

/// Everything a review command must resolve *before* it touches either store:
/// the epoch it is guarded by, that epoch's two stores, and the parsed fact id.
struct ReviewTarget {
    epoch: Arc<Epoch>,
    store: Arc<ProposalStore>,
    db: Arc<EmbeddedCatDb>,
    fact_id: Uuid,
}

/// Resolve a review request, failing before any write when the reviewer is
/// blank, the id is malformed, or the index is not open.
fn review_target(
    state: &WorkspaceState,
    fact_id: &str,
    reviewer: &str,
) -> Result<ReviewTarget, String> {
    if reviewer.trim().is_empty() {
        return Err("a reviewer is required: approval records WHO accepted the fact".to_string());
    }
    let epoch = current(state)?;
    let store = epoch
        .proposals()
        .ok_or_else(|| "the index is not open yet".to_string())?;
    let db = epoch
        .db()
        .ok_or_else(|| "the index is not open yet".to_string())?;
    let fact_id = Uuid::parse_str(fact_id).map_err(|err| format!("not a fact id: {err}"))?;
    Ok(ReviewTarget {
        epoch,
        store,
        db,
        fact_id,
    })
}

/// Map an absolute document path back to the source-relative stored form the
/// extractors recorded. A path outside the root is passed through unchanged
/// (it simply matches nothing).
fn relative_to_root(epoch: &Epoch, path: &str) -> String {
    std::path::Path::new(path)
        .strip_prefix(&epoch.root)
        .map(|rest| rest.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string())
}

fn inspected(proposal: &StoredProposal, db: Option<&EmbeddedCatDb>) -> InspectedFact {
    let (kind, value, source_id, target_id) = match &proposal.fact {
        StoredFact::Object(object) => ("object", Some(object.object.value.to_string()), None, None),
        StoredFact::Morphism(morphism) => (
            "morphism",
            None,
            Some(morphism.morphism.source.to_string()),
            Some(morphism.morphism.target.to_string()),
        ),
    };
    InspectedFact {
        fact_id: proposal.fact_id.to_string(),
        kind: kind.to_string(),
        schema: proposal.schema.clone(),
        name: proposal.qualified(),
        value,
        source_id,
        target_id,
        rule: proposal.rule().to_string(),
        extractor: proposal.extractor.clone(),
        provenance: proposal.provenance().into(),
        state: state_label(proposal),
        reviewer: proposal.review().reviewer.clone(),
        in_graph: db.is_some_and(|db| committed(db, proposal)),
        reviewed_content_hash: proposal.reviewed_content_hash.clone(),
        source_changed_since_review: proposal.source_changed_since_review(),
    }
}

fn state_label(proposal: &StoredProposal) -> String {
    match proposal.state() {
        mdreader_domain::extract::ApprovalState::Proposed => "proposed",
        mdreader_domain::extract::ApprovalState::Approved => "approved",
        mdreader_domain::extract::ApprovalState::Rejected => "rejected",
    }
    .to_string()
}

/// Whether the fact is committed to the graph *right now*.
///
/// Objects are looked up by id. Morphisms have no id in the engine's edge read
/// (`MorphismEdge` is `(arrow, source, target)`), so they are matched on those
/// three — which identifies them uniquely, because the extractor derives a
/// morphism's id deterministically from exactly that triple.
fn committed(db: &EmbeddedCatDb, proposal: &StoredProposal) -> bool {
    match &proposal.fact {
        StoredFact::Object(_) => db.store().object_exists(proposal.fact_id).unwrap_or(false),
        StoredFact::Morphism(morphism) => {
            let arrow = morphism.morphism.arrow.to_string();
            let (source, target) = (
                *morphism.morphism.source.as_uuid(),
                *morphism.morphism.target.as_uuid(),
            );
            db.store()
                .morphisms_with_arrow_prefix(&arrow)
                .map(|edges| {
                    edges.iter().any(|edge| {
                        edge.arrow == arrow && edge.source == source && edge.target == target
                    })
                })
                .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use catdb_embed::{Embedder, HashEmbedder};
    use catdb_engine::{EngineConfig, ImportOptions};
    use tempfile::TempDir;

    /// A tiny corpus that exercises both overlays AND breaks one law:
    /// `broken-app` says it depends on `catdb-core` *through* `Search API`,
    /// but `Search API` is exposed by `catdb-engine`. That is exactly the
    /// architecture overlay's `Dependency.via.exposes = Dependency.provider`.
    fn write_corpus(root: &std::path::Path) {
        let files = [
            (
                "docs/adr/0003-storage.md",
                "---\ntype: decision\ntitle: Use flat files\n---\n\n# Use flat files\n\nThe first cut.\n",
            ),
            (
                "docs/adr/0007-storage.md",
                "---\ntype: decision\ntitle: Store the catalog in SQLite\n---\n\n# Store the catalog in SQLite\n\n## Supersedes\n\n- [[0003-storage]]\n",
            ),
            (
                "docs/arch/catdb-core.md",
                "---\ntype: crate\nname: catdb-core\nlayer: platform\n---\n\n# catdb-core\n\nThe kernel.\n\n## Interfaces\n\n### Schema Validation\n\nChecks schemas.\n",
            ),
            (
                "docs/arch/catdb-engine.md",
                "---\ntype: crate\nname: catdb-engine\nlayer: platform\n---\n\n# catdb-engine\n\nThe engine.\n\n## Interfaces\n\n### Search API\n\nHybrid retrieval.\n",
            ),
            (
                "docs/arch/broken-app.md",
                "---\ntype: crate\nname: broken-app\nlayer: app\n---\n\n# broken-app\n\nThe app.\n\n## Depends on\n\n- [[catdb-core#Search API]]\n",
            ),
        ];
        for (rel, body) in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
    }

    /// A granted workspace whose epoch already holds a real engine and the
    /// proposal store that engine's ingest recorded into — the state every
    /// command below reads through.
    struct Harness {
        _corpus: TempDir,
        _index: TempDir,
        root: PathBuf,
        state: WorkspaceState,
    }

    impl Harness {
        fn new() -> Self {
            let corpus = TempDir::new().unwrap();
            let root = fs::canonicalize(corpus.path()).unwrap();
            write_corpus(&root);

            let index = TempDir::new().unwrap();
            let store = Arc::new(
                ProposalStore::open(index.path().join("proposals.db")).expect("proposal store"),
            );
            let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
            let mut config = EngineConfig::in_memory(embedder).with_actor("overlay-test");
            config.domains = vec![mdreader_ingest::registration_with_proposals(Arc::clone(
                &store,
            ))];
            let db = Arc::new(EmbeddedCatDb::open(config).expect("open in-memory engine"));

            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(db.import(&root, ImportOptions::default()))
                .expect("import corpus");

            let state = WorkspaceState::granted(root.clone());
            let epoch = state.current().unwrap();
            epoch.install_proposals_for_test(store);
            epoch.install_engine_for_test(db);
            Harness {
                _corpus: corpus,
                _index: index,
                root,
                state,
            }
        }

        fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(future)
        }

        fn store(&self) -> Arc<ProposalStore> {
            self.state.current().unwrap().proposals().unwrap()
        }

        fn db(&self) -> Arc<EmbeddedCatDb> {
            self.state.current().unwrap().db().unwrap()
        }

        fn inspect(&self, rel: &str, start: u32, end: u32) -> Vec<InspectedFact> {
            inspect_span_guarded(
                &self.state,
                &SpanQuery {
                    path: self.root.join(rel).to_string_lossy().into_owned(),
                    start_line: start,
                    end_line: end,
                },
                || {},
            )
            .expect("inspect")
        }
    }

    // ----------------------------------------------------------------------
    // View As
    // ----------------------------------------------------------------------

    #[test]
    fn overlay_completion_keeps_source_identity_failure_distinct_from_a_switch() {
        let temp = TempDir::new().expect("tempdir");
        let granted = temp.path().join("notes");
        fs::create_dir(&granted).expect("create granted root");
        let granted = fs::canonicalize(granted).expect("canonical granted root");
        let state = WorkspaceState::granted(granted.clone());
        let moved = temp.path().join("moved-original");

        let error = list_schemas_guarded(&state, || {
            fs::rename(&granted, &moved).expect("move admitted root");
            fs::create_dir(&granted).expect("create same-path replacement");
        })
        .expect_err("overlay result must refuse replacement of its source root");

        assert!(
            error.contains("identity"),
            "identity refusal must not be flattened to a stale switch: {error}"
        );
    }

    /// The version and checksum are the ENGINE's, read back from what it
    /// persisted — not a number this module made up, and not a re-derivation
    /// that merely ought to agree.
    ///
    /// Asserted by looking the same schema up through
    /// [`EmbeddedCatDb::stored_schemas`] and requiring an exact match on both
    /// fields, so a summary can only carry a version the catalog actually
    /// holds.
    #[test]
    fn schema_versions_and_checksums_come_from_the_engines_own_catalog() {
        let harness = Harness::new();
        let summaries = list_schemas_guarded(&harness.state, || {}).expect("list");
        let stored = harness.db().stored_schemas().expect("stored schemas");
        assert!(
            !stored.is_empty(),
            "opening the engine persisted the domain"
        );

        for summary in &summaries {
            let newest = stored
                .iter()
                .filter(|s| s.name == summary.name)
                .max_by_key(|s| s.version)
                .unwrap_or_else(|| panic!("`{}` was persisted", summary.name));
            assert_eq!(summary.version, Some(newest.version), "{}", summary.name);
            assert_eq!(
                summary.checksum.as_deref(),
                Some(newest.checksum.as_str()),
                "{}",
                summary.name
            );
        }

        // ...and the SHAPE is the persisted one too: the detail view is built
        // from `StoredSchema::schema()`, so it must equal that reconstruction.
        let detail =
            describe_schema_guarded(&harness.state, "architecture_overlay", || {}).expect("detail");
        let persisted = stored
            .iter()
            .filter(|s| s.name == "architecture_overlay")
            .max_by_key(|s| s.version)
            .expect("persisted")
            .schema()
            .expect("reconstructs");
        assert_eq!(detail.version, Some(1));
        let mut arrows: Vec<String> = detail.arrows.iter().map(|a| a.name.clone()).collect();
        arrows.sort();
        let mut expected: Vec<String> = persisted.arrows.keys().map(ToString::to_string).collect();
        expected.sort();
        assert_eq!(arrows, expected);
        assert_eq!(detail.equations.len(), persisted.equations.len());
    }

    /// The picker lists every registered schema, flags exactly the overlays as
    /// approval-required, and reports live counts from the two stores.
    #[test]
    fn list_schemas_names_every_registered_schema_and_flags_the_overlays() {
        let harness = Harness::new();
        let summaries = list_schemas_guarded(&harness.state, || {}).expect("list");

        let names: Vec<&str> = summaries.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "fs",
                "md",
                "kn",
                "rx",
                "md_core",
                "kn_core",
                "rx_core",
                "document_overlay",
                "decision_overlay",
                "architecture_overlay",
                // The federated `Document` concept. It is NOT approval-required:
                // it records which source answered each field, rather than
                // proposing an interpretation a human rules on.
                "document",
            ]
        );
        let approval: Vec<&str> = summaries
            .iter()
            .filter(|s| s.approval_required)
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(
            approval,
            vec![
                "document_overlay",
                "decision_overlay",
                "architecture_overlay"
            ]
        );

        // The mechanical pipeline schemas are populated by the import...
        let md = summaries.iter().find(|s| s.name == "md").unwrap();
        assert!(md.object_count > 0, "the corpus was indexed");
        assert_eq!(md.proposal_count, 0, "md holds no reviewable claims");
        // ...while the overlays hold proposals and NOT graph objects.
        let decision = summaries
            .iter()
            .find(|s| s.name == "decision_overlay")
            .unwrap();
        assert_eq!(decision.object_count, 0, "nothing approved yet");
        assert!(decision.proposal_count > 0, "but facts were proposed");
    }

    /// Projecting the corpus through a schema shows what is *committed*, with
    /// each object's provenance — so an overlay projection is empty until
    /// somebody approves something.
    #[test]
    fn project_corpus_shows_the_committed_graph_with_provenance() {
        let harness = Harness::new();

        let overlay = project_corpus_guarded(&harness.state, "architecture_overlay", 100, || {})
            .expect("project");
        assert!(overlay.objects.is_empty(), "nothing approved yet");
        assert!(overlay.morphisms.is_empty());
        assert!(!overlay.truncated);

        let md = project_corpus_guarded(&harness.state, "md", 100, || {}).expect("project");
        assert!(!md.objects.is_empty());
        let document = md
            .objects
            .iter()
            .find(|object| object.entity == "md.Document")
            .expect("documents are projected");
        assert!(document.value.contains("file_path"));
        assert!(document.provenance.file.is_some());
        assert!(!md.morphisms.is_empty(), "typed edges are projected too");

        // An unregistered schema is an error, not an empty projection — the
        // caller asked for something that does not exist.
        assert!(project_corpus_guarded(&harness.state, "nope", 10, || {}).is_err());
    }

    /// `limit` truncates and says so, rather than silently returning a partial
    /// list a UI would render as the whole graph.
    #[test]
    fn project_corpus_reports_truncation_instead_of_lying_about_completeness() {
        let harness = Harness::new();
        let full = project_corpus_guarded(&harness.state, "md", 1000, || {}).expect("project");
        assert!(!full.truncated);
        let clipped = project_corpus_guarded(&harness.state, "md", 2, || {}).expect("project");
        assert_eq!(clipped.objects.len(), 2);
        assert!(clipped.truncated);
    }

    // ----------------------------------------------------------------------
    // Schema View
    // ----------------------------------------------------------------------

    /// Introspection returns the schema as declared: entities with their
    /// attributes, arrows with domain and codomain, and the path equations.
    #[test]
    fn describe_schema_returns_entities_arrows_and_path_equations() {
        let harness = Harness::new();
        let detail = describe_schema_guarded(&harness.state, "architecture_overlay", || {})
            .expect("describe");

        assert!(detail.approval_required);
        let entities: Vec<&str> = detail.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(entities.contains(&"Component"));
        assert!(entities.contains(&"Dependency"));
        assert!(entities.contains(&"Interface"));

        let component = detail
            .entities
            .iter()
            .find(|entity| entity.name == "Component")
            .unwrap();
        let attributes: Vec<&str> = component
            .attributes
            .iter()
            .map(|attribute| attribute.name.as_str())
            .collect();
        assert!(attributes.contains(&"component_name"), "got {attributes:?}");

        let exposes = detail
            .arrows
            .iter()
            .find(|arrow| arrow.name == "exposes")
            .expect("exposes is declared");
        assert_eq!(exposes.dom, "Interface");
        assert_eq!(exposes.cod, "Component");

        // The law the Constraint Explorer will report a violation of.
        let law = detail
            .equations
            .iter()
            .find(|equation| equation.lhs == vec!["via".to_string(), "exposes".to_string()])
            .expect("the interface-ownership law is declared");
        assert_eq!(law.start, "Dependency");
        assert_eq!(law.rhs, vec!["provider".to_string()]);
        assert!(
            law.law.contains('='),
            "rendered as an equation: {}",
            law.law
        );

        assert!(describe_schema_guarded(&harness.state, "nope", || {}).is_err());
    }

    // ----------------------------------------------------------------------
    // Semantic Inspector
    // ----------------------------------------------------------------------

    /// A span of a document yields the facts derived from it, each with its
    /// rule, extractor@version, schema version, confidence, cited lines,
    /// content hash, and review state.
    #[test]
    fn inspect_span_returns_located_facts_with_their_full_derivation() {
        let harness = Harness::new();
        let facts = harness.inspect("docs/arch/catdb-engine.md", 11, 16);

        let interface = facts
            .iter()
            .find(|fact| fact.name == "architecture_overlay.Interface")
            .expect("the Search API interface was extracted from this span");
        assert_eq!(interface.kind, "object");
        assert_eq!(interface.rule, "A3");
        assert_eq!(interface.extractor, "mdreader.architecture@1");
        assert_eq!(interface.schema, "architecture_overlay");
        assert_eq!(interface.state, "proposed");
        assert_eq!(interface.reviewer, None);
        assert!(!interface.in_graph);
        assert_eq!(
            interface.provenance.file.as_deref(),
            Some("docs/arch/catdb-engine.md")
        );
        assert_eq!(
            interface.provenance.extractor.as_deref(),
            Some("mdreader.architecture@1")
        );
        assert_eq!(interface.provenance.schema_version.as_deref(), Some("1"));
        assert_eq!(interface.provenance.confidence, Some(0.85));
        assert!(interface.provenance.content_hash.is_some());
        assert_eq!(interface.provenance.approved_by, None);
        assert!(interface.value.as_deref().unwrap().contains("Search API"));

        // Morphisms come through too, with their endpoints.
        let exposes = facts
            .iter()
            .find(|fact| fact.name == "architecture_overlay.exposes")
            .expect("the exposes edge was extracted from this span");
        assert_eq!(exposes.kind, "morphism");
        assert_eq!(exposes.value, None);
        assert_eq!(
            exposes.source_id.as_deref(),
            Some(interface.fact_id.as_str())
        );
        assert!(exposes.target_id.is_some());
    }

    /// The span filter is an OVERLAP test on the cited range, so a selection
    /// inside one section does not drag in the neighbouring section's facts.
    #[test]
    fn inspect_span_returns_only_facts_whose_cited_range_overlaps_the_selection() {
        let harness = Harness::new();
        let interfaces = harness.inspect("docs/arch/catdb-core.md", 13, 16);
        assert!(interfaces
            .iter()
            .any(|fact| fact.name == "architecture_overlay.Interface"));
        // A one-line selection on the `# catdb-core` title only catches the
        // whole-file facts (A1/A2), never the section-scoped ones.
        let title = harness.inspect("docs/arch/catdb-core.md", 7, 7);
        assert!(title
            .iter()
            .all(|fact| fact.name != "architecture_overlay.Interface"));
        assert!(title
            .iter()
            .any(|fact| fact.name == "architecture_overlay.Component"));
    }

    /// Approving through the command surface puts the fact in the graph, with
    /// the reviewer on its provenance — and `in_graph` flips because it is
    /// measured, not assumed.
    #[test]
    fn approve_fact_promotes_through_the_approved_path_and_in_graph_follows() {
        let harness = Harness::new();
        let component = harness
            .inspect("docs/arch/catdb-engine.md", 1, 3)
            .into_iter()
            .find(|fact| fact.name == "architecture_overlay.Component")
            .expect("the component was proposed");
        assert!(!component.in_graph);

        let outcome = harness
            .block_on(approve_and_promote(
                &harness.store(),
                &harness.db(),
                Uuid::parse_str(&component.fact_id).unwrap(),
                "alice",
            ))
            .expect("approve");
        assert_eq!(outcome.promoted.len(), 1);

        let after = harness
            .inspect("docs/arch/catdb-engine.md", 1, 3)
            .into_iter()
            .find(|fact| fact.fact_id == component.fact_id)
            .expect("still inspectable");
        assert_eq!(after.state, "approved");
        assert_eq!(after.reviewer.as_deref(), Some("alice"));
        assert!(after.in_graph, "measured against storage, not inferred");

        // ...and the projection now shows it, reviewer included.
        let projection = project_corpus_guarded(&harness.state, "architecture_overlay", 100, || {})
            .expect("project");
        assert_eq!(projection.objects.len(), 1);
        assert_eq!(
            projection.objects[0].provenance.approved_by.as_deref(),
            Some("alice")
        );
    }

    /// `in_graph` is **measured against storage**, never inferred from the
    /// review state — and the two genuinely diverge.
    ///
    /// Approving an edge whose endpoints are still proposals is a legal
    /// review action, but the edge is withheld (a dangling morphism must not
    /// enter the graph). So the fact reads `approved` and `in_graph = false`
    /// at the same time, which is the truth. Deriving the flag from the state
    /// would report the edge as committed when the graph does not hold it —
    /// exactly the class of lie this field exists to prevent.
    #[test]
    fn in_graph_is_measured_so_an_approved_but_withheld_edge_reads_as_absent() {
        let harness = Harness::new();
        let edge = harness
            .inspect("docs/adr/0007-storage.md", 10, 10)
            .into_iter()
            .find(|fact| fact.name == "decision_overlay.supersedes")
            .expect("the supersedes edge was extracted from the link line");
        assert_eq!(edge.kind, "morphism");

        let outcome = harness
            .block_on(approve_and_promote(
                &harness.store(),
                &harness.db(),
                Uuid::parse_str(&edge.fact_id).unwrap(),
                "alice",
            ))
            .expect("approving an edge is legal");
        assert!(
            outcome.promoted.is_empty(),
            "both endpoints are still proposals, so the edge is withheld"
        );

        let after = harness
            .inspect("docs/adr/0007-storage.md", 10, 10)
            .into_iter()
            .find(|fact| fact.fact_id == edge.fact_id)
            .expect("still inspectable");
        assert_eq!(after.state, "approved", "the review really did happen");
        assert!(
            !after.in_graph,
            "...but the graph does not hold it, and the field says so"
        );
        assert!(
            project_corpus_guarded(&harness.state, "decision_overlay", 100, || {})
                .expect("project")
                .morphisms
                .is_empty(),
            "the projection agrees with in_graph"
        );
    }

    /// Rejecting keeps the fact out of the graph and records who said no, so a
    /// later import does not re-ask.
    #[test]
    fn reject_fact_keeps_it_out_of_the_graph_and_records_the_reviewer() {
        let harness = Harness::new();
        let component = harness
            .inspect("docs/arch/catdb-engine.md", 1, 3)
            .into_iter()
            .find(|fact| fact.name == "architecture_overlay.Component")
            .expect("proposed");

        harness
            .block_on(reject_and_retract(
                &harness.store(),
                &harness.db(),
                Uuid::parse_str(&component.fact_id).unwrap(),
                "bob",
            ))
            .expect("reject");

        let after = harness
            .inspect("docs/arch/catdb-engine.md", 1, 3)
            .into_iter()
            .find(|fact| fact.fact_id == component.fact_id)
            .expect("still inspectable");
        assert_eq!(after.state, "rejected");
        assert_eq!(after.reviewer.as_deref(), Some("bob"));
        assert!(!after.in_graph);
        // The rejection is stamped against the text it was a rejection OF, and
        // nothing has changed under it yet.
        assert_eq!(
            after.reviewed_content_hash, after.provenance.content_hash,
            "the review names the text it ruled on"
        );
        assert!(!after.source_changed_since_review);
        let projection = project_corpus_guarded(&harness.state, "architecture_overlay", 100, || {})
            .expect("project");
        assert!(projection.objects.is_empty());
    }

    /// A blank reviewer is refused here, not two layers down: the engine
    /// treats a whitespace-only `approved_by` as unapproved, so accepting it
    /// would surface as a baffling rejection from the write boundary.
    #[test]
    fn a_blank_reviewer_is_refused_before_anything_is_written() {
        let harness = Harness::new();
        let component = harness
            .inspect("docs/arch/catdb-engine.md", 1, 3)
            .into_iter()
            .find(|fact| fact.name == "architecture_overlay.Component")
            .expect("proposed");
        let error = match review_target(&harness.state, &component.fact_id, "   ") {
            Err(error) => error,
            Ok(_) => panic!("a blank reviewer must be refused"),
        };
        assert!(error.contains("reviewer"), "{error}");
        assert!(!harness
            .inspect("docs/arch/catdb-engine.md", 1, 3)
            .iter()
            .any(|fact| fact.in_graph));
    }

    /// A malformed fact id is a typed error, not a silent empty result.
    #[test]
    fn a_malformed_fact_id_is_an_error() {
        let harness = Harness::new();
        assert!(review_target(&harness.state, "not-a-uuid", "alice").is_err());
    }

    // ----------------------------------------------------------------------
    // Constraint Explorer
    // ----------------------------------------------------------------------

    /// A corpus that genuinely contradicts itself produces a violation naming
    /// the law AND the markdown span that caused it — surfaced field for
    /// field, never flattened to prose.
    #[test]
    fn check_constraints_surfaces_located_violations_with_their_spans() {
        let harness = Harness::new();
        let store = harness.store();
        let db = harness.db();

        // Clean while the overlay graph is empty.
        let clean = harness.block_on(constraint_report(&db)).expect("check");
        assert!(clean.violations.is_empty());
        assert!(clean.schemas_checked > 0);

        // Approve the whole architecture overlay, contradiction included:
        // objects first, so no edge is withheld for a missing endpoint.
        let proposals = store
            .list(&ProposalFilter {
                schema: Some("architecture_overlay".into()),
                ..ProposalFilter::default()
            })
            .expect("list");
        let (objects, morphisms): (Vec<_>, Vec<_>) = proposals
            .iter()
            .partition(|proposal| matches!(proposal.fact, StoredFact::Object(_)));
        for proposal in objects.iter().chain(morphisms.iter()) {
            harness
                .block_on(approve_and_promote(&store, &db, proposal.fact_id, "alice"))
                .expect("approve");
        }

        let report = harness.block_on(constraint_report(&db)).expect("check");
        let violation = report
            .violations
            .iter()
            .find(|violation| violation.schema == "architecture_overlay")
            .expect("the corpus contradicts the interface-ownership law");
        assert_eq!(violation.lhs_path, vec!["via", "exposes"]);
        assert_eq!(violation.rhs_path, vec!["provider"]);
        assert!(violation.law.contains('='));
        assert_eq!(violation.subject.entity, "architecture_overlay.Dependency");
        let span = violation
            .subject
            .source
            .as_ref()
            .expect("the violation is LOCATED, not just named");
        assert_eq!(span.file, "docs/arch/broken-app.md");
        assert!(span.start_line.is_some() && span.end_line.is_some());
        assert!(span.content_hash.is_some());
    }

    /// [`check_constraints`]'s body with the epoch plumbing removed, so the
    /// engine-to-DTO conversion is exercisable without a Tauri `State`.
    async fn constraint_report(db: &EmbeddedCatDb) -> Result<ConstraintReport, String> {
        let report = db.check_equations().await.map_err(|err| err.to_string())?;
        Ok(ConstraintReport {
            schemas_checked: report.schemas_checked as u32,
            violations: report.violations.into_iter().map(Into::into).collect(),
        })
    }

    // ----------------------------------------------------------------------
    // Guards
    // ----------------------------------------------------------------------

    /// Every read here is epoch-guarded: a workspace switch landing inside the
    /// call discards a previous workspace's facts rather than surfacing them
    /// under the new grant.
    #[test]
    fn a_switch_inside_the_window_discards_the_previous_workspaces_facts() {
        let harness = Harness::new();
        let other = TempDir::new().unwrap();
        let other_root = fs::canonicalize(other.path()).unwrap();
        let switch = || {
            harness.state.switch(other_root.clone()).unwrap();
        };
        assert!(list_schemas_guarded(&harness.state, switch).is_err());

        let harness = Harness::new();
        let other = TempDir::new().unwrap();
        let other_root = fs::canonicalize(other.path()).unwrap();
        assert!(project_corpus_guarded(&harness.state, "md", 10, || {
            harness.state.switch(other_root.clone()).unwrap();
        })
        .is_err());

        let harness = Harness::new();
        let other = TempDir::new().unwrap();
        let other_root = fs::canonicalize(other.path()).unwrap();
        assert!(describe_schema_guarded(&harness.state, "md", || {
            harness.state.switch(other_root.clone()).unwrap();
        })
        .is_err());

        let harness = Harness::new();
        let other = TempDir::new().unwrap();
        let other_root = fs::canonicalize(other.path()).unwrap();
        let query = SpanQuery {
            path: harness
                .root
                .join("docs/arch/catdb-engine.md")
                .to_string_lossy()
                .into_owned(),
            start_line: 1,
            end_line: 20,
        };
        assert!(inspect_span_guarded(&harness.state, &query, || {
            harness.state.switch(other_root.clone()).unwrap();
        })
        .is_err());
    }

    /// With no workspace open, every command says so rather than inventing an
    /// empty answer.
    #[test]
    fn without_a_workspace_every_surface_reports_that_rather_than_faking_it() {
        let state = WorkspaceState::default();
        assert!(list_schemas_guarded(&state, || {}).is_err());
        assert!(project_corpus_guarded(&state, "md", 10, || {}).is_err());
        assert!(describe_schema_guarded(&state, "md", || {}).is_err());
        assert!(review_target(&state, &Uuid::nil().to_string(), "alice").is_err());
    }

    /// Before an index exists the Inspector is honestly empty — the schemas
    /// are still describable (they come from the registration), but there are
    /// no facts to review yet, and no counts to invent.
    #[test]
    fn before_an_index_exists_the_inspector_is_empty_but_the_schemas_are_known() {
        let dir = TempDir::new().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let state = WorkspaceState::granted(root.clone());
        let query = SpanQuery {
            path: root.join("anything.md").to_string_lossy().into_owned(),
            start_line: 1,
            end_line: 10,
        };
        assert!(inspect_span_guarded(&state, &query, || {})
            .unwrap()
            .is_empty());
        let summaries = list_schemas_guarded(&state, || {}).expect("list");
        assert_eq!(summaries.len(), 11);
        assert!(summaries.iter().all(|s| s.object_count == 0));
        assert!(summaries.iter().all(|s| s.proposal_count == 0));
        // Nothing has been persisted, so there is no version to report — and
        // no number is invented to fill the field.
        assert!(summaries.iter().all(|s| s.version.is_none()));
        assert!(summaries.iter().all(|s| s.checksum.is_none()));
        assert!(
            summaries.iter().any(|s| s.approval_required),
            "approval flags are a registration property and are known regardless"
        );
    }
}
