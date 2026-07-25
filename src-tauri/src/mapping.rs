//! The *structural* half of the command surface: the reads behind Mapping
//! Explorer, Semantic Diff, and Migration Inspector.
//!
//! | Surface | Commands |
//! |---|---|
//! | **Mapping Explorer** | [`list_mappings`], [`inspect_mapping`], [`compose_mappings`] |
//! | **Semantic Diff** | [`diff_schema_versions`] |
//! | **Migration Inspector** | [`inspect_migration`] |
//!
//! ## Thin, like the rest of the command layer
//!
//! Every answer here comes from `catdb-core` (composition, coverage) or
//! `catdb-engine` (semantic diff, migration inspection). This module resolves
//! names, converts platform types to wire DTOs, and applies the epoch guard.
//! It computes nothing — a second implementation of "what did this mapping
//! lose" living in the app would be a second answer, and the two would drift.
//!
//! ## One source for a chain's schemas
//!
//! [`compose_mappings`] builds every leg from the **registration** — the same
//! `RegisteredMapping` values the engine validated at
//! [`open`](catdb_engine::EmbeddedCatDb::open) — rather than mixing registered
//! mappings with catalog-reconstructed schemas. Two versions of one schema name
//! are two different schemas, and composing across them silently is precisely
//! what [`CompositionError::IntermediateSchemaDisagreement`] exists to reject;
//! taking both ends of every leg from one place means the question never
//! arises. Each leg still reports the persisted version and checksum it
//! corresponds to, so the Explorer can show *which* version it is looking at.
//!
//! ## Read-only, all of it
//!
//! Nothing in this module writes. [`inspect_migration`] in particular reports
//! what a migration *would* do without performing any of it — see
//! [`catdb_engine::migration`] for the guarantee and the test that pins it.
//!
//! ## Epoch-guarded, like everything else
//!
//! Every command captures the current [`Epoch`] once and funnels its result
//! through [`Epoch::finish`], so a workspace switch mid-read discards the
//! previous workspace's answer.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use specta::Type;

use catdb_core::{
    compose_validated, coverage, Concept, ConceptImage, InformationLoss, Leg, MappedConcept,
    MappingCoverage, MappingWriteCapability, Path,
};
use catdb_engine::{
    ArrowMove, ClassificationVerdict, ConceptClassification, EggProver, EmbeddedCatDb, EntityMove,
    EntityObjectCount, FieldMove, MigrationAmbiguity, MigrationOptions, MigrationPlan,
    MigrationValidation, RegisteredMapping, SchemaChange, SchemaDiff, SchemaVersionRef,
    SigmaOutcome,
};

use crate::commands::WorkspaceState;
use crate::engine::{Epoch, StaleEpoch};

/// The bounds the desktop surface inspects under.
///
/// The chase is bounded well below the platform default: a desktop command
/// must come back, and a corpus above the bound is **declined** (reported as
/// such) rather than chased over a truncation.
fn inspection_options() -> MigrationOptions {
    MigrationOptions {
        max_objects: 20_000,
        ..MigrationOptions::default()
    }
}

// --------------------------------------------------------------------------
// Wire DTOs
// --------------------------------------------------------------------------

/// One named piece of a schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ConceptDto {
    /// `"entity"`, `"arrow"`, or `"attribute"`.
    pub kind: String,
    /// The concept's name, as the schema declares it.
    pub name: String,
}

impl From<&Concept> for ConceptDto {
    fn from(concept: &Concept) -> Self {
        Self {
            kind: concept.kind().to_string(),
            name: concept.name().to_string(),
        }
    }
}

/// A source concept and where it lands.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MappedConceptDto {
    /// The source concept.
    pub concept: ConceptDto,
    /// Its image, rendered (`Item`, `Item.label`, `Item::item_title`).
    pub image: String,
    /// Other source concepts landing on the same image. Empty means the image
    /// determines this one.
    pub shared_with: Vec<ConceptDto>,
}

/// One way the mapping fails to determine a source concept from its image.
///
/// See [`catdb_core::coverage`] for what "loss" does and does not claim: it is
/// a statement about the mapping, never a claim that a lossless mapping is
/// invertible.
#[derive(Debug, Clone, Serialize, Type)]
pub struct InformationLossDto {
    /// `"dropped"`, `"entities_merged"`, `"arrows_merged"`, or
    /// `"attributes_overwritten"`.
    pub kind: String,
    /// The shared destination image, rendered; empty for `"dropped"`.
    pub image: String,
    /// The source concepts involved.
    pub sources: Vec<String>,
    /// The kernel's own sentence about it — not re-worded here.
    pub detail: String,
}

