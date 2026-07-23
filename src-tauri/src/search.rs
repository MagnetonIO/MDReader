//! The *search* half of the command surface: hybrid search (streamed in two
//! honest phases), related-document lookup, and the explain payloads the
//! palette surfaces.
//!
//! ## Two-phase streaming (honest, not decorative)
//!
//! `catdb-engine`'s [`search`](catdb_engine::EmbeddedCatDb::search) is a single
//! atomic call — it does not itself stream. But the CLI's two-phase *reality*
//! (a lexical batch, then a semantic merge) is real work we can mirror without
//! faking it: [`search`] runs the engine **twice** — first `lexical_only` (the
//! FTS arm alone, no query embedding), streamed as the `"lexical"` batch, then
//! the full hybrid pass (FTS + vectors, RRF-fused, boosted), streamed as the
//! `"final"` batch and returned. Each phase is genuine search output, so the
//! palette can render lexical hits instantly and reorder as the semantic pass
//! lands — no synthetic delay, no fake partials. An `Exact` search skips the
//! second pass (lexical *is* final).
//!
//! ## Provenance ids threaded through (reconciliation)
//!
//! Every serde-defaulted [`SearchHit`](catdb_engine::SearchHit) field —
//! `rrf_score`, `boosts`, `explain`, and the provenance chain's ids — is carried
//! onto the wire DTO so the bindings expose them to TS (the palette's score
//! badge, explain popover, and provenance breadcrumb read them). Uuids cross as
//! strings (specta has no native Uuid). Each hit also carries the **absolute**
//! path (source root + stored path) the reader opens, and the file's mtime so
//! the `Recent` intent can sort by recency.
//!
//! ## Epoch-guarded, like everything else
//!
//! Both commands capture the current [`Epoch`](crate::engine::Epoch) once and
//! funnel their result through [`Epoch::finish`](crate::engine::Epoch::finish):
//! a workspace switch mid-search discards the (previous-workspace) hits rather
//! than surfacing them under the new grant. A filter parse/validation error
//! from the typed query language propagates as the command error (the schema-
//! derived text the palette shows inline).

use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::ipc::Channel;
use tauri::State;

use catdb_engine::{
    EmbeddedCatDb, HitBoosts, HitExplain, ProvenanceChain, RelatedComponents, RelatedDoc,
    SearchHit, SearchOptions,
};

use crate::commands::{mtime_millis, WorkspaceState};
use crate::engine::Epoch;

// --------------------------------------------------------------------------
// Wire DTOs
// --------------------------------------------------------------------------

/// Options for [`search`], mirrored from the engine's [`SearchOptions`] with
/// the fields the palette drives.
#[derive(Debug, Clone, Deserialize, Type)]
pub struct SearchQuery {
    /// Maximum hits per phase.
    pub limit: u32,
    /// Skip the vector arm entirely (the `Exact` intent) — one phase only.
    pub lexical_only: bool,
    /// Attach the full [`HitExplain`] breakdown to every hit.
    pub explain: bool,
}

impl SearchQuery {
    /// Engine options for a given `lexical_only` override (the two-phase driver
    /// forces the lexical arm on the first pass).
    fn engine_options(&self, lexical_only: bool) -> SearchOptions {
        SearchOptions {
            limit: self.limit,
            lexical_only,
            candidates: SearchOptions::default().candidates,
            explain: self.explain,
        }
    }
}

/// The recorded contribution of each ranking boost to a hit's score.
#[derive(Debug, Clone, Copy, Serialize, Type)]
pub struct BoostsDto {
    /// Title match (query terms in the heading breadcrumb's head).
    pub title: f32,
    /// Path match (query terms in the stored path).
    pub path: f32,
    /// Recency (modification-time decay).
    pub recency: f32,
    /// Graph proximity to a `linked-to:` document.
    pub graph: f32,
}

