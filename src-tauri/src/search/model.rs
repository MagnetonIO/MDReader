use super::*;
#[derive(Debug, Clone, Deserialize, Type)]
pub struct SearchQuery {
    /// Maximum hits per phase.
    pub limit: u32,
    /// Skip the vector arm entirely (the `Exact` intent) — one phase only.
    pub lexical_only: bool,
    /// Attach the full per-hit explanation breakdown to every hit.
    pub explain: bool,
    #[serde(default)]
    #[specta(optional)]
    pub intent: Option<String>,
    /// Explicit enabled canonical provider sources; absence retains local-only search.
    #[serde(default)]
    #[specta(optional)]
    pub provider_sources: Option<Vec<String>>,
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

/// One hybrid-search hit on the wire: provenance (path, heading breadcrumb,
/// line range), per-modality ranks, boost contributions, an optional explain
/// breakdown, plus the absolute path the reader opens and the file mtime the
/// `Recent` intent sorts on.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchHitDto {
    #[specta(optional)]
    pub source_kind: Option<String>,
    /// Exact native device identity when this hit belongs to the imported library.
    #[specta(optional)]
    pub device_document_id: Option<String>,
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
    /// Final exact selected-source observation. Superseded results claim none.
    pub coverage: Option<super::SearchCoverageDto>,
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
pub(super) fn ranked_related_dto(
    root: &Path,
    hit: mdreader_core::RelatedRankedHit,
) -> RelatedDocDto {
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