/// What a mapping carries, what it drops, and what that costs.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MappingCoverageDto {
    /// Source schema name.
    pub src: String,
    /// Destination schema name.
    pub dst: String,
    /// Source concepts with an image.
    pub mapped: Vec<MappedConceptDto>,
    /// Source concepts with none (a validated mapping has none).
    pub unmapped: Vec<ConceptDto>,
    /// Concepts the mapping maps that the source schema does not declare.
    pub foreign: Vec<ConceptDto>,
    /// Every determination the mapping loses.
    pub loss: Vec<InformationLossDto>,
    /// Destination concepts nothing in the source reaches. **Not loss.**
    pub unreached_destination: Vec<ConceptDto>,
    /// True when the mapping maps exactly the source's concepts.
    pub total: bool,
    /// True when every source concept is determined by its image.
    pub lossless: bool,
}

/// One registered mapping, as the Explorer lists it.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MappingSummaryDto {
    /// The registered name (e.g. `F_parse`).
    pub name: String,
    /// The schema it maps from.
    pub src: String,
    /// The schema it maps to.
    pub dst: String,
    /// The persisted version ordinal; `None` before the index has been opened.
    pub version: Option<u32>,
    /// `blake3:` checksum of the persisted mapping; `None` under the same
    /// condition.
    pub checksum: Option<String>,
    /// How many source entities it maps.
    pub entity_count: u32,
    /// How many source arrows it maps.
    pub arrow_count: u32,
    /// How many source attributes it maps.
    pub attribute_count: u32,
    /// The declared write capability, as the kernel spells it.
    pub write_capability: String,
    /// Whether it maps exactly the source schema's concepts.
    pub total: bool,
    /// Whether every source concept is determined by its image.
    pub lossless: bool,
}

/// One leg of a composed chain.
#[derive(Debug, Clone, Serialize, Type)]
pub struct CompositionLegDto {
    /// The leg's registered mapping name.
    pub name: String,
    /// Source schema name.
    pub src: String,
    /// Destination schema name.
    pub dst: String,
    /// The persisted version ordinal of the mapping, when it has one.
    pub version: Option<u32>,
    /// `blake3:` checksum of the persisted mapping, when it has one.
    pub checksum: Option<String>,
}

/// Why a chain did not produce a validated composite.
#[derive(Debug, Clone, Serialize, Type)]
pub struct CompositionFailureDto {
    /// `"structure"`, `"leg"`, or `"composite"`.
    pub kind: String,
    /// Which leg, for a `"leg"` failure.
    pub leg: Option<u32>,
    /// The kernel's own message.
    pub message: String,
    /// The individual validation errors, for a `"leg"` or `"composite"` failure.
    pub errors: Vec<String>,
}

/// A composed chain: either a validated composite, or exactly why not.
///
/// Failure is part of the value rather than an error string, because "leg 1
/// does not validate" is something the Explorer must *draw* — on the leg — not
/// flash as a toast.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MappingCompositionDto {
    /// The legs, in order.
    pub legs: Vec<CompositionLegDto>,
    /// The schemas the chain passes through, source first. Present even when
    /// composition failed, so the Explorer can draw the intended chain.
    pub chain: Vec<String>,
    /// True when the composite is a validated first-class mapping.
    pub validated: bool,
    /// The composite's coverage, when there is a composite.
    pub coverage: Option<MappingCoverageDto>,
    /// Why not, when there is not.
    pub failure: Option<CompositionFailureDto>,
    /// Everything true about the composite that must not be ignored: inherited
    /// waivers, a degraded write capability.
    pub warnings: Vec<String>,
}

/// Which version a diff side refers to.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaVersionRefDto {
    /// The catalog's content-derived id (uuid string), when persisted.
    pub schema_id: Option<String>,
    /// The schema's name.
    pub name: String,
    /// The version ordinal, when persisted.
    pub version: Option<u32>,
    /// `blake3:` content checksum.
    pub checksum: String,
}

impl From<&SchemaVersionRef> for SchemaVersionRefDto {
    fn from(reference: &SchemaVersionRef) -> Self {
        Self {
            schema_id: reference.schema_id.map(|id| id.to_string()),
            name: reference.name.clone(),
            version: reference.version,
            checksum: reference.checksum.clone(),
        }
    }
}

/// One difference between two schema versions.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaChangeDto {
    /// The change class, e.g. `"entity_added"`, `"attribute_altered"`,
    /// `"equation_removed"`.
    pub class: String,
    /// `"schema"`, `"type"`, `"entity"`, `"arrow"`, `"attribute"`, or
    /// `"equation"`.
    pub kind: String,
    /// What changed, by name (or the law's rendering, for an equation).
    pub subject: String,
    /// The before/after detail, rendered.
    pub detail: String,
}

/// What becomes of one concept of the old version.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ConceptClassificationDto {
    /// The old version's concept.
    pub concept: ConceptDto,
    /// `"mapped_exactly"`, `"needs_classification"`, or `"unmappable"`.
    pub verdict: String,
    /// The change that makes it ambiguous, for `"needs_classification"`.
    pub reason: Option<String>,
}