impl From<HitBoosts> for BoostsDto {
    fn from(b: HitBoosts) -> Self {
        Self {
            title: b.title,
            path: b.path,
            recency: b.recency,
            graph: b.graph,
        }
    }
}

/// The chunk → section → document → file → source provenance ids (uuids as
/// strings), present when `explain` was requested.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProvenanceDto {
    /// The matched chunk id.
    pub chunk_id: String,
    /// The chunk's section id.
    pub section_id: String,
    /// The section's document id.
    pub document_id: String,
    /// Source-relative stored path.
    pub file_path: String,
    /// The attributing source's id, if any.
    pub source_id: Option<String>,
    /// That source's root directory, if any.
    pub source_root: Option<String>,
}

impl From<ProvenanceChain> for ProvenanceDto {
    fn from(p: ProvenanceChain) -> Self {
        Self {
            chunk_id: p.chunk_id.to_string(),
            section_id: p.section_id.to_string(),
            document_id: p.document_id.to_string(),
            file_path: p.file_path,
            source_id: p.source_id.map(|id| id.to_string()),
            source_root: p.source_root,
        }
    }
}

/// The full per-hit score breakdown (see [`SearchQuery::explain`]).
#[derive(Debug, Clone, Serialize, Type)]
pub struct ExplainDto {
    /// 1-based lexical rank, when the chunk appeared in the FTS results.
    pub lex_rank: Option<u32>,
    /// 1-based semantic rank, when the chunk appeared in the vector results.
    pub sem_rank: Option<u32>,
    /// The fused reciprocal-rank score before boosts.
    pub rrf_score: f32,
    /// Each boost's recorded contribution.
    pub boosts: BoostsDto,
    /// `rrf_score + boosts.total()` — the ordering key.
    pub final_score: f32,
    /// The index version the semantic modality searched, if it ran.
    pub index_version: Option<String>,
    /// That version's embedding model.
    pub embedding_model: Option<String>,
    /// The provenance chain.
    pub provenance: ProvenanceDto,
}

impl From<HitExplain> for ExplainDto {
    fn from(e: HitExplain) -> Self {
        Self {
            lex_rank: e.lex_rank,
            sem_rank: e.sem_rank,
            rrf_score: e.rrf_score,
            boosts: e.boosts.into(),
            final_score: e.final_score,
            index_version: e.index_version.map(|id| id.to_string()),
            embedding_model: e.embedding_model,
            provenance: e.provenance.into(),
        }
    }
}

/// One hybrid-search hit on the wire: provenance (path, heading breadcrumb,
/// line range), per-modality ranks, boost contributions, an optional explain
/// breakdown, plus the absolute path the reader opens and the file mtime the
/// `Recent` intent sorts on.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchHitDto {
    /// The matched chunk id (uuid string).
    pub chunk_id: String,
    /// The chunk's document id (uuid string).
    pub document_id: String,
    /// Source-relative stored path of the document.
    pub file_path: String,
    /// Absolute path the reader opens (source root + stored path).
    pub abs_path: String,
    /// The document's file name (basename).
    pub name: String,
    /// Heading breadcrumb of the chunk's section.
    pub heading_path: Vec<String>,
    /// First source line of the section (1-based, inclusive) — the jump target.
    pub start_line: u32,
    /// Last source line of the section (1-based, inclusive).
    pub end_line: u32,
    /// Final score: fused RRF plus boosts (higher is better).
    pub score: f32,
    /// The fused RRF score before boosts.
    pub rrf_score: f32,
    /// Recorded boost contributions.
    pub boosts: BoostsDto,
    /// 1-based rank in the lexical result list, if the chunk appeared there.
    pub lex_rank: Option<u32>,
    /// 1-based rank in the semantic result list, if the chunk appeared there.
    pub sem_rank: Option<u32>,
    /// Preview snippet (FTS-highlighted when the hit is lexical).
    pub snippet: String,
    /// Modification time of the document file, ms since the Unix epoch.
    pub mtime: f64,
    /// The full breakdown, present when `explain` was requested.
    pub explain: Option<ExplainDto>,
}

