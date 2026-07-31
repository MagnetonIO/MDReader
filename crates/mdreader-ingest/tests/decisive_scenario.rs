//! **THE DECISIVE TEST.** The acceptance criterion for the whole CatDB thesis:
//!
//! > The user takes **the same corpus**, applies a **new schema**, **maps it**
//! > to an existing schema, **inspects what was preserved or lost**, and
//! > **queries both interpretations** — **without duplicating the source**,
//! > with the sources **hash-verified untouched** afterwards.
//!
//! Every other test in this workspace proves a component. This one proves the
//! *claim*: schemas are non-destructive overlays over markdown that stays the
//! source of truth. If it does not run, the project's central argument is
//! unproven however many unit tests are green.
//!
//! # The scenario, and what each step actually asserts
//!
//! 1. **Index a corpus once.** Every byte of every file under the corpus root
//!    is fingerprinted first (path → length + blake3), so step 7 can compare.
//!    The import runs through the real [`EmbeddedCatDb`] against a real
//!    file-backed SQLite workspace *outside* the corpus directory.
//! 2. **Interpretation A.** `architecture_overlay` is populated the only way
//!    the engine permits: extraction PROPOSES, a named human APPROVES, and the
//!    engine's approval check lets the fact through. Each committed fact is
//!    then checked against the bytes on disk — the cited line range is re-read
//!    from the file and re-hashed, and must equal the `content_hash` the fact
//!    carries. The named components, interfaces and layers are asserted BY
//!    NAME, and one hybrid search over the same corpus is asserted to come
//!    back with a full `chunk > section > document > file @ source` chain.
//! 3. **Apply a NEW schema to the SAME corpus.** The workspace is *reopened*
//!    with `deployment_overlay` added to the registration. Nothing is
//!    re-imported: the document/section/chunk/link counts, the operation log's
//!    length, its chain anchor (`op_count` + `head_digest`), and a fingerprint
//!    of every committed overlay fact must all come back byte-identical, while
//!    `stored_schemas()` gains exactly one row and every pre-existing schema
//!    version keeps its id, ordinal and checksum.
//! 4. **Map it.** `F_deployment : deployment_overlay → architecture_overlay`
//!    goes through [`compose_validated`] with the same `EggProver` the engine
//!    itself uses, yielding a real `ValidMapping` — not a rendering trick.
//! 5. **Inspect preserved/lost.** [`catdb_sdk::core::coverage`] is asserted BY
//!    NAME: which concepts are carried, and the exact `EntitiesMerged` /
//!    `ArrowsMerged` / `AttributesOverwritten` groups, with `Dropped` absent
//!    because a validated mapping is total. `unreached_destination` is
//!    asserted by name AND asserted *not* to be reported as loss.
//! 6. **Query BOTH interpretations over the one corpus.** Interpretation A is
//!    read straight out of the graph. Interpretation B is Δ_F — the pullback
//!    of that same instance along the mapping. Δ is a *view*: the row ids in
//!    the `deployment_overlay` reading are the very same stored object ids,
//!    so the same documents surface under two different typed projections with
//!    nothing copied. Both are asserted non-empty first, then by name.
//! 7. **Re-hash every source file.** Byte-identical to step 1, no file added,
//!    none removed. The workspace database is asserted to live outside the
//!    corpus, and the number of indexed documents is asserted to equal the
//!    number of markdown files — one document per file, not two.
//!
//! # Why Δ rather than a second extraction
//!
//! The claim is *"without duplicating the source"*. A second extractor writing
//! `deployment_overlay` rows would duplicate the *derived data* even while
//! leaving the markdown alone. Δ_F duplicates nothing at all: it re-reads the
//! stored instance through the mapping, and `deployment_overlay.Service` is
//! literally the set of `architecture_overlay.Component` object ids. That the
//! two projections share row identity is the strongest available form of the
//! claim, and it is what step 6 asserts.
//!
//! # The guard on the guard
//!
//! [`the_scenarios_own_equality_guards_are_not_vacuous`] proves the three
//! load-bearing *equalities* above can actually fail: a real second ingest
//! moves the log and the anchor, a one-byte edit moves a source hash, and a
//! mapping with one image removed fails validation at the leg. Without it,
//! "unchanged" could be true because nothing ever changes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;

use catdb_sdk::core::{
    builtin, compose_validated, coverage, delta, ArrowName, AttrName, AttrTarget, Concept,
    EntityName, InformationLoss, Leg, Mapping, MemInstance, Path, RowId, Schema, SchemaName,
};
use catdb_sdk::embed::HashEmbedder;
use catdb_sdk::engine::{
    DomainRegistration, EggProver, EmbeddedCatDb, EngineConfig, ImportOptions, RegisteredMapping,
    SearchOptions,
};
use mdreader_domain::extract::ApprovalState;
use mdreader_ingest::proposals::{approve_and_promote, ProposalFilter, ProposalStore, StoredFact};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// The two interpretations
// ---------------------------------------------------------------------------

