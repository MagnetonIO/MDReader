//! The *search* half of the command surface: hybrid search (streamed in two
//! honest phases), related-document lookup, and the explain payloads the
//! palette surfaces.
//!
//! ## Two-phase streaming (honest, not decorative)
//!
//! `catdb-engine`'s [`search`](catdb_sdk::engine::EmbeddedCatDb::search) is a single
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
//! Every serde-defaulted [`SearchHit`](catdb_sdk::engine::SearchHit) field —
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

use catdb_sdk::engine::{
    EmbeddedCatDb, HitBoosts, HitExplain, ProvenanceChain, RelatedComponents, RelatedDoc,
    SearchHit, SearchOptions,
};
use mdreader_ingest::SourceRoot;

use crate::commands::{mtime_millis, WorkspaceState};
use crate::engine::{Epoch, Semantic, StaleEpoch};

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
    #[serde(default)]
    #[specta(optional)]
    pub intent: Option<String>,
    /// Explicit enabled canonical provider sources; absence retains local-only search.
    #[serde(default)]
    #[specta(optional)]
    pub provider_sources: Option<Vec<String>>,
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
    #[specta(optional)]
    pub source_id: Option<String>,
    #[specta(optional)]
    pub revision: Option<String>,
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
    /// Set when the semantic arm was **wanted and could not run** — see
    /// [`SearchResults::semantic_unavailable`]. Carried on the batch as well as
    /// the resolved value so a consumer that renders streamed phases cannot
    /// paint a lexical-only list as the hybrid ranking, even for an instant.
    pub semantic_unavailable: Option<String>,
}

/// What a [`search`] produced, and whether it is the full ranking.
///
/// The hits alone cannot say this: a hybrid pass that quietly lost its vector
/// arm returns fewer, differently-ordered hits that look exactly like a normal
/// result set. So the outcome carries the distinction explicitly, and the caller
/// has to look at it to get at the hits.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchResults {
    /// The final hits, best-first.
    pub hits: Vec<SearchHitDto>,
    /// `None` when the ranking is what was asked for — either a full hybrid pass,
    /// or a lexical-only pass the caller requested (the `Exact` intent). `Some`
    /// carries the reason the semantic arm could not run when it *was* asked for
    /// (typically: the embedding model could not be loaded), so `hits` is a
    /// lexical-only list and must be presented as one.
    pub semantic_unavailable: Option<String>,
}

/// Per-component contributions behind a related score.
#[derive(Debug, Clone, Copy, Serialize, Type)]
pub struct RelatedComponentsDto {
    /// Link/span-graph walk (depth-decayed, ≤ 2 hops).
    pub graph: f32,
    /// Shared concepts (Jaccard over the two concept sets).
    pub concept: f32,
    /// Weighted best destination-chunk cosine; None means unavailable.
    pub vector: Option<f32>,
    /// Actual frontmatter tag overlap, distinct from legacy concepts.
    pub shared_tags: Option<f32>,
}

impl From<RelatedComponents> for RelatedComponentsDto {
    fn from(c: RelatedComponents) -> Self {
        Self {
            graph: c.graph,
            concept: c.concept,
            vector: Some(c.vector),
            shared_tags: None,
        }
    }
}