/// A streamed batch: the `"lexical"` first pass, then the `"final"` hybrid pass.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchBatch {
    /// `"lexical"` (FTS-only first pass) or `"final"` (hybrid).
    pub phase: String,
    /// The hits for this phase, best-first.
    pub hits: Vec<SearchHitDto>,
    /// The workspace epoch generation this batch was produced under. `finish`
    /// releases the state lock before the send below, so a switch could still
    /// interleave one batch onto the channel — the frontend discards any batch
    /// whose generation is not the current one (belt+suspenders over the lock).
    pub generation: u32,
}

/// Per-component contributions behind a related score.
#[derive(Debug, Clone, Copy, Serialize, Type)]
pub struct RelatedComponentsDto {
    /// Link/span-graph walk (depth-decayed, ≤ 2 hops).
    pub graph: f32,
    /// Shared concepts (Jaccard over the two concept sets).
    pub concept: f32,
    /// Document-centroid vector similarity.
    pub vector: f32,
}

impl From<RelatedComponents> for RelatedComponentsDto {
    fn from(c: RelatedComponents) -> Self {
        Self {
            graph: c.graph,
            concept: c.concept,
            vector: c.vector,
        }
    }
}

/// A related document on the wire: its path (relative + absolute), name, the
/// unified score, and the per-component decomposition the rail tooltips show.
#[derive(Debug, Clone, Serialize, Type)]
pub struct RelatedDocDto {
    /// The related document's id (uuid string).
    pub document_id: String,
    /// Source-relative stored path.
    pub file_path: String,
    /// Absolute path the reader opens.
    pub abs_path: String,
    /// The document's file name (basename).
    pub name: String,
    /// Combined score (graph + concept + vector).
    pub score: f32,
    /// Per-component contributions.
    pub components: RelatedComponentsDto,
}

// --------------------------------------------------------------------------
// Conversions
// --------------------------------------------------------------------------

/// The final path component (with extension).
fn basename(rel: &str) -> String {
    rel.rsplit(['/', '\\']).next().unwrap_or(rel).to_string()
}

/// Build a wire hit from an engine [`SearchHit`], reconstructing the absolute
/// path under `src_root` and statting the file for its mtime.
fn hit_dto(src_root: &Path, hit: SearchHit) -> SearchHitDto {
    let abs = src_root.join(&hit.file_path);
    let mtime = mtime_millis(std::fs::metadata(&abs).ok().and_then(|m| m.modified().ok()));
    SearchHitDto {
        chunk_id: hit.chunk_id.to_string(),
        document_id: hit.document_id.to_string(),
        name: basename(&hit.file_path),
        abs_path: abs.to_string_lossy().into_owned(),
        file_path: hit.file_path,
        heading_path: hit.heading_path,
        start_line: hit.start_line,
        end_line: hit.end_line,
        score: hit.score,
        rrf_score: hit.rrf_score,
        boosts: hit.boosts.into(),
        lex_rank: hit.lex_rank,
        sem_rank: hit.sem_rank,
        snippet: hit.snippet,
        mtime,
        explain: hit.explain.map(Into::into),
    }
}

fn related_dto(src_root: &Path, doc: RelatedDoc) -> RelatedDocDto {
    let abs = src_root.join(&doc.file_path);
    RelatedDocDto {
        document_id: doc.document_id.to_string(),
        name: basename(&doc.file_path),
        abs_path: abs.to_string_lossy().into_owned(),
        file_path: doc.file_path,
        score: doc.score,
        components: doc.components.into(),
    }
}

// --------------------------------------------------------------------------
// search
// --------------------------------------------------------------------------