/// What changed between two versions of a schema.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SchemaDiffDto {
    /// The older side.
    pub from: SchemaVersionRefDto,
    /// The newer side.
    pub to: SchemaVersionRefDto,
    /// True exactly when nothing changed — the same answer the content
    /// checksums give, by construction.
    pub identical: bool,
    /// Every difference.
    pub changes: Vec<SchemaChangeDto>,
    /// One entry per concept of the OLD version. Empty when `identical`.
    pub classification: Vec<ConceptClassificationDto>,
    /// Concepts only the new version declares.
    pub introduced: Vec<ConceptDto>,
}

/// Where one source entity's rows go.
#[derive(Debug, Clone, Serialize, Type)]
pub struct EntityMoveDto {
    /// The source entity.
    pub from: String,
    /// The destination entity.
    pub to: String,
    /// Other source entities landing there.
    pub shared_with: Vec<String>,
}

/// Where one source arrow goes.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ArrowMoveDto {
    /// The source arrow.
    pub from: String,
    /// The destination path, rendered (`Item.label`).
    pub to_path: String,
    /// The destination path's arrows, in application order.
    pub to_arrows: Vec<String>,
    /// Other source arrows landing on the same path.
    pub shared_with: Vec<String>,
}

/// Where one old field goes.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FieldMoveDto {
    /// The source attribute.
    pub from: String,
    /// The source entity it is observed on.
    pub from_entity: String,
    /// The destination path, rendered.
    pub to_path: String,
    /// The destination path's arrows, in application order.
    pub to_arrows: Vec<String>,
    /// The destination attribute observed at the end of the path.
    pub to_attr: String,
    /// Other source attributes landing on the same slot — their values compete.
    pub shared_with: Vec<String>,
    /// The whole move on one line.
    pub rendered: String,
}

/// How many stored objects of one source entity move without a decision.
#[derive(Debug, Clone, Serialize, Type)]
pub struct EntityObjectCountDto {
    /// The source entity.
    pub entity: String,
    /// The qualified name the corpus stores its objects under.
    pub qualified: String,
    /// How many are stored.
    pub objects: u32,
    /// How many move mechanically.
    pub auto_mapped: u32,
    /// How many need a human decision.
    pub needs_decision: u32,
    /// Why, when they do.
    pub reason: Option<String>,
}

/// A place the plan cannot be applied mechanically.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MigrationAmbiguityDto {
    /// `"entity_collision"`, `"arrow_collision"`, `"attribute_collision"`,
    /// `"unmapped"`, or `"destination_unreached"`.
    pub kind: String,
    /// The shared destination image, rendered; empty where there is none.
    pub image: String,
    /// The source concepts involved.
    pub sources: Vec<String>,
    /// Stored objects affected, for an entity collision.
    pub objects: u32,
    /// A one-line rendering.
    pub detail: String,
}

/// What Σ did, or why it did not run.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SigmaOutcomeDto {
    /// `"not_needed"`, `"not_attempted"`, `"ran"`, or `"refused"`.
    pub state: String,
    /// Why it was not attempted.
    pub reason: Option<String>,
    /// Σ's own refusal message.
    pub error: Option<String>,
    /// Which chase bound was hit (`"depth"`, `"element"`, `"round"`), when the
    /// refusal was a bound rather than a defect in the input.
    pub limit: Option<String>,
    /// Rows in the source instance the inspection derived.
    pub rows_in: Option<u32>,
    /// Rows in Σ's output.
    pub rows_out: Option<u32>,
    /// Output rows Σ invented (labelled nulls).
    pub labelled_nulls: Option<u32>,
    /// Elements the chase created.
    pub elements: Option<u32>,
    /// How deeply the labelled nulls nested.
    pub depth: Option<u32>,
    /// Merges the chase performed — irreversible identifications.
    pub merges: Option<u32>,
    /// Fixpoint rounds the chase ran.
    pub rounds: Option<u32>,
}

/// Everything applying a mapping to this corpus would do — none of it done.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MigrationPlanDto {
    /// The mapping's registered name.
    pub mapping_name: Option<String>,
    /// The source schema version.
    pub src: SchemaVersionRefDto,
    /// The destination schema version.
    pub dst: SchemaVersionRefDto,
    /// Whether the mapping validates on its own.
    pub valid: bool,
    /// Its validation errors, when it does not.
    pub validation_errors: Vec<String>,
    /// Waivers its validation leaned on.
    pub waivers: Vec<String>,
    /// Where each source entity's rows go.
    pub entity_moves: Vec<EntityMoveDto>,
    /// Where each source arrow goes.
    pub arrow_moves: Vec<ArrowMoveDto>,
    /// Where each old field goes.
    pub field_moves: Vec<FieldMoveDto>,
    /// The mapping's coverage and loss analysis.
    pub coverage: MappingCoverageDto,
    /// Per-entity object counts over the live corpus.
    pub objects: Vec<EntityObjectCountDto>,
    /// Total objects that move without a decision.
    pub auto_mapped_objects: u32,
    /// Total objects that need one.
    pub objects_needing_decision: u32,
    /// Every place the plan cannot be applied mechanically.
    pub ambiguities: Vec<MigrationAmbiguityDto>,
    /// What Σ did, or why it did not run.
    pub sigma: SigmaOutcomeDto,
    /// Everything the plan is NOT claiming. Render these verbatim.
    pub notes: Vec<String>,
}