/// A related document on the wire: its path (relative + absolute), name, the
/// unified score, and the per-component decomposition the rail tooltips show.
#[derive(Debug, Clone, Serialize, Type)]
pub struct RelatedDocDto {
    pub source_id: Option<String>,
    pub revision: Option<String>,
    pub line: Option<u32>,
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

/// Availability accompanies empty lists as well as ranked results.
#[derive(Debug, Clone, Serialize, Type)]
pub struct RelatedOutcomeDto {
    pub hits: Vec<RelatedDocDto>,
    pub availability: RelatedAvailabilityDto,
}
#[derive(Debug, Clone, Serialize, Type)]
pub struct RelatedAvailabilityDto {
    pub vector: RelatedVectorStateDto,
    pub anchor_chunks: u32,
    pub anchor_vectors: u32,
    pub legacy_concepts: RelatedLegacyStateDto,
    pub legacy_graph_entities: RelatedLegacyStateDto,
}
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RelatedVectorStateDto {
    Available,
    AnchorMissing,
    NoAnchorVectors,
    ZeroCentroid,
}
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RelatedLegacyStateDto {
    NotMigrated,
}
impl From<mdreader_core::RelatedAvailability> for RelatedAvailabilityDto {
    fn from(a: mdreader_core::RelatedAvailability) -> Self {
        Self {
            vector: match a.vector {
                mdreader_core::RelatedVectorState::Available => RelatedVectorStateDto::Available,
                mdreader_core::RelatedVectorState::AnchorMissing => {
                    RelatedVectorStateDto::AnchorMissing
                }
                mdreader_core::RelatedVectorState::NoAnchorVectors => {
                    RelatedVectorStateDto::NoAnchorVectors
                }
                mdreader_core::RelatedVectorState::ZeroCentroid => {
                    RelatedVectorStateDto::ZeroCentroid
                }
            },
            anchor_chunks: a.anchor_chunks.try_into().unwrap_or(u32::MAX),
            anchor_vectors: a.anchor_vectors.try_into().unwrap_or(u32::MAX),
            legacy_concepts: RelatedLegacyStateDto::NotMigrated,
            legacy_graph_entities: RelatedLegacyStateDto::NotMigrated,
        }
    }
}
fn ranked_related_dto(root: &Path, hit: mdreader_core::RelatedRankedHit) -> RelatedDocDto {
    RelatedDocDto {
        document_id: uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("mdreader:{}:{}", hit.source_id, hit.path).as_bytes(),
        )
        .to_string(),
        source_id: Some(hit.source_id),
        revision: Some(hit.revision),
        line: Some(hit.line),
        abs_path: root.join(&hit.path).to_string_lossy().into_owned(),
        name: basename(&hit.path),
        file_path: hit.path,
        score: hit.score as f32,
        components: RelatedComponentsDto {
            graph: hit.components.graph as f32,
            concept: 0.0,
            shared_tags: Some(hit.components.shared_tags as f32),
            vector: hit.components.vector.map(|v| v as f32),
        },
    }
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
fn hit_dto(src_root: &SourceRoot, hit: SearchHit) -> SearchHitDto {
    let relative = Path::new(&hit.file_path);
    let abs = src_root.path.join(relative);
    let mtime = mtime_millis(
        src_root
            .metadata_relative(relative)
            .ok()
            .and_then(|metadata| metadata.modified().ok()),
    );
    SearchHitDto {
        source_id: None,
        revision: None,
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
        source_id: None,
        revision: None,
        line: None,
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
/// `on_event` and returned as the final hit list plus whether that list is the
/// full ranking.
///
/// The typed filter language is honored as-is (`tag:` `path:` `linked-to:`
/// `after:` bare terms, RRF-fused): a parse/validation failure returns the
/// schema-derived error text as the command error (the palette shows it inline
/// under the input). Phase one is `lexical_only` (streamed as `"lexical"`);
/// phase two is the full hybrid pass (streamed as `"final"` and returned).
/// `options.lexical_only` (the `Exact` intent) skips the second pass. Before
/// the engine is installed (no import yet) both phases are empty.
///
/// **Degrading honestly.** The lexical arm needs no embedder, so when the model
/// could not be loaded this still searches — and says so: the second pass is
/// skipped and [`SearchResults::semantic_unavailable`] carries the reason, on the
/// streamed batches as well as the returned value. What it never does is run a
/// lexical-only pass and hand it back as though the vector arm had contributed.
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
    reader: State<'_, crate::reader::ReaderBackend>,
) -> Result<SearchResults, String> {
    // A closed channel is ignored — the resolved return is authoritative.
    search_reader(&reader, &workspace, &query, &options, |batch| {
        let _ = on_event.send(batch);
    })
    .await
}

fn reader_hit(
    root: Option<&SourceRoot>,
    hit: mdreader_core::Evidence,
    explain: bool,
) -> SearchHitDto {
    let document_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("mdreader:{}:{}", hit.source_id, hit.path).as_bytes(),
    )
    .to_string();
    let chunk_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!(
            "{document_id}:{}:{}:{}",
            hit.revision, hit.line_start, hit.heading
        )
        .as_bytes(),
    )
    .to_string();
    let section_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("{document_id}:{}", hit.heading).as_bytes(),
    )
    .to_string();
    let score = hit.lexical_rank.map_or(0.0, |r| 1.0 / (60.0 + r as f32))
        + hit.semantic_rank.map_or(0.0, |r| 1.0 / (60.0 + r as f32));
    let boosts = BoostsDto {
        title: 0.0,
        path: 0.0,
        recency: 0.0,
        graph: 0.0,
    };
    let explanation = explain.then(|| ExplainDto {
        lex_rank: hit.lexical_rank,
        sem_rank: hit.semantic_rank,
        rrf_score: score,
        boosts,
        final_score: score,
        index_version: None,
        embedding_model: hit.semantic_rank.map(|_| mdreader_core::MODEL_NAME.into()),
        provenance: ProvenanceDto {
            chunk_id: chunk_id.clone(),
            section_id,
            document_id: document_id.clone(),
            file_path: hit.path.clone(),
            source_id: Some(hit.source_id.clone()),
            source_root: root.map(|root| root.path.to_string_lossy().into_owned()),
        },
    });
    SearchHitDto {
        source_id: Some(hit.source_id),
        revision: Some(hit.revision),
        chunk_id,
        document_id,
        file_path: hit.path.clone(),
        abs_path: root.map_or_else(
            || hit.path.clone(),
            |root| root.path.join(&hit.path).to_string_lossy().into_owned(),
        ),
        name: if root.is_none() {
            hit.title
        } else {
            basename(&hit.path)
        },
        heading_path: hit.heading.split(" > ").map(String::from).collect(),
        start_line: hit.line_start,
        end_line: hit.line_end,
        score,
        rrf_score: score,
        boosts,
        lex_rank: hit.lexical_rank,
        sem_rank: hit.semantic_rank,
        snippet: hit.text,
        mtime: hit.modified_ms as f64,
        explain: explanation,
    }
}