/// Hybrid search over the workspace index, streamed in two honest phases on
/// `on_event` and returned as the final (hybrid) hit list.
///
/// The typed filter language is honored as-is (`tag:` `path:` `linked-to:`
/// `after:` bare terms, RRF-fused): a parse/validation failure returns the
/// schema-derived error text as the command error (the palette shows it inline
/// under the input). Phase one is `lexical_only` (streamed as `"lexical"`);
/// phase two is the full hybrid pass (streamed as `"final"` and returned).
/// `options.lexical_only` (the `Exact` intent) skips the second pass. Before
/// the engine is installed (no import yet) both phases are empty.
///
/// Epoch-guarded: a workspace switch mid-search supersedes the captured epoch,
/// discarding its hits rather than surfacing a previous workspace's documents.
#[tauri::command]
#[specta::specta]
pub async fn search(
    query: String,
    options: SearchQuery,
    on_event: Channel<SearchBatch>,
    workspace: State<'_, WorkspaceState>,
) -> Result<Vec<SearchHitDto>, String> {
    // Capture the epoch ONCE; root + engine travel together.
    let epoch = workspace
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;

    let Some(db) = epoch.db() else {
        // No engine yet (import not finished): honestly empty (guarded).
        // EPOCH-GUARDED
        return Ok(epoch.finish(&workspace, Vec::new()).unwrap_or_default());
    };

    let src_root = epoch.root.clone();

    // Phase 1 — the lexical (FTS) arm alone. A typed-filter error surfaces here
    // (same parse both phases), so we never stream a partial for a bad query.
    let lexical = run_phase(&db, &src_root, &query, options.engine_options(true)).await?;
    stream(&epoch, &workspace, &on_event, "lexical", &lexical);

    // Phase 2 — the full hybrid pass (unless the caller asked lexical-only).
    let final_hits = if options.lexical_only {
        lexical
    } else {
        run_phase(&db, &src_root, &query, options.engine_options(false)).await?
    };
    stream(&epoch, &workspace, &on_event, "final", &final_hits);

    // EPOCH-GUARDED: a switch mid-search means these hits belong to a previous
    // workspace — return an empty list (silently-clean), never the stale hits
    // and never a spurious error the palette would show as a filter failure. The
    // generation-tagged batches above are the streaming counterpart of this.
    Ok(epoch.finish(&workspace, final_hits).unwrap_or_default())
}

/// Run one engine search pass and lift its hits onto the wire DTO.
async fn run_phase(
    db: &EmbeddedCatDb,
    src_root: &Path,
    query: &str,
    options: SearchOptions,
) -> Result<Vec<SearchHitDto>, String> {
    let hits = db.search(query, options).await.map_err(|e| e.to_string())?;
    Ok(hits.into_iter().map(|h| hit_dto(src_root, h)).collect())
}

/// Stream one phase's batch while `epoch` is still current (a closed channel is
/// ignored — the resolved return is authoritative for a mocked IPC). Every batch
/// is tagged with `epoch.generation` so the frontend can discard one that still
/// slips through after the `finish` lock is released (the send is not under the
/// lock — see [`SearchBatch::generation`]).
fn stream(
    epoch: &std::sync::Arc<Epoch>,
    workspace: &WorkspaceState,
    channel: &Channel<SearchBatch>,
    phase: &str,
    hits: &[SearchHitDto],
) {
    let generation = epoch.generation as u32;
    // EPOCH-GUARDED: only stream while this epoch is the current era.
    if epoch.finish(workspace, ()).is_ok() {
        let _ = channel.send(SearchBatch {
            phase: phase.to_string(),
            hits: hits.to_vec(),
            generation,
        });
    }
}

// --------------------------------------------------------------------------
// related
// --------------------------------------------------------------------------