/// The interpretation the corpus is first read through — a schema that already
/// ships, already validated, already extracted into.
const SCHEMA_A: &str = "architecture_overlay";

/// The interpretation applied afterwards, to the SAME corpus.
const SCHEMA_B: &str = "deployment_overlay";

/// The registered name of the mapping between them.
const MAPPING: &str = "F_deployment";

/// The human who rules on every claim in this scenario. Never blank: the
/// engine refuses an overlay fact whose provenance carries no approver, and
/// this test must go through that door, not around it.
const REVIEWER: &str = "matthew.long";

/// A NEW schema, authored here rather than taken from the builtins — the point
/// of the step is that a user brings an interpretation the platform has never
/// seen and lays it over a corpus that is already indexed.
///
/// It reads the same architecture corpus as a deployment catalogue: units that
/// ship (`Service`) and units that are linked in (`Library`), the surfaces they
/// publish (`Endpoint`), and where they run (`Zone`). The distinction between a
/// service and a library is real *here* and has no counterpart in
/// `architecture_overlay`, which knows only `Component` — which is exactly the
/// information loss step 5 asserts.
///
/// The law says an endpoint runs where its owner runs. It is a genuine proof
/// obligation on the mapping: both sides have to translate to the same
/// `architecture_overlay` path before `F_deployment` can validate.
fn deployment_overlay_schema() -> Schema {
    let mut schema = Schema::new(SCHEMA_B, builtin::standard_typeside());
    schema.add_entity("Service");
    schema.add_entity("Library");
    schema.add_entity("Endpoint");
    schema.add_entity("Zone");
    schema.add_arrow("service_zone", "Service", "Zone");
    schema.add_arrow("library_zone", "Library", "Zone");
    schema.add_arrow("endpoint_owner", "Endpoint", "Service");
    schema.add_arrow("endpoint_zone", "Endpoint", "Zone");
    schema.add_equation(
        Path::new(
            "Endpoint",
            vec!["endpoint_owner".into(), "service_zone".into()],
        ),
        Path::new("Endpoint", vec!["endpoint_zone".into()]),
    );
    schema.add_attribute("service_name", "Service", "Text", false);
    schema.add_attribute("service_runtime", "Service", "Text", false);
    schema.add_attribute("library_name", "Library", "Text", false);
    schema.add_attribute("endpoint_name", "Endpoint", "Text", false);
    schema.add_attribute("zone_name", "Zone", "Text", false);
    schema
}

/// `F_deployment : deployment_overlay → architecture_overlay`.
///
/// Both `Service` and `Library` land on `Component`, and both their names land
/// on `component_name`: the deployment reading draws a distinction the
/// architecture reading cannot express. That is deliberate — a mapping with no
/// loss would make step 5 a formality.
fn deployment_to_architecture() -> Mapping {
    let mut mapping = Mapping::new(SCHEMA_B, SCHEMA_A);
    mapping
        .entity_map
        .insert("Service".into(), "Component".into());
    mapping
        .entity_map
        .insert("Library".into(), "Component".into());
    mapping
        .entity_map
        .insert("Endpoint".into(), "Interface".into());
    mapping.entity_map.insert("Zone".into(), "Layer".into());

    mapping.arrow_map.insert(
        "service_zone".into(),
        Path::new("Component", vec!["inLayer".into()]),
    );
    mapping.arrow_map.insert(
        "library_zone".into(),
        Path::new("Component", vec!["inLayer".into()]),
    );
    mapping.arrow_map.insert(
        "endpoint_owner".into(),
        Path::new("Interface", vec!["exposes".into()]),
    );
    mapping.arrow_map.insert(
        "endpoint_zone".into(),
        Path::new("Interface", vec!["exposes".into(), "inLayer".into()]),
    );

    mapping.attr_map.insert(
        "service_name".into(),
        AttrTarget {
            path: Path::identity("Component"),
            attr: "component_name".into(),
        },
    );
    mapping.attr_map.insert(
        "library_name".into(),
        AttrTarget {
            path: Path::identity("Component"),
            attr: "component_name".into(),
        },
    );
    mapping.attr_map.insert(
        "service_runtime".into(),
        AttrTarget {
            path: Path::identity("Component"),
            attr: "component_kind".into(),
        },
    );
    mapping.attr_map.insert(
        "endpoint_name".into(),
        AttrTarget {
            path: Path::identity("Interface"),
            attr: "interface_name".into(),
        },
    );
    mapping.attr_map.insert(
        "zone_name".into(),
        AttrTarget {
            path: Path::identity("Layer"),
            attr: "layer_name".into(),
        },
    );
    mapping
}

// ---------------------------------------------------------------------------
// The corpus, and the fingerprints that prove it was never touched
// ---------------------------------------------------------------------------

/// The shared `mdreader-domain` fixture corpus. Copied into a scratch
/// directory so this test owns every byte it hashes.
fn fixture_root() -> PathBuf {
    FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../mdreader-domain/tests/fixtures")
}