// --------------------------------------------------------------------------
// Mapping Explorer
// --------------------------------------------------------------------------

/// Every registered mapping, with its persisted version identity and a one-
/// glance verdict on whether it is total and lossless.
#[tauri::command]
#[specta::specta]
pub fn list_mappings(
    state: tauri::State<'_, WorkspaceState>,
) -> Result<Vec<MappingSummaryDto>, String> {
    list_mappings_guarded(&state, || {})
}

fn list_mappings_guarded(
    state: &WorkspaceState,
    gate: impl FnOnce(),
) -> Result<Vec<MappingSummaryDto>, String> {
    let epoch = current(state)?;
    let db = epoch.db();
    let summaries = known_mappings(db.as_deref())
        .iter()
        .map(|known| {
            let report = coverage(
                &known.registered.mapping,
                &known.registered.src,
                &known.registered.dst,
            );
            MappingSummaryDto {
                name: known.registered.name.clone(),
                src: known.registered.mapping.src.to_string(),
                dst: known.registered.mapping.dst.to_string(),
                version: known.version,
                checksum: known.checksum.clone(),
                entity_count: known.registered.mapping.entity_map.len() as u32,
                arrow_count: known.registered.mapping.arrow_map.len() as u32,
                attribute_count: known.registered.mapping.attr_map.len() as u32,
                write_capability: render_capability(&known.registered.mapping.write_capability),
                total: report.is_total(),
                lossless: report.is_lossless(),
            }
        })
        .collect();
    gate();
    // EPOCH-GUARDED
    epoch
        .finish(state, summaries)
        .map_err(|_| StaleEpoch::message())
}