/// Documents related to `doc_path` (an absolute path in the granted
/// workspace), unified graph + concept + vector scoring with each component
/// recorded for the rail's decomposition tooltips.
///
/// The absolute path is mapped back to its source-relative stored form and
/// resolved against the index; an unknown document (or an index not yet built)
/// yields an empty list rather than an error, so the rail simply shows nothing
/// until the import lands. Epoch-guarded like [`search`].
#[tauri::command]
#[specta::specta]
pub async fn related(
    doc_path: String,
    limit: u32,
    workspace: State<'_, WorkspaceState>,
) -> Result<Vec<RelatedDocDto>, String> {
    let epoch = workspace
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;

    let Some(db) = epoch.db() else {
        // EPOCH-GUARDED
        return Ok(epoch.finish(&workspace, Vec::new()).unwrap_or_default());
    };

    let src_root = epoch.root.clone();
    // Map the absolute path back to the source-relative stored path.
    let rel = Path::new(&doc_path)
        .strip_prefix(&src_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| doc_path.clone());

    let related = match db
        .store()
        .document_by_path(&rel)
        .map_err(|e| format!("resolve document: {e}"))?
    {
        Some(doc) => db
            .related(doc.id, limit)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|r| related_dto(&src_root, r))
            .collect(),
        // Unknown document (e.g. not yet indexed): empty, not an error.
        None => Vec::new(),
    };

    // EPOCH-GUARDED: a switch mid-lookup yields an empty rail (silently-clean),
    // never a previous workspace's related list nor a spurious error.
    Ok(epoch.finish(&workspace, related).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    use catdb_embed::{Embedder, HashEmbedder};
    use catdb_engine::{EngineConfig, ImportOptions};
    use mdreader_ingest::registration;

    fn workspace_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    /// A real in-memory engine over a hash embedder (no model download), with
    /// the MDReader domain registered.
    fn engine() -> Arc<EmbeddedCatDb> {
        let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
        let mut config = EngineConfig::in_memory(embedder).with_actor("search-test");
        config.domains = vec![registration()];
        Arc::new(EmbeddedCatDb::open(config).expect("open in-memory engine"))
    }

    #[test]
    fn basename_takes_the_final_component() {
        assert_eq!(basename("notes/deep/page.md"), "page.md");
        assert_eq!(basename("page.md"), "page.md");
    }

    /// hit_dto reconstructs the absolute path under the source root and carries
    /// every provenance / score field through (reconciliation 2).
    #[test]
    fn hit_dto_reconstructs_absolute_path_and_carries_fields() {
        let (_dir, root) = workspace_dir();
        let file = root.join("notes/page.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "# Page\n").unwrap();

        let hit = SearchHit {
            chunk_id: uuid::Uuid::nil(),
            document_id: uuid::Uuid::nil(),
            file_path: "notes/page.md".to_string(),
            heading_path: vec!["Page".to_string()],
            start_line: 3,
            end_line: 9,
            score: 1.25,
            rrf_score: 0.9,
            boosts: HitBoosts {
                title: 0.1,
                ..HitBoosts::default()
            },
            lex_rank: Some(1),
            sem_rank: None,
            snippet: "hi".to_string(),
            explain: None,
        };
        let dto = hit_dto(&root, hit);
        assert_eq!(dto.abs_path, file.to_string_lossy());
        assert_eq!(dto.name, "page.md");
        assert_eq!(dto.start_line, 3);
        assert_eq!(dto.rrf_score, 0.9);
        assert_eq!(dto.boosts.title, 0.1);
        assert!(dto.mtime > 0.0, "existing file has a real mtime");
    }

    /// End-to-end (real engine + ingest): a two-phase search over a tiny corpus
    /// returns hits whose provenance line range and reconstructed absolute path
    /// are the reader's jump target.
    #[test]
    fn search_over_a_real_corpus_returns_provenance_bearing_hits() {
        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        // The fixture corpus (shared, character-for-character, with the reader's
        // provenance golden test in `@mdreader/ui`).
        const FIXTURE: &str =
            "# Guide\n\nIntro.\n\n## Storage engine\n\nThe storage engine persists chunks.\n";
        fs::write(root.join("guide.md"), FIXTURE).unwrap();

        // The section's true source line, derived from the corpus (never
        // hardcoded): the `## Storage engine` heading's 1-based line.
        let heading_line = FIXTURE
            .lines()
            .position(|line| line.trim() == "## Storage engine")
            .map(|index| index as u32 + 1)
            .expect("heading present in the fixture");

        let db = engine();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(db.import(&root, ImportOptions::default()))
            .expect("import corpus");

        let opts = SearchQuery {
            limit: 10,
            lexical_only: false,
            explain: true,
        };
        let hits = rt
            .block_on(run_phase(
                &db,
                &root,
                "storage engine",
                opts.engine_options(false),
            ))
            .expect("search");
        let hit = hits
            .iter()
            .find(|h| h.file_path == "guide.md")
            .expect("guide.md is a hit");
        // THE contract the reader's golden test consumes: a real hit's
        // `start_line` is the section's heading line, and the range is well
        // formed (start ≤ end). The reader then lands on the block whose
        // `data-line` range contains this very line.
        assert_eq!(
            hit.start_line, heading_line,
            "hit start_line is the section's source line"
        );
        assert!(hit.end_line >= hit.start_line);
        assert_eq!(hit.abs_path, root.join("guide.md").to_string_lossy());
        assert!(hit.explain.is_some(), "explain requested");

        // Generate the committed golden the reader's TS golden test reads: ONE
        // fixture and ONE real `start_line` cross the Rust→TS boundary here. If
        // the chunker changes the section's start line, THIS test rewrites the
        // JSON and the TS side follows (drift caught by check-bindings' git
        // diff). Writing (rather than asserting) makes the Rust side the single
        // source of truth for the value.
        let golden = serde_json::json!({
            "doc": "guide.md",
            "fixture": FIXTURE,
            "startLine": hit.start_line,
        });
        let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ui/src/goldens/provenance.json");
        std::fs::create_dir_all(golden_path.parent().unwrap()).unwrap();
        std::fs::write(
            &golden_path,
            format!("{}\n", serde_json::to_string_pretty(&golden).unwrap()),
        )
        .unwrap();
    }

    /// A typed-filter parse error surfaces as the command error (the schema-
    /// derived text the palette shows inline), not a panic or empty result.
    #[test]
    fn a_bad_filter_field_is_a_typed_error() {
        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        fs::write(root.join("a.md"), "# A\n\nBody.\n").unwrap();

        let db = engine();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(db.import(&root, ImportOptions::default()))
            .expect("import");

        let opts = SearchQuery {
            limit: 10,
            lexical_only: false,
            explain: false,
        };
        let err = rt
            .block_on(run_phase(
                &db,
                &root,
                "bogusfield:value",
                opts.engine_options(true),
            ))
            .expect_err("unknown filter field is rejected");
        assert!(!err.is_empty(), "carries a schema-derived message");
    }

    /// related maps an absolute path back to the stored path and returns the
    /// unified decomposition; an unknown document yields an empty list.
    #[test]
    fn related_decomposes_and_tolerates_unknown_docs() {
        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        fs::write(root.join("a.md"), "# A\n\nStart [next](b.md).\n").unwrap();
        fs::write(root.join("b.md"), "# B\n\nMiddle note.\n").unwrap();

        let db = engine();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(db.import(&root, ImportOptions::default()))
            .expect("import");

        let doc = db.store().document_by_path("a.md").unwrap().unwrap();
        let related = rt.block_on(db.related(doc.id, 10)).unwrap();
        let dtos: Vec<RelatedDocDto> = related.into_iter().map(|r| related_dto(&root, r)).collect();
        let b = dtos
            .iter()
            .find(|r| r.file_path == "b.md")
            .expect("b.md is related via the a -> b span edge");
        assert!(b.components.graph > 0.0, "graph component recorded");
        assert_eq!(b.abs_path, root.join("b.md").to_string_lossy());
    }
}