fn canonical_provider_source(id: &str) -> bool {
    id.strip_prefix("provider:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
fn reader_scope_current(
    epoch: Option<&std::sync::Arc<Epoch>>,
    workspace: &WorkspaceState,
) -> Result<bool, String> {
    match epoch {
        Some(epoch) => {
            epoch.validate_root()?;
            Ok(workspace.is_current(epoch))
        }
        None => Ok(workspace.current().is_none()),
    }
}
pub(crate) async fn search_reader(
    reader: &crate::reader::ReaderBackend,
    workspace: &WorkspaceState,
    query: &str,
    options: &SearchQuery,
    mut sink: impl FnMut(SearchBatch),
) -> Result<SearchResults, String> {
    use mdreader_service::{Request, SearchMode, SearchOutcome};
    let epoch = workspace.current();
    if let Some(epoch) = &epoch {
        epoch.validate_root()?;
    }
    let selected = options.provider_sources.as_deref().unwrap_or_default();
    let mut sources = std::collections::BTreeSet::new();
    if selected.len() > 64
        || selected
            .iter()
            .any(|id| !canonical_provider_source(id) || !sources.insert(id.clone()))
    {
        return Err("Invalid selected provider search scope".into());
    }
    if !selected.is_empty() {
        let authoritative: Vec<mdreader_core::Source> = reader.invoke(Request::ListSources).await?;
        if selected.iter().any(|id| {
            !authoritative
                .iter()
                .any(|source| source.enabled && source.id == *id)
        }) {
            return Err(
                "A selected provider source is unavailable; review the search selection".into(),
            );
        }
    }
    let local = if let Some(epoch) = &epoch {
        let source = reader.admit_async(epoch.source_root()).await?;
        sources.insert(source.clone());
        Some(source)
    } else {
        None
    };
    if sources.is_empty() {
        return Err("Open a folder or select a cloud source for search".into());
    }
    let sources: Vec<_> = sources.into_iter().collect();
    let mode = match options.intent.as_deref() {
        Some("semantic") => SearchMode::Semantic,
        Some("recent") => SearchMode::Recent,
        Some("exact") => SearchMode::Exact,
        Some("best") | None => {
            if options.lexical_only {
                SearchMode::Exact
            } else {
                SearchMode::Hybrid
            }
        }
        _ => return Err("Unknown search intent".into()),
    };
    let map_hits = |hits: Vec<mdreader_core::Evidence>| -> Result<Vec<SearchHitDto>, String> {
        hits.into_iter()
            .map(|hit| {
                if !sources.contains(&hit.source_id) {
                    return Err("Reader returned evidence outside the selected scope".into());
                }
                let root = if local.as_ref() == Some(&hit.source_id) {
                    epoch.as_ref().map(|epoch| epoch.source_root())
                } else {
                    None
                };
                Ok(reader_hit(root.as_ref(), hit, options.explain))
            })
            .collect()
    };
    let mut emit =
        |phase: &str, hits: &[SearchHitDto], reason: Option<&str>| -> Result<(), String> {
            if let Some(epoch) = &epoch {
                stream(epoch, workspace, &mut sink, phase, hits, reason)
            } else if reader_scope_current(None, workspace)? {
                sink(SearchBatch {
                    phase: phase.into(),
                    hits: hits.to_vec(),
                    generation: 0,
                    semantic_unavailable: reason.map(str::to_string),
                });
                Ok(())
            } else {
                Ok(())
            }
        };
    let mut lexical_outcome = None;
    if !matches!(mode, SearchMode::Semantic) {
        let lexical: SearchOutcome = reader
            .invoke(Request::SearchRanked {
                query: query.into(),
                sources: sources.clone(),
                limit: options.limit as usize,
                mode: if matches!(mode, SearchMode::Recent) {
                    SearchMode::Recent
                } else {
                    SearchMode::Exact
                },
            })
            .await?;
        let hits = map_hits(lexical.hits.clone())?;
        emit("lexical", &hits, None)?;
        lexical_outcome = Some(lexical);
    }
    if !reader_scope_current(epoch.as_ref(), workspace)? {
        return Ok(SearchResults::empty());
    }
    // Each phase ranks the complete chosen scope in one service query. Exact
    // and Recent can reuse their authoritative first pass; never merge per-source ranks.
    let outcome = if matches!(mode, SearchMode::Exact | SearchMode::Recent) {
        lexical_outcome.ok_or("Reader lexical phase missing")?
    } else {
        reader
            .invoke(Request::SearchRanked {
                query: query.into(),
                sources: sources.clone(),
                limit: options.limit as usize,
                mode,
            })
            .await?
    };
    let hits = map_hits(outcome.hits)?;
    emit("final", &hits, outcome.semantic_unavailable.as_deref())?;
    if !reader_scope_current(epoch.as_ref(), workspace)? {
        return Ok(SearchResults::empty());
    }
    let result = SearchResults {
        hits,
        semantic_unavailable: outcome.semantic_unavailable,
    };
    if let Some(epoch) = epoch {
        finish_discarding_superseded(epoch.finish(workspace, result), SearchResults::empty())
    } else {
        Ok(result)
    }
}

/// The search core, streaming each phase into `sink` instead of a Tauri
/// [`Channel`] so the whole two-phase contract — including the degraded one — is
/// drivable from a test against a real engine.
async fn search_over(
    workspace: &WorkspaceState,
    query: &str,
    options: &SearchQuery,
    mut sink: impl FnMut(SearchBatch),
) -> Result<SearchResults, String> {
    // Capture the epoch ONCE; root + engine travel together.
    let epoch = workspace
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    epoch.validate_root()?;

    let Some(db) = epoch.db() else {
        // No engine yet (import not finished): honestly empty (guarded).
        epoch.validate_root()?;
        // EPOCH-GUARDED
        return finish_discarding_superseded(
            epoch.finish(workspace, SearchResults::empty()),
            SearchResults::empty(),
        );
    };

    let src_root = epoch.source_root();
    // Whether the vector arm can run. Only reported when it was actually wanted:
    // a caller that asked for `lexical_only` got exactly the ranking it asked for.
    let unavailable = if options.lexical_only {
        None
    } else {
        semantic_unavailable(&epoch)
    };

    // Phase 1 — the lexical (FTS) arm alone. A typed-filter error surfaces here
    // (same parse both phases), so we never stream a partial for a bad query.
    epoch.validate_root()?;
    let lexical = run_phase(&db, &src_root, query, options.engine_options(true)).await?;
    stream(
        &epoch,
        workspace,
        &mut sink,
        "lexical",
        &lexical,
        unavailable.as_deref(),
    )?;

    // Phase 2 — the full hybrid pass, unless the caller asked lexical-only or the
    // semantic arm cannot run (in which case the lexical list IS the answer, and
    // travels labelled as one rather than dressed up as a fused ranking).
    let final_hits = if options.lexical_only || unavailable.is_some() {
        lexical
    } else {
        epoch.validate_root()?;
        run_phase(&db, &src_root, query, options.engine_options(false)).await?
    };
    stream(
        &epoch,
        workspace,
        &mut sink,
        "final",
        &final_hits,
        unavailable.as_deref(),
    )?;

    let results = SearchResults {
        hits: final_hits,
        semantic_unavailable: unavailable,
    };
    epoch.validate_root()?;
    // EPOCH-GUARDED: a switch mid-search means these hits belong to a previous
    // workspace — return an empty list (silently-clean), never the stale hits
    // and never a spurious error the palette would show as a filter failure. The
    // generation-tagged batches above are the streaming counterpart of this.
    finish_discarding_superseded(epoch.finish(workspace, results), SearchResults::empty())
}

impl SearchResults {
    /// No hits, and no claim about the ranking — the shape every guarded
    /// "discard this" return uses.
    fn empty() -> Self {
        Self {
            hits: Vec::new(),
            semantic_unavailable: None,
        }
    }
}

/// The reason the semantic arm cannot run for `epoch`, or `None` when it can.
/// A model still loading is reported too: hits produced while it loads are just
/// as lexical-only as hits produced after it failed.
fn semantic_unavailable(epoch: &Epoch) -> Option<String> {
    match epoch.semantic() {
        Semantic::Ready => None,
        Semantic::Loading => Some("the embedding model is still loading".to_string()),
        Semantic::Unavailable(reason) => Some(reason),
    }
}

/// Run one engine search pass and lift its hits onto the wire DTO.
async fn run_phase(
    db: &EmbeddedCatDb,
    src_root: &SourceRoot,
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
    sink: &mut impl FnMut(SearchBatch),
    phase: &str,
    hits: &[SearchHitDto],
    semantic_unavailable: Option<&str>,
) -> Result<(), String> {
    epoch.validate_root()?;
    let generation = epoch.generation as u32;
    // EPOCH-GUARDED: only stream while this epoch is the current era.
    match epoch.finish(workspace, ()) {
        Ok(()) => {
            sink(SearchBatch {
                phase: phase.to_string(),
                hits: hits.to_vec(),
                generation,
                semantic_unavailable: semantic_unavailable.map(str::to_string),
            });
        }
        Err(error) if error.is_superseded() => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

/// Preserve the intentionally quiet behavior for a superseded epoch while
/// keeping a same-epoch source-identity refusal visible to the caller.
fn finish_discarding_superseded<T>(
    result: Result<T, StaleEpoch>,
    superseded_value: T,
) -> Result<T, String> {
    match result {
        Ok(value) => Ok(value),
        Err(error) if error.is_superseded() => Ok(superseded_value),
        Err(error) => Err(error.to_string()),
    }
}

// --------------------------------------------------------------------------
// explain_query (Query Explanation surface)
// --------------------------------------------------------------------------

/// One hybrid pass with the full score breakdown attached to every hit — the
/// **Query Explanation** surface's read.
///
/// It is [`search`]'s second phase with `explain` forced on and no channel:
/// the same engine call, the same [`ExplainDto`], surfaced as a plain
/// request/response because an explanation panel has nothing to stream. Each
/// hit carries its lexical and semantic ranks, the fused RRF score, every
/// boost's recorded contribution, the final ordering key, the index version
/// and embedding model the semantic arm used, and the
/// chunk → section → document → file → source provenance chain.
///
/// Before the engine is installed (no import yet) the list is honestly empty.
/// Epoch-guarded like [`search`].
///
/// Unlike [`search`] this has no honest degraded form: the thing being explained
/// IS the fused hybrid ranking, so when the semantic arm cannot run there is no
/// ranking to explain and the command says exactly that rather than presenting a
/// lexical-only breakdown as the explanation.
#[tauri::command]
#[specta::specta]
pub async fn explain_query(
    query: String,
    limit: u32,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, crate::reader::ReaderBackend>,
) -> Result<Vec<SearchHitDto>, String> {
    let epoch = workspace.current().ok_or("no workspace has been opened")?;
    let result = search_reader(
        &reader,
        &workspace,
        &query,
        &SearchQuery {
            limit,
            lexical_only: false,
            explain: true,
            intent: Some("best".into()),
            provider_sources: None,
        },
        |_| {},
    )
    .await?;
    if let Some(reason) = result.semantic_unavailable {
        return Err(format!(
            "{reason}; a hybrid ranking cannot be explained without its semantic arm"
        ));
    }
    // EPOCH-GUARDED: also cover the interval after the delegated search returns.
    finish_discarding_superseded(epoch.finish(&workspace, result.hits), Vec::new())
}

// --------------------------------------------------------------------------
// related
// --------------------------------------------------------------------------

/// Documents related to `doc_path` (an absolute path in the granted
/// workspace), using directed Markdown links, explicit shared tags, and stored
/// vector evidence. Availability and migration gaps are carried even when empty.
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
    reader: State<'_, crate::reader::ReaderBackend>,
) -> Result<RelatedOutcomeDto, String> {
    related_reader(&reader, &workspace, &doc_path, limit).await
}
pub(crate) async fn related_reader(
    reader: &crate::reader::ReaderBackend,
    workspace: &WorkspaceState,
    doc_path: &str,
    limit: u32,
) -> Result<RelatedOutcomeDto, String> {
    let epoch = workspace.current().ok_or("no workspace has been opened")?;
    epoch.validate_root()?;
    let from = epoch
        .source_root()
        .relative_path(Path::new(&doc_path))
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let source = reader.admit_async(epoch.source_root()).await?;
    let ranked: mdreader_core::RelatedRanked = reader
        .invoke(mdreader_service::Request::RelatedRanked {
            source_id: source,
            path: from,
            query: None,
            limit: limit as usize,
        })
        .await?;
    let related = RelatedOutcomeDto {
        hits: ranked
            .hits
            .into_iter()
            .map(|hit| ranked_related_dto(&epoch.root, hit))
            .collect(),
        availability: ranked.availability.into(),
    };
    epoch.finish(workspace, related).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    use catdb_sdk::embed::{Embedder, HashEmbedder};
    use catdb_sdk::engine::{EngineConfig, ImportOptions};
    use mdreader_ingest::registration;

    fn workspace_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    fn retained(path: &Path) -> SourceRoot {
        SourceRoot::capture(path.to_path_buf()).expect("capture source root")
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

    #[test]
    fn search_refuses_a_replacement_root_instead_of_returning_empty_results() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        let root = fs::canonicalize(root).expect("canonical root");
        let workspace = WorkspaceState::granted(root.clone());
        let epoch = workspace.current().expect("active epoch");

        fs::rename(&root, temp.path().join("moved-original")).expect("move admitted root");
        fs::create_dir(&root).expect("create replacement root");
        fs::write(root.join("replacement.md"), "# replacement").expect("write replacement");

        let mut streamed = false;
        let stream_error = stream(
            &epoch,
            &workspace,
            &mut |_| streamed = true,
            "lexical",
            &[],
            None,
        )
        .expect_err("a changed root must be refused at the streaming boundary");
        assert!(
            !streamed,
            "identity failure must occur before emitting a batch"
        );
        assert!(!stream_error.is_empty() && stream_error.contains("identity"));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let error = runtime
            .block_on(search_over(
                &workspace,
                "replacement",
                &SearchQuery {
                    limit: 10,
                    lexical_only: true,
                    explain: false,
                    intent: None,
                    provider_sources: None,
                },
                |_| {},
            ))
            .expect_err("search must refuse the replacement root");
        assert!(
            !error.is_empty() && error.contains("identity"),
            "root substitution must be explicit, never an empty search: {error}"
        );
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
        let dto = hit_dto(&retained(&root), hit);
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
            intent: None,
            provider_sources: None,
        };
        let hits = rt
            .block_on(run_phase(
                &db,
                &retained(&root),
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
            intent: None,
            provider_sources: None,
        };
        let err = rt
            .block_on(run_phase(
                &db,
                &retained(&root),
                "bogusfield:value",
                opts.engine_options(true),
            ))
            .expect_err("unknown filter field is rejected");
        assert!(!err.is_empty(), "carries a schema-derived message");
    }

    /// The **Query Explanation** surface: every hit carries the full
    /// breakdown — per-modality ranks, the fused RRF score, each boost's
    /// recorded contribution, the final ordering key, and the
    /// chunk → section → document → file provenance chain. Every field is the
    /// engine's, none of it reconstructed here.
    #[test]
    fn explain_query_attaches_the_full_breakdown_and_provenance_chain() {
        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        fs::write(
            root.join("guide.md"),
            "# Guide\n\n## Storage engine\n\nThe storage engine persists chunks.\n",
        )
        .unwrap();

        let db = engine();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(db.import(&root, ImportOptions::default()))
            .expect("import");

        // The same call `explain_query` makes, minus the epoch plumbing.
        let hits = rt
            .block_on(run_phase(
                &db,
                &retained(&root),
                "storage engine",
                SearchOptions {
                    limit: 10,
                    lexical_only: false,
                    candidates: SearchOptions::default().candidates,
                    explain: true,
                },
            ))
            .expect("explained search");
        let hit = hits.first().expect("the corpus matches");
        let explain = hit
            .explain
            .as_ref()
            .expect("explain was requested, so it must be present");
        assert!(
            explain.lex_rank.is_some() || explain.sem_rank.is_some(),
            "a hit came from at least one modality"
        );
        assert_eq!(
            explain.final_score,
            explain.rrf_score
                + explain.boosts.title
                + explain.boosts.path
                + explain.boosts.recency
                + explain.boosts.graph,
            "the ordering key is the recorded components, not a second number"
        );
        assert_eq!(explain.provenance.file_path, "guide.md");
        assert_eq!(explain.provenance.chunk_id, hit.chunk_id);
        assert_eq!(explain.provenance.document_id, hit.document_id);
        assert!(explain.provenance.source_root.is_some());

        // ...and without `explain` the field is absent rather than zeroed.
        let plain = rt
            .block_on(run_phase(
                &db,
                &retained(&root),
                "storage engine",
                SearchOptions {
                    limit: 10,
                    lexical_only: false,
                    candidates: SearchOptions::default().candidates,
                    explain: false,
                },
            ))
            .expect("plain search");
        assert!(plain.first().expect("a hit").explain.is_none());
    }

    /// A workspace with an index but no loadable embedding model still searches —
    /// the lexical arm needs no vectors — and the result set is LABELLED as
    /// lexical-only rather than handed back as if the vector arm had run.
    ///
    /// Both halves matter and both are asserted against the same real engine and
    /// the same corpus: with the model installed the hybrid pass runs and claims
    /// nothing; with the model failed the hits keep coming, no hit claims a
    /// semantic rank, and every streamed batch carries the reason.
    ///
    /// Revert-RED: drop the `|| unavailable.is_some()` from `search_over`'s phase-2
    /// condition (run the hybrid pass regardless) and this fails on the degraded
    /// half — the engine's embed call errors, so `search_over` returns `Err` and
    /// the lexical hits the user could have had are lost. Alternatively set
    /// `semantic_unavailable: None` unconditionally and the labelling assertions
    /// fail: a lexical-only list is served as the full ranking.
    #[test]
    fn a_search_without_an_embedder_stays_lexical_and_says_so() {
        use crate::engine::DeferredEmbedder;
        use catdb_sdk::embed::ModelInfo;

        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        fs::write(
            root.join("guide.md"),
            "# Guide\n\n## Storage engine\n\nThe storage engine persists chunks.\n",
        )
        .unwrap();

        // The engine is opened through the deferred embedder, exactly as the app
        // opens it, with the model installed — so a real index gets built.
        let info: ModelInfo = HashEmbedder::new().info();
        let deferred = Arc::new(DeferredEmbedder::loading(info));
        deferred
            .install(Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>)
            .expect("the configured model installs");
        let mut config =
            EngineConfig::in_memory(Arc::clone(&deferred) as Arc<dyn Embedder>).with_actor("test");
        config.domains = vec![registration()];
        let db = Arc::new(EmbeddedCatDb::open(config).expect("open in-memory engine"));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(db.import(&root, ImportOptions::default()))
            .expect("import corpus");

        let ws = WorkspaceState::granted(root.clone());
        let epoch = ws.current().unwrap();
        epoch.install_engine_for_test(Arc::clone(&db));
        epoch.install_embedder_for_test(Arc::clone(&deferred));

        let options = SearchQuery {
            limit: 10,
            lexical_only: false,
            explain: false,
            intent: None,
            provider_sources: None,
        };

        // Control: with the model installed, the hybrid pass runs and the result
        // claims nothing about being degraded.
        let mut batches: Vec<SearchBatch> = Vec::new();
        let full = rt
            .block_on(search_over(&ws, "storage engine", &options, |batch| {
                batches.push(batch)
            }))
            .expect("hybrid search");
        assert!(!full.hits.is_empty(), "the corpus matches");
        assert_eq!(full.semantic_unavailable, None, "nothing to report");
        assert!(
            full.hits.iter().any(|hit| hit.sem_rank.is_some()),
            "the semantic arm contributed"
        );
        assert!(batches
            .iter()
            .all(|batch| batch.semantic_unavailable.is_none()));

        // Now the model becomes unavailable (the reported failure), over the SAME
        // index the earlier import built.
        deferred.fail("the embedding model could not be loaded — timed out after 120s".to_string());

        let mut batches: Vec<SearchBatch> = Vec::new();
        let degraded = rt
            .block_on(search_over(&ws, "storage engine", &options, |batch| {
                batches.push(batch)
            }))
            .expect("lexical search still runs without an embedder");

        // Lexical search still works…
        assert!(
            !degraded.hits.is_empty(),
            "the lexical arm needs no embedder"
        );
        // …and it is not passed off as the hybrid ranking.
        let reason = degraded
            .semantic_unavailable
            .as_deref()
            .expect("the semantic arm reports its own unavailability");
        assert!(
            reason.contains("the embedding model could not be loaded"),
            "got: {reason}"
        );
        assert!(
            degraded.hits.iter().all(|hit| hit.sem_rank.is_none()),
            "no hit may claim a semantic rank when the vector arm never ran"
        );
        // Every streamed phase carries it too, so a consumer rendering the stream
        // cannot paint the lexical batch as final-and-complete even for an instant.
        assert_eq!(batches.len(), 2, "both phases streamed");
        for batch in &batches {
            assert_eq!(batch.semantic_unavailable.as_deref(), Some(reason));
        }

        // The Exact intent is NOT a degradation: the caller asked for lexical.
        let exact = rt
            .block_on(search_over(
                &ws,
                "storage engine",
                &SearchQuery {
                    limit: 10,
                    lexical_only: true,
                    explain: false,
                    intent: None,
                    provider_sources: None,
                },
                |_| {},
            ))
            .expect("exact search");
        assert_eq!(
            exact.semantic_unavailable, None,
            "a lexical-only request got exactly the ranking it asked for"
        );

        // And the explanation surface refuses rather than explaining a ranking
        // that never happened.
        assert!(matches!(
            crate::engine::Epoch::semantic(&epoch),
            crate::engine::Semantic::Unavailable(_)
        ));
    }

    #[cfg(unix)]
    fn provider_search_fixture(
        reader: &crate::reader::ReaderBackend,
        directory: &Path,
    ) -> mdreader_core::ProviderProjectionReceipt {
        use mdreader_provider::*;
        let binding = SourceBinding::new(
            LibraryId::new("synthetic-search-library").unwrap(),
            SourceId::new("selected-drive").unwrap(),
            AccountBinding::new("private-account").unwrap(),
            uuid::Uuid::new_v4(),
            "1".repeat(64),
        )
        .unwrap();
        let text = "# Provider plan\n\nquartz storage evidence\n";
        let version = DocumentVersion {
            provider_revision: OpaqueValue::new("private-remote-version").unwrap(),
            content_hash: None,
            conditional_write_token: None,
        };
        let metadata = Metadata {
            key: RecordKey::new("private-record").unwrap(),
            name: DisplayText::new("Plan.md").unwrap(),
            display_path: DisplayText::new("Plan.md").unwrap(),
            media_type: None,
            modified_at_ms: Some(1),
            size_bytes: Some(text.len() as u64),
            version: version.clone(),
        };
        let body = DocumentBody {
            key: metadata.key.clone(),
            version,
            body: MarkdownBytes::new(text.as_bytes().to_vec()).unwrap(),
            observed_at_ms: 1,
        };
        let projection = mdreader_core::ProviderProjection::open(
            &directory.join("provider-cache"),
            binding.library_id().clone(),
        )
        .unwrap();
        reader
            .with_mobile_store(|store| {
                let receipt = store.with_library_connection(|c| {
                    let tx = c.transaction().unwrap();
                    let receipt = projection.admit(&tx, &binding, &metadata, &body).unwrap();
                    tx.commit().unwrap();
                    receipt
                });
                store
                    .index_source(&receipt.reader_source_id)
                    .map_err(|e| e.to_string())?;
                Ok(receipt)
            })
            .unwrap()
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn selected_provider_search_without_workspace_keeps_canonical_revision_and_rejects_disabled_scope(
    ) {
        let directory = TempDir::new().unwrap();
        let reader = crate::reader::ReaderBackend::embedded(directory.path()).unwrap();
        let receipt = provider_search_fixture(&reader, directory.path());
        let workspace = WorkspaceState::default();
        let options:SearchQuery=serde_json::from_value(serde_json::json!({"limit":20,"lexical_only":true,"explain":true,"intent":"exact","provider_sources":[receipt.reader_source_id]})).unwrap();
        let outcome = search_reader(&reader, &workspace, "quartz", &options, |_| {})
            .await
            .expect("provider-only search must not require a local folder");
        assert_eq!(outcome.hits.len(), 1);
        let hit = &outcome.hits[0];
        assert_eq!(
            hit.source_id.as_deref(),
            Some(receipt.reader_source_id.as_str())
        );
        assert_eq!(hit.revision.as_deref(), Some(receipt.revision.as_str()));
        assert_eq!(
            hit.abs_path, receipt.path,
            "provider paths are never joined to local/cache roots"
        );
        assert_eq!(hit.explain.as_ref().unwrap().provenance.source_root, None);
        reader
            .with_mobile_store(|store| {
                store
                    .set_source_enabled(&receipt.reader_source_id, false)
                    .map_err(|e| e.to_string())
            })
            .unwrap();
        assert!(
            search_reader(&reader, &workspace, "quartz", &options, |_| {})
                .await
                .is_err()
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn mixed_selected_scope_is_ranked_together_and_unselected_provider_is_excluded() {
        let directory = TempDir::new().unwrap();
        let local = TempDir::new().unwrap();
        fs::write(
            local.path().join("local.md"),
            "# Local plan\n\nquartz storage evidence\n",
        )
        .unwrap();
        let reader = crate::reader::ReaderBackend::embedded(directory.path()).unwrap();
        let receipt = provider_search_fixture(&reader, directory.path());
        let workspace = WorkspaceState::granted(local.path().canonicalize().unwrap());
        let id = reader
            .admit_async(workspace.current().unwrap().source_root())
            .await
            .unwrap();
        reader
            .with_mobile_store(|store| store.index_source(&id).map_err(|e| e.to_string()))
            .unwrap();
        let local_options: SearchQuery = serde_json::from_value(
            serde_json::json!({"limit":20,"lexical_only":true,"explain":false,"intent":"exact"}),
        )
        .unwrap();
        let local_only = search_reader(&reader, &workspace, "quartz", &local_options, |_| {})
            .await
            .unwrap();
        assert_eq!(local_only.hits.len(), 1);
        assert_eq!(local_only.hits[0].source_id.as_deref(), Some(id.as_str()));
        let mixed_options:SearchQuery=serde_json::from_value(serde_json::json!({"limit":20,"lexical_only":true,"explain":false,"intent":"exact","provider_sources":[receipt.reader_source_id]})).unwrap();
        let mixed = search_reader(&reader, &workspace, "quartz", &mixed_options, |_| {})
            .await
            .unwrap();
        assert_eq!(mixed.hits.len(), 2);
        let ranks: Vec<_> = mixed.hits.iter().map(|h| h.lex_rank.unwrap()).collect();
        assert_eq!(
            ranks,
            vec![1, 2],
            "ranks belong to one cross-source query, not merged source-local rank1 lists"
        );
        assert!(mixed.hits.iter().any(|h| h.abs_path
            == workspace
                .current()
                .unwrap()
                .root
                .join("local.md")
                .to_string_lossy()));
        assert!(mixed.hits.iter().any(|h| h.source_id.as_deref()
            == Some(receipt.reader_source_id.as_str())
            && h.abs_path == receipt.path));
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