/// One mapping's full coverage: every source concept, where it lands, what it
/// shares that landing with, and what the destination is left holding.
#[tauri::command]
#[specta::specta]
pub fn inspect_mapping(
    mapping: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<MappingCoverageDto, String> {
    inspect_mapping_guarded(&state, &mapping, || {})
}

fn inspect_mapping_guarded(
    state: &WorkspaceState,
    mapping: &str,
    gate: impl FnOnce(),
) -> Result<MappingCoverageDto, String> {
    let epoch = current(state)?;
    let known = known_mappings(epoch.db().as_deref());
    let found = known
        .iter()
        .find(|known| known.registered.name == mapping)
        .ok_or_else(|| format!("no registered mapping named `{mapping}`"))?;
    let report = coverage(
        &found.registered.mapping,
        &found.registered.src,
        &found.registered.dst,
    );
    let dto = coverage_dto(&report);
    gate();
    // EPOCH-GUARDED
    epoch.finish(state, dto).map_err(|_| StaleEpoch::message())
}

/// Compose a chain of registered mappings into a single validated mapping, and
/// report the composite's coverage.
///
/// This is the claim that N×M pipelines collapse toward N+M, made concrete: the
/// result is a real [`catdb_core::ComposedMapping`], validated end to end with
/// the same equality-saturation prover that gates every registered mapping at
/// startup — not a rendering of two mappings side by side.
///
/// A chain that does not compose returns a plan-shaped answer with
/// [`MappingCompositionDto::failure`] set, so the Explorer can draw the break
/// on the leg that caused it.
#[tauri::command]
#[specta::specta]
pub fn compose_mappings(
    mappings: Vec<String>,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<MappingCompositionDto, String> {
    compose_mappings_guarded(&state, &mappings, || {})
}

fn compose_mappings_guarded(
    state: &WorkspaceState,
    names: &[String],
    gate: impl FnOnce(),
) -> Result<MappingCompositionDto, String> {
    let epoch = current(state)?;
    if names.is_empty() {
        return Err("a chain needs at least one mapping".to_string());
    }
    let known = known_mappings(epoch.db().as_deref());
    let chosen: Vec<&KnownMapping> = names
        .iter()
        .map(|name| {
            known
                .iter()
                .find(|known| &known.registered.name == name)
                .ok_or_else(|| format!("no registered mapping named `{name}`"))
        })
        .collect::<Result<_, _>>()?;

    let legs: Vec<Leg<'_>> = chosen
        .iter()
        .map(|known| {
            Leg::new(
                &known.registered.mapping,
                &known.registered.src,
                &known.registered.dst,
            )
        })
        .collect();
    let leg_dtos: Vec<CompositionLegDto> = chosen
        .iter()
        .map(|known| CompositionLegDto {
            name: known.registered.name.clone(),
            src: known.registered.mapping.src.to_string(),
            dst: known.registered.mapping.dst.to_string(),
            version: known.version,
            checksum: known.checksum.clone(),
        })
        .collect();
    // The intended chain, drawn from the legs themselves so it survives a
    // composition failure — the Explorer still draws the chain it was asked
    // for, with the break marked on the leg that caused it.
    let mut chain = vec![chosen[0].registered.src.name.to_string()];
    chain.extend(
        chosen
            .iter()
            .map(|known| known.registered.dst.name.to_string()),
    );
    let (src, dst) = (
        &chosen[0].registered.src,
        &chosen[chosen.len() - 1].registered.dst,
    );

    let dto = match compose_validated(&legs, &EggProver::default()) {
        Ok(composed) => MappingCompositionDto {
            coverage: Some(coverage_dto(&composed.coverage(src, dst))),
            chain: composed.chain().iter().map(ToString::to_string).collect(),
            validated: true,
            warnings: composed
                .warnings()
                .iter()
                .map(ToString::to_string)
                .collect(),
            legs: leg_dtos,
            failure: None,
        },
        Err(failure) => MappingCompositionDto {
            legs: leg_dtos,
            chain,
            validated: false,
            coverage: None,
            failure: Some(failure_dto(&failure)),
            warnings: Vec::new(),
        },
    };
    gate();
    // EPOCH-GUARDED
    epoch.finish(state, dto).map_err(|_| StaleEpoch::message())
}

// --------------------------------------------------------------------------
// Semantic Diff
// --------------------------------------------------------------------------

/// Diff two persisted versions of one schema, by ordinal.
///
/// The comparison short-circuits on the stored content checksums, so two
/// identical versions cost one string comparison and no parsing — and the
/// short answer can never disagree with the long one (see
/// [`catdb_engine::semantic_diff`]).
#[tauri::command]
#[specta::specta]
pub fn diff_schema_versions(
    schema: String,
    from: u32,
    to: u32,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<SchemaDiffDto, String> {
    diff_schema_versions_guarded(&state, &schema, from, to, || {})
}

fn diff_schema_versions_guarded(
    state: &WorkspaceState,
    schema: &str,
    from: u32,
    to: u32,
    gate: impl FnOnce(),
) -> Result<SchemaDiffDto, String> {
    let epoch = current(state)?;
    let db = epoch
        .db()
        .ok_or_else(|| "the index is not open yet".to_string())?;
    let diff = db
        .diff_schema_versions(schema, from, to)
        .map_err(|err| err.to_string())?;
    let dto = diff_dto(&diff);
    gate();
    // EPOCH-GUARDED
    epoch.finish(state, dto).map_err(|_| StaleEpoch::message())
}

// --------------------------------------------------------------------------
// Migration Inspector
// --------------------------------------------------------------------------

/// Report what migrating this corpus along a registered mapping *would* do —
/// without doing any of it.
///
/// Read-only: it reads objects, morphisms, and the schema catalog, derives an
/// in-memory instance, and runs Σ over that copy. The corpus and the operation
/// log are never touched (`catdb-engine`'s `tests/migration.rs` asserts the
/// bytes are unchanged).
///
/// Where the plan depends on Σ, its limits travel with it:
/// [`SigmaOutcomeDto::state`] says whether the chase ran, was declined, or
/// refused, and [`MigrationPlanDto::notes`] carries the claims the plan is
/// explicitly NOT making. Render the notes.
#[tauri::command]
#[specta::specta]
pub fn inspect_migration(
    mapping: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<MigrationPlanDto, String> {
    inspect_migration_guarded(&state, &mapping, || {})
}

fn inspect_migration_guarded(
    state: &WorkspaceState,
    mapping: &str,
    gate: impl FnOnce(),
) -> Result<MigrationPlanDto, String> {
    let epoch = current(state)?;
    let db = epoch
        .db()
        .ok_or_else(|| "the index is not open yet".to_string())?;
    let options = inspection_options();
    // Prefer the persisted mapping (that is what the corpus was written
    // against); fall back to the live registration when the catalog has not
    // stored it yet, rather than refusing to inspect a freshly-opened index.
    let plan = match db.inspect_stored_migration(mapping, &options) {
        Ok(plan) => plan,
        Err(stored_err) => {
            let registration = mdreader_ingest::registration();
            let registered = registration
                .mappings
                .iter()
                .find(|registered| registered.name == mapping)
                .ok_or_else(|| stored_err.to_string())?;
            catdb_engine::inspect_migration(
                &db,
                &registered.mapping,
                &registered.src,
                &registered.dst,
                &options,
            )
            .map_err(|err| err.to_string())?
        }
    };
    let dto = plan_dto(mapping, &plan);
    gate();
    // EPOCH-GUARDED
    epoch.finish(state, dto).map_err(|_| StaleEpoch::message())
}

// --------------------------------------------------------------------------
// Registration-derived mapping introspection
// --------------------------------------------------------------------------

/// A mapping the surfaces can describe: the registration the engine validated
/// at open, plus the version identity the catalog persisted for it.
struct KnownMapping {
    registered: RegisteredMapping,
    version: Option<u32>,
    checksum: Option<String>,
}

/// The mappings this workspace can be explored through.
///
/// The **shape** comes from the registration — deliberately, and unlike
/// [`crate::overlay::known_schemas`], which prefers the catalog. A composed
/// chain needs both ends of every leg to be the *same* schema value, and the
/// registration is the one place that guarantees it; the catalog stores a
/// mapping's body but not the schemas it was validated against, so mixing the
/// two could compose across two versions of one name without noticing.
///
/// The persisted version and checksum ride along as metadata, so the Explorer
/// can still say which stored version a leg corresponds to (and `None` says
/// honestly that the index has not been opened yet).
fn known_mappings(db: Option<&EmbeddedCatDb>) -> Vec<KnownMapping> {
    let stored = db
        .and_then(|db| db.stored_mappings().ok())
        .unwrap_or_default();
    let latest = |name: &str| {
        stored
            .iter()
            .filter(|candidate| candidate.name == name)
            .max_by_key(|candidate| candidate.version)
    };
    mdreader_ingest::registration()
        .mappings
        .into_iter()
        .map(|registered| {
            let found = latest(&registered.name);
            KnownMapping {
                version: found.map(|stored| stored.version),
                checksum: found.map(|stored| stored.checksum.clone()),
                registered,
            }
        })
        .collect()
}

// --------------------------------------------------------------------------
// Conversions
// --------------------------------------------------------------------------

fn render_capability(capability: &MappingWriteCapability) -> String {
    match capability {
        MappingWriteCapability::ReadOnly => "read-only".to_string(),
        MappingWriteCapability::FullyInvertible => "fully invertible".to_string(),
        MappingWriteCapability::PartiallyInvertible { writable_paths } => {
            format!("partially invertible ({} path(s))", writable_paths.len())
        }
        MappingWriteCapability::RequiresReconciliation => "requires reconciliation".to_string(),
    }
}

fn render_image(image: &ConceptImage) -> String {
    image.to_string()
}

fn mapped_dto(mapped: &MappedConcept) -> MappedConceptDto {
    MappedConceptDto {
        concept: (&mapped.concept).into(),
        image: render_image(&mapped.image),
        shared_with: mapped.shared_with.iter().map(Into::into).collect(),
    }
}

fn loss_dto(loss: &InformationLoss) -> InformationLossDto {
    let (kind, image, sources) = match loss {
        InformationLoss::Dropped { concept } => {
            ("dropped", String::new(), vec![concept.name().to_string()])
        }
        InformationLoss::EntitiesMerged { image, sources } => (
            "entities_merged",
            image.to_string(),
            sources.iter().map(ToString::to_string).collect(),
        ),
        InformationLoss::ArrowsMerged { image, sources } => (
            "arrows_merged",
            image.to_string(),
            sources.iter().map(ToString::to_string).collect(),
        ),
        InformationLoss::AttributesOverwritten { image, sources } => (
            "attributes_overwritten",
            format!("{}::{}", image.path, image.attr),
            sources.iter().map(ToString::to_string).collect(),
        ),
    };
    InformationLossDto {
        kind: kind.to_string(),
        image,
        sources,
        detail: loss.to_string(),
    }
}

fn coverage_dto(report: &MappingCoverage) -> MappingCoverageDto {
    MappingCoverageDto {
        src: report.src.to_string(),
        dst: report.dst.to_string(),
        mapped: report.mapped.iter().map(mapped_dto).collect(),
        unmapped: report.unmapped.iter().map(Into::into).collect(),
        foreign: report.foreign.iter().map(Into::into).collect(),
        loss: report.loss.iter().map(loss_dto).collect(),
        unreached_destination: report
            .unreached_destination
            .iter()
            .map(Into::into)
            .collect(),
        total: report.is_total(),
        lossless: report.is_lossless(),
    }
}

fn failure_dto(failure: &catdb_core::CompositionFailure) -> CompositionFailureDto {
    match failure {
        catdb_core::CompositionFailure::Structure(error) => CompositionFailureDto {
            kind: "structure".to_string(),
            leg: None,
            message: error.to_string(),
            errors: Vec::new(),
        },
        catdb_core::CompositionFailure::Leg { leg, errors } => CompositionFailureDto {
            kind: "leg".to_string(),
            leg: Some(*leg as u32),
            message: failure.to_string(),
            errors: errors.iter().map(ToString::to_string).collect(),
        },
        catdb_core::CompositionFailure::Composite(errors) => CompositionFailureDto {
            kind: "composite".to_string(),
            leg: None,
            message: failure.to_string(),
            errors: errors.iter().map(ToString::to_string).collect(),
        },
    }
}

fn change_dto(change: &SchemaChange) -> SchemaChangeDto {
    let (class, kind, subject, detail) = match change {
        SchemaChange::Renamed { from, to } => (
            "renamed",
            "schema",
            from.to_string(),
            format!("`{from}` -> `{to}`"),
        ),
        SchemaChange::TypeAdded { name, base } => (
            "type_added",
            "type",
            name.to_string(),
            format!("bound to {base:?}"),
        ),
        SchemaChange::TypeRemoved { name, base } => (
            "type_removed",
            "type",
            name.to_string(),
            format!("was bound to {base:?}"),
        ),
        SchemaChange::TypeAltered { name, from, to } => (
            "type_altered",
            "type",
            name.to_string(),
            format!("{from:?} -> {to:?}"),
        ),
        SchemaChange::EntityAdded { entity } => {
            ("entity_added", "entity", entity.to_string(), String::new())
        }
        SchemaChange::EntityRemoved { entity } => (
            "entity_removed",
            "entity",
            entity.to_string(),
            String::new(),
        ),
        SchemaChange::ArrowAdded { arrow } => (
            "arrow_added",
            "arrow",
            arrow.name.to_string(),
            format!("{} -> {}", arrow.dom, arrow.cod),
        ),
        SchemaChange::ArrowRemoved { arrow } => (
            "arrow_removed",
            "arrow",
            arrow.name.to_string(),
            format!("was {} -> {}", arrow.dom, arrow.cod),
        ),
        SchemaChange::ArrowAltered { name, from, to } => (
            "arrow_altered",
            "arrow",
            name.to_string(),
            format!(
                "{} -> {} becomes {} -> {}",
                from.dom, from.cod, to.dom, to.cod
            ),
        ),
        SchemaChange::AttributeAdded { attribute } => (
            "attribute_added",
            "attribute",
            attribute.name.to_string(),
            render_attribute(attribute),
        ),
        SchemaChange::AttributeRemoved { attribute } => (
            "attribute_removed",
            "attribute",
            attribute.name.to_string(),
            format!("was {}", render_attribute(attribute)),
        ),
        SchemaChange::AttributeAltered { name, from, to } => (
            "attribute_altered",
            "attribute",
            name.to_string(),
            format!(
                "{} becomes {}",
                render_attribute(from),
                render_attribute(to)
            ),
        ),
        SchemaChange::EquationAdded { equation } => (
            "equation_added",
            "equation",
            equation.to_string(),
            String::new(),
        ),
        SchemaChange::EquationRemoved { equation } => (
            "equation_removed",
            "equation",
            equation.to_string(),
            String::new(),
        ),
    };
    SchemaChangeDto {
        class: class.to_string(),
        kind: kind.to_string(),
        subject,
        detail,
    }
}

fn render_attribute(attribute: &catdb_core::Attribute) -> String {
    format!(
        "{}.{} : {}{}",
        attribute.dom,
        attribute.name,
        attribute.ty,
        if attribute.nullable { "?" } else { "" }
    )
}

fn classification_dto(classification: &ConceptClassification) -> ConceptClassificationDto {
    let (verdict, reason) = match &classification.verdict {
        ClassificationVerdict::MappedExactly => ("mapped_exactly", None),
        ClassificationVerdict::NeedsClassification { change } => (
            "needs_classification",
            Some({
                let rendered = change_dto(change);
                if rendered.detail.is_empty() {
                    rendered.class
                } else {
                    format!("{}: {}", rendered.class, rendered.detail)
                }
            }),
        ),
        ClassificationVerdict::Unmappable => ("unmappable", None),
    };
    ConceptClassificationDto {
        concept: (&classification.concept).into(),
        verdict: verdict.to_string(),
        reason,
    }
}

fn diff_dto(diff: &SchemaDiff) -> SchemaDiffDto {
    SchemaDiffDto {
        from: (&diff.from).into(),
        to: (&diff.to).into(),
        identical: diff.identical,
        changes: diff.changes.iter().map(change_dto).collect(),
        classification: diff.classification.iter().map(classification_dto).collect(),
        introduced: diff.introduced.iter().map(Into::into).collect(),
    }
}

fn arrows_of(path: &Path) -> Vec<String> {
    path.arrows.iter().map(ToString::to_string).collect()
}

fn entity_move_dto(entity_move: &EntityMove) -> EntityMoveDto {
    EntityMoveDto {
        from: entity_move.from.to_string(),
        to: entity_move.to.to_string(),
        shared_with: entity_move
            .shared_with
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

fn arrow_move_dto(arrow_move: &ArrowMove) -> ArrowMoveDto {
    ArrowMoveDto {
        from: arrow_move.from.to_string(),
        to_path: arrow_move.to_path.to_string(),
        to_arrows: arrows_of(&arrow_move.to_path),
        shared_with: arrow_move
            .shared_with
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

fn field_move_dto(field_move: &FieldMove) -> FieldMoveDto {
    FieldMoveDto {
        from: field_move.from.to_string(),
        from_entity: field_move.from_entity.to_string(),
        to_path: field_move.to_path.to_string(),
        to_arrows: arrows_of(&field_move.to_path),
        to_attr: field_move.to_attr.to_string(),
        shared_with: field_move
            .shared_with
            .iter()
            .map(ToString::to_string)
            .collect(),
        rendered: field_move.rendered.clone(),
    }
}

fn count_dto(count: &EntityObjectCount) -> EntityObjectCountDto {
    EntityObjectCountDto {
        entity: count.entity.to_string(),
        qualified: count.qualified.clone(),
        objects: count.objects as u32,
        auto_mapped: count.auto_mapped as u32,
        needs_decision: count.needs_decision as u32,
        reason: count.reason.clone(),
    }
}

fn ambiguity_dto(ambiguity: &MigrationAmbiguity) -> MigrationAmbiguityDto {
    let (kind, image, sources, objects, detail) = match ambiguity {
        MigrationAmbiguity::EntityCollision {
            image,
            sources,
            objects,
        } => (
            "entity_collision",
            image.to_string(),
            sources.iter().map(ToString::to_string).collect::<Vec<_>>(),
            *objects as u32,
            format!(
                "{} source entities land on `{image}`; {objects} stored object(s) affected",
                sources.len()
            ),
        ),
        MigrationAmbiguity::ArrowCollision { image, sources } => (
            "arrow_collision",
            image.to_string(),
            sources.iter().map(ToString::to_string).collect(),
            0,
            format!("{} source arrows land on `{image}`", sources.len()),
        ),
        MigrationAmbiguity::AttributeCollision { image, sources } => (
            "attribute_collision",
            format!("{}::{}", image.path, image.attr),
            sources.iter().map(ToString::to_string).collect(),
            0,
            format!(
                "{} source attributes compete for `{}::{}`",
                sources.len(),
                image.path,
                image.attr
            ),
        ),
        MigrationAmbiguity::Unmapped { concept } => (
            "unmapped",
            String::new(),
            vec![concept.name().to_string()],
            0,
            format!("{concept} has no image: its data has nowhere to go"),
        ),
        MigrationAmbiguity::DestinationUnreached { concept } => (
            "destination_unreached",
            concept.name().to_string(),
            Vec::new(),
            0,
            format!("{concept} is not reached by the mapping; Σ invents labelled nulls for it"),
        ),
    };
    MigrationAmbiguityDto {
        kind: kind.to_string(),
        image,
        sources,
        objects,
        detail,
    }
}

fn sigma_dto(outcome: &SigmaOutcome) -> SigmaOutcomeDto {
    let mut dto = SigmaOutcomeDto {
        state: String::new(),
        reason: None,
        error: None,
        limit: None,
        rows_in: None,
        rows_out: None,
        labelled_nulls: None,
        elements: None,
        depth: None,
        merges: None,
        rounds: None,
    };
    match outcome {
        SigmaOutcome::NotNeeded => dto.state = "not_needed".to_string(),
        SigmaOutcome::NotAttempted { reason } => {
            dto.state = "not_attempted".to_string();
            dto.reason = Some(reason.clone());
        }
        SigmaOutcome::Ran {
            stats,
            rows_in,
            rows_out,
            labelled_nulls,
        } => {
            dto.state = "ran".to_string();
            dto.rows_in = Some(*rows_in as u32);
            dto.rows_out = Some(*rows_out as u32);
            dto.labelled_nulls = Some(*labelled_nulls as u32);
            dto.elements = Some(stats.elements as u32);
            dto.depth = Some(stats.depth as u32);
            dto.merges = Some(stats.merges as u32);
            dto.rounds = Some(stats.rounds as u32);
        }
        SigmaOutcome::Refused { error, limit } => {
            dto.state = "refused".to_string();
            dto.error = Some(error.clone());
            dto.limit = limit.map(|limit| limit.to_string());
        }
    }
    dto
}

fn plan_dto(requested: &str, plan: &MigrationPlan) -> MigrationPlanDto {
    let (valid, validation_errors, waivers) = match &plan.validation {
        MigrationValidation::Valid { waivers } => (
            true,
            Vec::new(),
            waivers
                .iter()
                .map(|waiver| format!("{}: {}", waiver.equation, waiver.reason))
                .collect(),
        ),
        MigrationValidation::Invalid { errors } => (
            false,
            errors.iter().map(ToString::to_string).collect(),
            Vec::new(),
        ),
    };
    MigrationPlanDto {
        mapping_name: Some(
            plan.mapping_name
                .clone()
                .unwrap_or_else(|| requested.to_string()),
        ),
        src: (&plan.src).into(),
        dst: (&plan.dst).into(),
        valid,
        validation_errors,
        waivers,
        entity_moves: plan.entity_moves.iter().map(entity_move_dto).collect(),
        arrow_moves: plan.arrow_moves.iter().map(arrow_move_dto).collect(),
        field_moves: plan.field_moves.iter().map(field_move_dto).collect(),
        coverage: coverage_dto(&plan.coverage),
        objects: plan.objects.iter().map(count_dto).collect(),
        auto_mapped_objects: plan.auto_mapped_objects as u32,
        objects_needing_decision: plan.objects_needing_decision as u32,
        ambiguities: plan.ambiguities.iter().map(ambiguity_dto).collect(),
        sigma: sigma_dto(&plan.sigma),
        notes: plan.notes.clone(),
    }
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

fn current(state: &WorkspaceState) -> Result<Arc<Epoch>, String> {
    state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())
}