fn copy_tree(source: &FilePath, destination: &FilePath) {
    std::fs::create_dir_all(destination).expect("create destination");
    for entry in std::fs::read_dir(source).expect("readable source") {
        let entry = entry.expect("dir entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// EVERY file under `root` (not only markdown — a stray artifact written into
/// the corpus must show up as an added key), keyed by forward-slash relative
/// path, valued by `(byte length, blake3)`.
fn corpus_fingerprint(root: &FilePath) -> BTreeMap<String, (u64, String)> {
    fn walk(root: &FilePath, dir: &FilePath, out: &mut BTreeMap<String, (u64, String)>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("readable directory")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let bytes = std::fs::read(&path).expect("readable file");
                out.insert(
                    path.strip_prefix(root)
                        .expect("under root")
                        .to_string_lossy()
                        .replace('\\', "/"),
                    (
                        bytes.len() as u64,
                        format!("blake3:{}", blake3::hash(&bytes).to_hex()),
                    ),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Everything about the indexed workspace that a re-ingest would move.
///
/// The counts alone are not enough: a re-import of an unchanged corpus leaves
/// the counts identical while appending to the operation log and advancing the
/// chain anchor. Carrying the log length AND the anchor is what makes "the
/// corpus was not re-ingested" a real assertion rather than a coincidence —
/// see [`the_scenarios_own_equality_guards_are_not_vacuous`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceFingerprint {
    documents: u64,
    sections: u64,
    chunks: u64,
    links: u64,
    log_len: usize,
    anchor_op_count: u64,
    anchor_head_digest: String,
    /// One line per committed overlay fact, sorted: the whole of what the
    /// interpretation holds, provenance included.
    overlay: Vec<String>,
}

fn workspace_fingerprint(
    engine: &EmbeddedCatDb,
    runtime: &tokio::runtime::Runtime,
) -> WorkspaceFingerprint {
    let status = runtime.block_on(engine.status()).expect("status");
    let (_, anchor) = engine
        .store()
        .operation_log_snapshot()
        .expect("operation log snapshot");
    WorkspaceFingerprint {
        documents: status.documents,
        sections: status.sections,
        chunks: status.chunks,
        links: status.links,
        log_len: engine.operation_log_len().expect("log length"),
        anchor_op_count: anchor.op_count,
        anchor_head_digest: anchor.head_digest,
        overlay: overlay_fingerprint(engine),
    }
}

/// Every committed fact of both overlay interpretations, rendered so a
/// difference is readable in the failure message.
fn overlay_fingerprint(engine: &EmbeddedCatDb) -> Vec<String> {
    let mut out = Vec::new();
    for schema in [SCHEMA_A, SCHEMA_B, "decision_overlay", "document_overlay"] {
        let prefix = format!("{schema}.");
        for header in engine
            .store()
            .objects_with_entity_prefix(&prefix)
            .expect("read objects")
        {
            let provenance = &header.provenance;
            out.push(format!(
                "object {} {} {:?} {:?} {:?} {:?}",
                header.entity,
                header.id,
                provenance.source,
                provenance.range,
                provenance.content_hash,
                provenance.approved_by,
            ));
        }
        for edge in engine
            .store()
            .morphisms_with_arrow_prefix(&prefix)
            .expect("read morphisms")
        {
            out.push(format!(
                "morphism {} {} -> {}",
                edge.arrow, edge.source, edge.target
            ));
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Opening the workspace
// ---------------------------------------------------------------------------

/// The MDReader domain, optionally with the NEW schema and its mapping added.
///
/// This is exactly how a user applies a new interpretation: the registration
/// gains a schema, the workspace is reopened, and the engine validates and
/// persists it. No import is involved, which is the whole point.
fn registration(store: &Arc<ProposalStore>, with_new_schema: bool) -> DomainRegistration {
    let mut registration = mdreader_ingest::registration_with_proposals(Arc::clone(store));
    if with_new_schema {
        let schema = deployment_overlay_schema();
        registration
            .approval_required_schemas
            .push(schema.name.clone());
        registration.mappings.push(RegisteredMapping {
            name: MAPPING.to_string(),
            mapping: deployment_to_architecture(),
            src: schema.clone(),
            dst: builtin::architecture_overlay_schema(),
        });
        registration.schemas.push(schema);
    }
    registration
}

fn open(db: &FilePath, store: &Arc<ProposalStore>, with_new_schema: bool) -> EmbeddedCatDb {
    EmbeddedCatDb::open(
        EngineConfig::new(db, Arc::new(HashEmbedder::new()))
            .with_domain(registration(store, with_new_schema))
            .with_actor(REVIEWER),
    )
    .expect("the engine must open against the registered domain")
}

/// `(name, version, checksum, id)` for every persisted schema version — the
/// catalogue's own account of itself.
fn stored_schema_rows(engine: &EmbeddedCatDb) -> Vec<(String, u32, String, Uuid)> {
    let mut rows: Vec<(String, u32, String, Uuid)> = engine
        .stored_schemas()
        .expect("read the persisted schema catalogue")
        .into_iter()
        .map(|stored| {
            (
                stored.name,
                stored.version,
                stored.checksum,
                stored.schema_id,
            )
        })
        .collect();
    rows.sort();
    rows
}

// ---------------------------------------------------------------------------
// Lifting the stored graph into a kernel instance
// ---------------------------------------------------------------------------

/// The committed graph of `schema`, as a kernel instance.
///
/// A pure read: object ids become row ids verbatim, so the instance and the
/// stored graph share identity. That is what lets step 6 prove the two
/// interpretations are readings of the same rows rather than two copies.
fn instance_of(engine: &EmbeddedCatDb, schema: &Schema) -> MemInstance {
    let mut instance = MemInstance::new(schema.name.clone());
    for entity in &schema.entities {
        instance.rows.entry(entity.clone()).or_default();
        let qualified = format!("{}.{}", schema.name, entity);
        for (id, value) in engine
            .store()
            .objects_with_entity(&qualified)
            .expect("read objects")
        {
            let row = RowId::new(id.to_string());
            instance.add_row(entity.clone(), row.clone());
            for (name, attribute) in &schema.attributes {
                if attribute.dom != *entity {
                    continue;
                }
                if let Some(found) = value.get(name.as_str()) {
                    instance.set_attr(name.clone(), row.clone(), found.clone());
                }
            }
        }
    }
    let edges = engine
        .store()
        .morphisms_with_arrow_prefix(&format!("{}.", schema.name))
        .expect("read morphisms");
    for arrow in schema.arrows.keys() {
        let qualified = format!("{}.{}", schema.name, arrow);
        for edge in edges.iter().filter(|edge| edge.arrow == qualified) {
            instance.set_fk(
                arrow.clone(),
                RowId::new(edge.source.to_string()),
                RowId::new(edge.target.to_string()),
            );
        }
    }
    instance
}

/// The string values of one attribute column, sorted.
fn values(instance: &MemInstance, attr: &str) -> Vec<String> {
    let mut out: Vec<String> = instance
        .attrs
        .get(&AttrName::from(attr))
        .map(|column| {
            column
                .values()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The row set of one entity.
fn rows_of(instance: &MemInstance, entity: &str) -> BTreeSet<RowId> {
    instance
        .rows
        .get(&EntityName::from(entity))
        .cloned()
        .unwrap_or_default()
}

/// `attribute value at the source row → attribute value at the target row`,
/// for one arrow — a relation read in names rather than uuids.
fn named_edges(
    instance: &MemInstance,
    arrow: &str,
    from_attr: &str,
    to_attr: &str,
) -> BTreeSet<(String, String)> {
    let name_of = |attr: &str, row: &RowId| -> String {
        instance
            .attrs
            .get(&AttrName::from(attr))
            .and_then(|column| column.get(row))
            .and_then(|value| value.as_str())
            .unwrap_or("<unnamed>")
            .to_string()
    };
    instance
        .fks
        .get(&ArrowName::from(arrow))
        .map(|fks| {
            fks.iter()
                .map(|(from, to)| (name_of(from_attr, from), name_of(to_attr, to)))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// THE SCENARIO
// ---------------------------------------------------------------------------

#[test]
fn one_corpus_two_interpretations_mapped_inspected_and_queried_without_touching_the_source() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let corpus = scratch.path().join("corpus");
    copy_tree(&fixture_root(), &corpus);
    // The workspace lives OUTSIDE the corpus. Everything CatDB writes lands
    // here; the corpus directory is only ever read.
    let workspace = scratch.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    let db = workspace.join("catdb.db");
    let proposals_path = workspace.join("proposals.db");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    // === STEP 1 — index the corpus ONCE, fingerprinting it first ===========
    let before_hashes = corpus_fingerprint(&corpus);
    assert_eq!(
        before_hashes.len(),
        7,
        "the fixture corpus is seven files; got {:?}",
        before_hashes.keys().collect::<Vec<_>>()
    );
    let markdown_files = before_hashes
        .keys()
        .filter(|path| path.ends_with(".md"))
        .count() as u64;

    let store = Arc::new(ProposalStore::open(&proposals_path).expect("open proposal store"));
    let engine = open(&db, &store, false);
    let report = runtime
        .block_on(engine.import(&corpus, ImportOptions::default()))
        .expect("import");
    assert_eq!(
        report.files_scanned, markdown_files,
        "every markdown file in the corpus was scanned"
    );
    let status = runtime.block_on(engine.status()).expect("status");
    assert_eq!(
        status.documents, markdown_files,
        "ONE document per file — indexing must not duplicate the source"
    );
    assert!(status.chunks > 0, "the corpus must really be indexed");

    // === STEP 2 — interpretation A, through the approval door ==============
    // Extraction proposes; nothing is in the graph yet.
    let proposed = store
        .list(&ProposalFilter {
            schema: Some(SCHEMA_A.into()),
            ..ProposalFilter::default()
        })
        .expect("list proposals");
    assert!(
        !proposed.is_empty(),
        "the corpus must have produced {SCHEMA_A} proposals to review"
    );
    assert!(
        proposed
            .iter()
            .all(|proposal| proposal.state() == ApprovalState::Proposed),
        "extraction proposes; it never approves"
    );
    assert!(
        instance_of(&engine, &builtin::architecture_overlay_schema())
            .rows
            .values()
            .all(BTreeSet::is_empty),
        "the interpretation is empty until a human rules on it"
    );

    // A named human approves every claim. Objects first, so that each edge's
    // endpoints are already committed when the edge itself is approved.
    let (mut objects, mut morphisms) = (Vec::new(), Vec::new());
    for proposal in &proposed {
        match &proposal.fact {
            StoredFact::Object(_) => objects.push(proposal.fact_id),
            StoredFact::Morphism(_) => morphisms.push(proposal.fact_id),
        }
    }
    for fact_id in objects.iter().chain(morphisms.iter()) {
        runtime
            .block_on(approve_and_promote(&store, &engine, *fact_id, REVIEWER))
            .expect("a named reviewer may approve a proposed fact");
    }

    let schema_a = builtin::architecture_overlay_schema();
    let first_reading = instance_of(&engine, &schema_a);

    // Real hits, asserted BY NAME rather than by count.
    assert_eq!(
        values(&first_reading, "component_name"),
        vec!["broken-app", "catdb-core", "catdb-engine", "mdreader-app"],
        "interpretation A must read the corpus as these components"
    );
    assert_eq!(
        values(&first_reading, "interface_name"),
        vec!["Path Prover", "Schema Validation", "Search API"],
        "…exposing these interfaces"
    );
    assert_eq!(
        values(&first_reading, "layer_name"),
        vec!["app", "platform"],
        "…in these layers"
    );

    // Provenance, checked against the BYTES ON DISK rather than against the
    // extractor that produced it: re-read the cited lines out of the file and
    // re-hash them. A fact citing a range it does not cover fails here.
    let committed = engine
        .store()
        .objects_with_entity_prefix(&format!("{SCHEMA_A}."))
        .expect("read objects");
    assert!(!committed.is_empty(), "interpretation A must be populated");
    for header in &committed {
        let provenance = &header.provenance;
        let file = provenance
            .source
            .as_deref()
            .unwrap_or_else(|| panic!("{} carries no source file", header.entity));
        let path = corpus.join(file);
        assert!(
            path.is_file(),
            "{} cites {file}, which is not a file in the corpus",
            header.entity
        );
        let (start, end) = {
            let range = provenance
                .range
                .unwrap_or_else(|| panic!("{} carries no line range", header.entity));
            (range[0] as usize, range[1] as usize)
        };
        assert!(
            start >= 1 && end >= start,
            "{} cites a broken range {start}-{end}",
            header.entity
        );
        let text = std::fs::read_to_string(&path).expect("readable");
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            end <= lines.len(),
            "{} cites {file}:{start}-{end}, past the end of a {}-line file",
            header.entity,
            lines.len()
        );
        let cited = lines[start - 1..end].join("\n");
        assert_eq!(
            provenance.content_hash.as_deref(),
            Some(format!("blake3:{}", blake3::hash(cited.as_bytes()).to_hex()).as_str()),
            "{} cites {file}:{start}-{end} but hashed something else",
            header.entity
        );
        assert_eq!(
            provenance.approved_by.as_deref(),
            Some(REVIEWER),
            "every committed fact carries the human who let it in"
        );
    }

    // The corpus is queryable as a corpus too, with the full chain.
    let hits = runtime
        .block_on(engine.search(
            "hybrid retrieval",
            SearchOptions {
                explain: true,
                ..SearchOptions::default()
            },
        ))
        .expect("search");
    assert!(!hits.is_empty(), "the indexed corpus must answer a query");
    let explain = hits[0]
        .explain
        .as_ref()
        .expect("--explain must attach a breakdown");
    let chain = &explain.provenance;
    assert_ne!(chain.chunk_id, Uuid::nil());
    assert_ne!(chain.section_id, Uuid::nil());
    assert_ne!(chain.document_id, Uuid::nil());
    assert!(
        corpus.join(&chain.file_path).is_file(),
        "the chain must terminate at a real file in the corpus, got {:?}",
        chain.file_path
    );
    assert_eq!(
        chain.source_root.as_deref(),
        Some(
            corpus
                .canonicalize()
                .expect("canonical corpus")
                .to_str()
                .expect("utf-8 path")
        ),
        "…under the one source root that was imported"
    );

    // === STEP 3 — apply the NEW schema to the SAME corpus ==================
    let before_workspace = workspace_fingerprint(&engine, &runtime);
    let before_schemas = stored_schema_rows(&engine);
    assert!(
        before_schemas.iter().any(|(name, ..)| name == SCHEMA_A),
        "interpretation A's schema must already be persisted: {before_schemas:?}"
    );
    assert!(
        !before_schemas.iter().any(|(name, ..)| name == SCHEMA_B),
        "the new schema must not exist yet: {before_schemas:?}"
    );
    drop(engine);

    // Reopen the SAME database with the new schema in the registration. No
    // import. No second workspace. No copy of the corpus.
    let engine = open(&db, &store, true);

    let after_schemas = stored_schema_rows(&engine);
    let new_rows: Vec<_> = after_schemas
        .iter()
        .filter(|row| !before_schemas.contains(row))
        .collect();
    assert_eq!(
        new_rows.len(),
        1,
        "applying one schema must add exactly one catalogue version, got {new_rows:?}"
    );
    assert_eq!(new_rows[0].0, SCHEMA_B, "…and it must be the new schema");
    assert_eq!(new_rows[0].1, 1, "…at its first version");
    assert!(
        new_rows[0].2.starts_with("blake3:"),
        "…content-addressed: {:?}",
        new_rows[0].2
    );
    for row in &before_schemas {
        assert!(
            after_schemas.contains(row),
            "applying a new schema must not churn an existing one: {row:?} is gone"
        );
    }
    assert_eq!(
        engine.schema_versions(SCHEMA_A).expect("versions").len(),
        1,
        "{SCHEMA_A} must still be at exactly one version — nothing about it changed"
    );
    let round_tripped = engine
        .latest_schema_version(SCHEMA_B)
        .expect("versions")
        .expect("the new schema is persisted")
        .schema()
        .expect("the persisted spec round-trips");
    assert_eq!(
        round_tripped,
        deployment_overlay_schema(),
        "the catalogue must hold the new schema losslessly"
    );

    // THE non-duplication assertion: nothing about the indexed corpus moved.
    let after_workspace = workspace_fingerprint(&engine, &runtime);
    assert_eq!(
        after_workspace, before_workspace,
        "applying a schema must not re-ingest the corpus: the counts, the \
         operation log, its chain anchor, and every committed fact must be \
         byte-identical"
    );
    engine
        .verify_operation_log()
        .expect("the operation log must still verify against its anchor");

    // === STEP 4 — map the new schema to the existing one ===================
    let schema_b = deployment_overlay_schema();
    let mapping = deployment_to_architecture();
    let composed = compose_validated(
        &[Leg::new(&mapping, &schema_b, &schema_a)],
        &EggProver::default(),
    )
    .unwrap_or_else(|failure| panic!("{MAPPING} must validate against the two schemas: {failure}"));
    assert_eq!(
        composed.chain().to_vec(),
        vec![SchemaName::from(SCHEMA_B), SchemaName::from(SCHEMA_A)],
        "the validated chain runs from the new schema to the existing one"
    );
    assert!(
        composed.valid().waivers().is_empty(),
        "the mapping is proven outright, not waived: {:?}",
        composed.valid().waivers()
    );
    assert_eq!(
        composed.mapping(),
        &mapping,
        "the witness must be for the mapping we wrote"
    );
    // And the engine agrees: it validated and persisted the same mapping at
    // open, or it would not have opened at all.
    let stored_mapping = engine
        .latest_mapping_version(MAPPING)
        .expect("mapping versions")
        .expect("the mapping is persisted alongside the schema");
    assert_eq!(
        stored_mapping.mapping().expect("round-trip"),
        mapping,
        "the persisted mapping must be the one that was validated"
    );

    // === STEP 5 — inspect what was preserved and what was lost =============
    let report = composed.coverage(&schema_b, &schema_a);
    assert!(
        report.is_total(),
        "a validated mapping is total: {:?} unmapped, {:?} foreign",
        report.unmapped,
        report.foreign
    );
    assert!(
        !report
            .loss
            .iter()
            .any(|loss| matches!(loss, InformationLoss::Dropped { .. })),
        "totality means nothing is dropped: {:?}",
        report.loss
    );

    // PRESERVED, by name: the image determines the concept.
    let carried: BTreeSet<(String, String)> = report
        .mapped
        .iter()
        .filter(|mapped| mapped.shared_with.is_empty())
        .map(|mapped| (mapped.concept.name().to_string(), mapped.image.to_string()))
        .collect();
    for (concept, image) in [
        ("Endpoint", "Interface"),
        ("Zone", "Layer"),
        ("endpoint_owner", "Interface.exposes"),
        ("endpoint_zone", "Interface.exposes.inLayer"),
        ("endpoint_name", "Interface::interface_name"),
        ("zone_name", "Layer::layer_name"),
        ("service_runtime", "Component::component_kind"),
    ] {
        assert!(
            carried.contains(&(concept.to_string(), image.to_string())),
            "`{concept}` must be carried to `{image}`; carried set: {carried:?}"
        );
    }

    // LOST, by name: exactly these three collapses, and no others.
    assert_eq!(
        report.loss,
        vec![
            InformationLoss::EntitiesMerged {
                image: "Component".into(),
                sources: vec!["Library".into(), "Service".into()],
            },
            InformationLoss::ArrowsMerged {
                image: Path::new("Component", vec!["inLayer".into()]),
                sources: vec!["library_zone".into(), "service_zone".into()],
            },
            InformationLoss::AttributesOverwritten {
                image: AttrTarget {
                    path: Path::identity("Component"),
                    attr: "component_name".into(),
                },
                sources: vec!["library_name".into(), "service_name".into()],
            },
        ],
        "the coverage report must name what the translation cannot keep"
    );
    assert!(!report.is_lossless());

    // NOT loss: destination structure the source does not fill.
    assert_eq!(
        report.unreached_destination,
        vec![
            Concept::Entity("Dependency".into()),
            Concept::Arrow("dependant".into()),
            Concept::Arrow("provider".into()),
            Concept::Arrow("via".into()),
            Concept::Attribute("dependency_kind".into()),
        ],
        "the parts of {SCHEMA_A} the deployment reading never reaches"
    );
    for unreached in &report.unreached_destination {
        assert!(
            !report.loss.iter().any(|loss| match loss {
                InformationLoss::Dropped { concept } => concept == unreached,
                _ => false,
            }),
            "{unreached} is unreached destination structure, NOT loss"
        );
    }

    // === STEP 6 — query BOTH interpretations over the ONE corpus ===========
    // Both readings are taken from the REOPENED workspace, so what is compared
    // is what the workspace holds now, after the new schema was applied.
    let interpretation_a = instance_of(&engine, &schema_a);
    assert_eq!(
        interpretation_a, first_reading,
        "applying a schema must not have disturbed interpretation A"
    );
    let interpretation_b = delta(composed.valid(), &interpretation_a)
        .expect("Δ_F must read the stored instance through the new schema");

    let components = rows_of(&interpretation_a, "Component");
    let services = rows_of(&interpretation_b, "Service");
    let libraries = rows_of(&interpretation_b, "Library");
    assert!(
        !components.is_empty(),
        "interpretation A must return results before any comparison means anything"
    );
    assert!(
        !services.is_empty(),
        "interpretation B must return results too"
    );

    // THE non-duplication proof, at the end of the chain: the deployment
    // reading's rows ARE the architecture reading's rows — same stored object
    // ids, nothing copied.
    assert_eq!(
        services, components,
        "`Service` must be a READING of the stored components, not a copy"
    );
    assert_eq!(
        libraries, components,
        "…and so must `Library` — which is the EntitiesMerged loss, realized"
    );
    assert_eq!(
        rows_of(&interpretation_b, "Endpoint"),
        rows_of(&interpretation_a, "Interface"),
        "`Endpoint` is a reading of the stored interfaces"
    );
    assert_eq!(
        rows_of(&interpretation_b, "Zone"),
        rows_of(&interpretation_a, "Layer"),
        "`Zone` is a reading of the stored layers"
    );

    // Results appropriate to ITS schema, by name.
    assert_eq!(
        values(&interpretation_b, "service_name"),
        vec!["broken-app", "catdb-core", "catdb-engine", "mdreader-app"],
        "the deployment reading names the same units"
    );
    assert_eq!(
        values(&interpretation_b, "library_name"),
        values(&interpretation_b, "service_name"),
        "both columns carry the same values — the AttributesOverwritten loss, realized"
    );
    assert_eq!(
        values(&interpretation_b, "endpoint_name"),
        vec!["Path Prover", "Schema Validation", "Search API"],
    );
    assert_eq!(
        values(&interpretation_b, "zone_name"),
        vec!["app", "platform"],
    );
    assert_eq!(
        named_edges(
            &interpretation_b,
            "service_zone",
            "service_name",
            "zone_name"
        ),
        BTreeSet::from([
            ("broken-app".to_string(), "app".to_string()),
            ("catdb-core".to_string(), "platform".to_string()),
            ("catdb-engine".to_string(), "platform".to_string()),
            ("mdreader-app".to_string(), "app".to_string()),
        ]),
        "every unit runs in the zone its markdown declares"
    );
    assert_eq!(
        named_edges(
            &interpretation_b,
            "endpoint_owner",
            "endpoint_name",
            "service_name"
        ),
        BTreeSet::from([
            ("Path Prover".to_string(), "catdb-core".to_string()),
            ("Schema Validation".to_string(), "catdb-core".to_string()),
            ("Search API".to_string(), "catdb-engine".to_string()),
        ]),
        "every endpoint is owned by the unit whose document publishes it"
    );
    // The law the new schema imposes really holds over the corpus read
    // through it — an endpoint runs where its owner runs.
    assert!(
        interpretation_b.check_equations(&schema_b).is_empty(),
        "{SCHEMA_B}'s law must hold over the corpus: {:?}",
        interpretation_b.check_equations(&schema_b)
    );

    // The same DOCUMENTS surface under both readings.
    let provenance_of: BTreeMap<String, String> = engine
        .store()
        .objects_with_entity_prefix(&format!("{SCHEMA_A}."))
        .expect("read objects")
        .iter()
        .filter(|header| header.entity == format!("{SCHEMA_A}.Component"))
        .map(|header| {
            (
                header.id.to_string(),
                header.provenance.source.clone().unwrap_or_default(),
            )
        })
        .collect();
    let documents_behind_services: BTreeSet<&str> = services
        .iter()
        .map(|row| {
            provenance_of
                .get(row.as_str())
                .unwrap_or_else(|| panic!("`Service` row {row} is not a stored component"))
                .as_str()
        })
        .collect();
    assert_eq!(
        documents_behind_services,
        BTreeSet::from([
            "docs/arch/broken-app.md",
            "docs/arch/catdb-core.md",
            "docs/arch/catdb-engine.md",
            "docs/arch/mdreader-app.md",
        ]),
        "the deployment reading resolves to the very markdown the architecture \
         reading came from"
    );

    // === STEP 7 — the sources were never touched ==========================
    let after_hashes = corpus_fingerprint(&corpus);
    assert_eq!(
        after_hashes, before_hashes,
        "every source file must be byte-identical, and none added or removed"
    );
    assert!(
        !db.starts_with(&corpus),
        "the workspace database must not live inside the corpus"
    );
    assert!(
        !proposals_path.starts_with(&corpus),
        "neither must the proposal store"
    );
    let final_status = runtime.block_on(engine.status()).expect("status");
    assert_eq!(
        final_status.documents, markdown_files,
        "still one document per file — two interpretations, one corpus"
    );
}

// ---------------------------------------------------------------------------
// The guard on the guard
// ---------------------------------------------------------------------------

/// The scenario above leans on three EQUALITIES. An equality that can never
/// fail proves nothing, so each one is shown to be breakable here:
///
/// 1. a genuine second ingest DOES move the operation log and its anchor, so
///    "unchanged" in step 3 is a real observation;
/// 2. a one-byte edit DOES change a corpus fingerprint, so step 7 would catch
///    a source the product had written to;
/// 3. a mapping with one image removed DOES fail to validate at the leg, so
///    step 4's `ValidMapping` is earned rather than assumed.
#[test]
fn the_scenarios_own_equality_guards_are_not_vacuous() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let corpus = scratch.path().join("corpus");
    copy_tree(&fixture_root(), &corpus);
    let db = scratch.path().join("workspace/catdb.db");
    let proposals_path = scratch.path().join("workspace/proposals.db");
    std::fs::create_dir_all(scratch.path().join("workspace")).expect("workspace dir");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    // (1) A REAL re-ingest moves what step 3 asserts stands still.
    let store = Arc::new(ProposalStore::open(&proposals_path).expect("open proposal store"));
    let engine = open(&db, &store, false);
    runtime
        .block_on(engine.import(&corpus, ImportOptions::default()))
        .expect("import");
    let first = workspace_fingerprint(&engine, &runtime);
    // Change one file so the second import genuinely re-ingests it.
    let touched = corpus.join("docs/arch/catdb-engine.md");
    let text = std::fs::read_to_string(&touched).expect("read");
    std::fs::write(&touched, format!("{text}\nA new paragraph.\n")).expect("write");
    runtime
        .block_on(engine.import(&corpus, ImportOptions::default()))
        .expect("re-import");
    let second = workspace_fingerprint(&engine, &runtime);
    assert_ne!(
        first.log_len, second.log_len,
        "a real re-ingest must append to the operation log"
    );
    assert_ne!(
        first.anchor_head_digest, second.anchor_head_digest,
        "…and move the chain anchor — which is what makes step 3's equality \
         a real observation rather than a tautology"
    );

    // (2) A one-byte edit moves a corpus fingerprint.
    let before = corpus_fingerprint(&corpus);
    let victim = corpus.join("docs/adr/0003-storage.md");
    let text = std::fs::read_to_string(&victim).expect("read");
    std::fs::write(&victim, format!("{text} ")).expect("write");
    let after = corpus_fingerprint(&corpus);
    assert_ne!(
        before, after,
        "step 7's comparison must notice a single appended byte"
    );
    // …and an ADDED file, which is how a second copy of the source would look.
    std::fs::write(corpus.join("copy.md"), "# a duplicate\n").expect("write");
    assert!(
        corpus_fingerprint(&corpus).len() > after.len(),
        "step 7's comparison must notice a file appearing in the corpus"
    );

    // (3) A mapping missing one image does not validate.
    let schema_b = deployment_overlay_schema();
    let schema_a = builtin::architecture_overlay_schema();
    let mut broken = deployment_to_architecture();
    broken.attr_map.remove(&"zone_name".into());
    let failure = compose_validated(
        &[Leg::new(&broken, &schema_b, &schema_a)],
        &EggProver::default(),
    )
    .expect_err("a mapping that is not total must not yield a ValidMapping");
    let rendered = failure.to_string();
    assert!(
        rendered.contains("leg 0 does not validate"),
        "the failure must name the leg: {rendered}"
    );
    assert!(
        coverage(&broken, &schema_b, &schema_a)
            .loss
            .contains(&InformationLoss::Dropped {
                concept: Concept::Attribute("zone_name".into()),
            }),
        "…and coverage must report the dropped concept by name"
    );
}
