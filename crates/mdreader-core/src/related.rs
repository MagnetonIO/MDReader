//! Ranked Related uses outgoing Markdown links (two hops, 0.5 decay), explicit
//! frontmatter tag Jaccard, and stored-vector evidence. The vector arm compares
//! the anchor's stored-vector mean to every eligible destination chunk, retaining
//! its best cosine. It does not infer embeddings or pretend tags are concepts.
//! Legacy knowledge entities/spans, concept joins and optional insights remain
//! unmigrated. The older one-hop incoming/outgoing `related` API is unchanged.
use super::{invalid, vector_bytes, Cancellation, ReaderError, Reference, Result, EMBEDDING_DIM};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedRanked {
    /// Indexed anchor revision for this consistent read snapshot; absent if unknown.
    pub anchor: Option<Reference>,
    pub hits: Vec<RelatedRankedHit>,
    pub availability: RelatedAvailability,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedRankedHit {
    pub source_id: String,
    pub path: String,
    pub revision: String,
    pub title: String,
    pub line: u32,
    pub score: f64,
    pub components: RelatedRankedComponents,
    pub graph_depth: Option<u8>,
    pub shared_tag_count: u64,
    pub tag_union_count: u64,
    pub vector_similarity: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedRankedComponents {
    /// Weighted 0.5 for one outgoing hop or 0.25 for two, best path only.
    pub graph: f64,
    /// 0.5 * actual frontmatter-tag Jaccard; not the legacy entity concept model.
    pub shared_tags: f64,
    /// 0.4 * clamped measured cosine; None means no comparison was available.
    pub vector: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedAvailability {
    pub graph: bool,
    pub shared_tags: bool,
    pub vector: RelatedVectorState,
    pub anchor_chunks: u64,
    /// Coverage is explicit; a partial anchor uses only its installed vectors.
    pub anchor_vectors: u64,
    pub evaluated_vector_chunks: u64,
    pub evaluated_vector_documents: u64,
    pub legacy_concepts: RelatedLegacyState,
    pub legacy_graph_entities: RelatedLegacyState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelatedVectorState {
    Available,
    AnchorMissing,
    NoAnchorVectors,
    ZeroCentroid,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelatedLegacyState {
    NotMigrated,
}

pub const RELATED_GRAPH_WEIGHT: f64 = 1.0;
pub const RELATED_SHARED_TAG_WEIGHT: f64 = 0.5;
pub const RELATED_VECTOR_WEIGHT: f64 = 0.4;
const MAX_CANDIDATES: usize = 100_000;
const MAX_GRAPH_EDGES: usize = 100_000;
const MAX_VECTOR_CHUNKS: usize = 200_000;

#[derive(Default)]
struct Candidate {
    graph_depth: Option<u8>,
    graph_line: Option<u32>,
    shared: u64,
    union: u64,
    similarity: Option<f64>,
    vector_line: Option<u32>,
}
fn check_candidates(candidates: &HashMap<i64, Candidate>) -> Result<()> {
    if candidates.len() > MAX_CANDIDATES {
        return Err(invalid("related candidate budget exceeded"));
    }
    Ok(())
}
fn graph_neighbors(
    connection: &Connection,
    source: &str,
    origin: i64,
    remaining: usize,
) -> Result<Vec<(i64, u32)>> {
    // Group repeated links/headings to one neighbor before extending the walk.
    Ok(connection.prepare_cached("SELECT r.target_id,MIN(r.target_line) FROM resolved_links r JOIN documents d ON d.id=r.target_id WHERE r.source_id=?1 AND r.origin_id=?2 AND d.source_id=?1 GROUP BY r.target_id LIMIT ?3")?
      .query_map(params![source,origin,remaining as i64+1],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<std::result::Result<_,_>>()?)
}
fn graph(
    connection: &Connection,
    source: &str,
    anchor: i64,
    candidates: &mut HashMap<i64, Candidate>,
    cancel: &Cancellation,
) -> Result<()> {
    let first = graph_neighbors(connection, source, anchor, MAX_GRAPH_EDGES)?;
    let mut visited_edges = first.len();
    if visited_edges > MAX_GRAPH_EDGES {
        return Err(invalid("related graph traversal budget exceeded"));
    }
    for &(document, line) in &first {
        if document == anchor {
            continue;
        }
        let candidate = candidates.entry(document).or_default();
        candidate.graph_depth = Some(1);
        candidate.graph_line = Some(line);
    }
    check_candidates(candidates)?;
    for (origin, _) in first {
        cancel.check()?;
        if origin == anchor {
            continue;
        }
        let second = graph_neighbors(connection, source, origin, MAX_GRAPH_EDGES - visited_edges)?;
        visited_edges += second.len();
        if visited_edges > MAX_GRAPH_EDGES {
            return Err(invalid("related graph traversal budget exceeded"));
        }
        for (document, line) in second {
            if document == anchor {
                continue;
            }
            let candidate = candidates.entry(document).or_default();
            if candidate.graph_depth.is_none() {
                candidate.graph_depth = Some(2);
                candidate.graph_line = Some(line);
            } else if candidate.graph_depth == Some(2) {
                candidate.graph_line = Some(candidate.graph_line.unwrap_or(line).min(line));
            }
        }
        check_candidates(candidates)?;
    }
    Ok(())
}
fn shared_tags(
    connection: &Connection,
    anchor: i64,
    filters: &str,
    values: &[Value],
    candidates: &mut HashMap<i64, Candidate>,
    cancel: &Cancellation,
) -> Result<()> {
    let mine: u64 = connection.query_row(
        "SELECT COUNT(*) FROM document_tags WHERE document_id=?1",
        [anchor],
        |r| r.get(0),
    )?;
    if mine == 0 {
        return Ok(());
    }
    let sql=format!("SELECT d.id,COUNT(*),(SELECT COUNT(*) FROM document_tags total WHERE total.document_id=d.id) FROM document_tags own JOIN document_tags other ON other.tag=own.tag JOIN documents d ON d.id=other.document_id WHERE own.document_id=? AND d.id<>? AND {filters} GROUP BY d.id LIMIT ?");
    let mut parameters: Vec<Value> = vec![anchor.into(), anchor.into()];
    parameters.extend_from_slice(values);
    parameters.push((MAX_CANDIDATES as i64 + 1).into());
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(parameters))?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        cancel.check()?;
        count += 1;
        if count > MAX_CANDIDATES {
            return Err(invalid("related shared-tag candidate budget exceeded"));
        }
        let document: i64 = row.get(0)?;
        let shared: u64 = row.get(1)?;
        let theirs: u64 = row.get(2)?;
        let candidate = candidates.entry(document).or_default();
        candidate.shared = shared;
        candidate.union = mine + theirs - shared;
        check_candidates(candidates)?;
    }
    Ok(())
}
fn centroid(
    connection: &Connection,
    anchor: i64,
    cancel: &Cancellation,
) -> Result<(u64, Option<Vec<f32>>)> {
    let mut sums = vec![0.0f64; EMBEDDING_DIM];
    let mut count = 0;
    let mut statement=connection.prepare("SELECT v.embedding FROM chunks c JOIN vectors v ON v.rowid=c.id WHERE c.document_id=?1 ORDER BY c.id")?;
    let mut rows = statement.query([anchor])?;
    while let Some(row) = rows.next()? {
        cancel.check()?;
        count += 1;
        if count > MAX_VECTOR_CHUNKS as u64 {
            return Err(invalid("related anchor vector budget exceeded"));
        }
        let bytes: Vec<u8> = row.get(0)?;
        if bytes.len() != EMBEDDING_DIM * 4 {
            return Err(invalid("invalid indexed embedding dimensions"));
        }
        for (sum, bytes) in sums.iter_mut().zip(bytes.chunks_exact(4)) {
            let value = f32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| invalid("invalid indexed embedding"))?,
            );
            if !value.is_finite() {
                return Err(invalid("invalid indexed embedding value"));
            }
            *sum += f64::from(value);
        }
    }
    // Cosine is scale invariant: normalizing the sum is the same direction as
    // the stored-vector mean, with stable accumulation and no f32 overflow.
    let norm = sums.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm == 0.0 {
        return Ok((count, None));
    }
    let centroid = sums.into_iter().map(|sum| (sum / norm) as f32).collect();
    Ok((count, Some(centroid)))
}
fn vectors(
    connection: &Connection,
    anchor: i64,
    centroid: &[f32],
    filter: (&str, &[Value]),
    candidates: &mut HashMap<i64, Candidate>,
    cancel: &Cancellation,
) -> Result<(u64, u64)> {
    let (filters, values) = filter;
    // Force vec0 row-order iteration. Reordered per-chunk point reads jump
    // between large vector blobs and repeatedly traverse overflow pages.
    // Scope/filter and exclude the entire anchor
    // before any bound; chunk-heavy documents cannot crowd a global top-k pool.
    let sql=format!("SELECT d.id,c.line_start,1.0-vec_distance_cosine(v.embedding,?) FROM vectors v CROSS JOIN chunks c ON c.id=v.rowid CROSS JOIN documents d ON d.id=c.document_id WHERE d.id<>? AND {filters} LIMIT ?");
    let mut parameters: Vec<Value> = vec![vector_bytes(centroid)?.into(), anchor.into()];
    parameters.extend_from_slice(values);
    parameters.push((MAX_VECTOR_CHUNKS as i64 + 1).into());
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(parameters))?;
    let mut count = 0u64;
    let mut documents = HashSet::new();
    while let Some(row) = rows.next()? {
        cancel.check()?;
        count += 1;
        if count > MAX_VECTOR_CHUNKS as u64 {
            return Err(invalid("related vector scan budget exceeded"));
        }
        let document: i64 = row.get(0)?;
        let line: u32 = row.get(1)?;
        let similarity: f64 = row.get(2)?;
        if !similarity.is_finite() {
            return Err(invalid("invalid indexed vector similarity"));
        }
        let similarity = similarity.clamp(-1.0, 1.0);
        documents.insert(document);
        if similarity <= 0.0 && !candidates.contains_key(&document) {
            continue;
        }
        let candidate = candidates.entry(document).or_default();
        if candidate
            .similarity
            .is_none_or(|previous| similarity > previous)
        {
            candidate.similarity = Some(similarity);
            candidate.vector_line = Some(line);
        } else if candidate.similarity == Some(similarity) {
            candidate.vector_line = Some(candidate.vector_line.unwrap_or(line).min(line));
        }
        check_candidates(candidates)?;
    }
    Ok((count, documents.len() as u64))
}

pub(super) fn rank(
    connection: &Connection,
    source: &str,
    path: &str,
    filters: &str,
    values: &[Value],
    limit: usize,
    cancel: &Cancellation,
) -> Result<RelatedRanked> {
    let anchor: Option<(i64, String)> = connection
        .query_row(
            "SELECT id,revision FROM documents WHERE source_id=?1 AND path=?2",
            params![source, path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let mut availability = RelatedAvailability {
        graph: anchor.is_some(),
        shared_tags: anchor.is_some(),
        vector: RelatedVectorState::AnchorMissing,
        anchor_chunks: 0,
        anchor_vectors: 0,
        evaluated_vector_chunks: 0,
        evaluated_vector_documents: 0,
        legacy_concepts: RelatedLegacyState::NotMigrated,
        legacy_graph_entities: RelatedLegacyState::NotMigrated,
    };
    let Some((anchor, revision)) = anchor else {
        return Ok(RelatedRanked {
            anchor: None,
            hits: Vec::new(),
            availability,
        });
    };
    availability.anchor_chunks = connection.query_row(
        "SELECT COUNT(*) FROM chunks WHERE document_id=?1",
        [anchor],
        |r| r.get(0),
    )?;
    let mut candidates = HashMap::new();
    graph(connection, source, anchor, &mut candidates, cancel)?;
    shared_tags(connection, anchor, filters, values, &mut candidates, cancel)?;
    let (embedded, centroid) = centroid(connection, anchor, cancel)?;
    availability.anchor_vectors = embedded;
    availability.vector = if embedded == 0 {
        RelatedVectorState::NoAnchorVectors
    } else if centroid.is_none() {
        RelatedVectorState::ZeroCentroid
    } else {
        RelatedVectorState::Available
    };
    if let Some(centroid) = centroid {
        (
            availability.evaluated_vector_chunks,
            availability.evaluated_vector_documents,
        ) = vectors(
            connection,
            anchor,
            &centroid,
            (filters, values),
            &mut candidates,
            cancel,
        )?;
    }
    // Remove zero-only candidates, but keep measured zero/negative cosine on a
    // graph/tag candidate distinct from an unavailable vector measurement.
    candidates.retain(|_, candidate| {
        candidate.graph_depth.is_some()
            || candidate.shared > 0
            || candidate.similarity.is_some_and(|score| score > 0.0)
    });
    let mut hits = Vec::new();
    if !candidates.is_empty() {
        let ids = candidates.keys().copied().collect::<Vec<_>>();
        let sql=format!("SELECT d.id,d.path,d.revision,d.title FROM documents d WHERE d.id IN(SELECT value FROM json_each(?)) AND {filters}");
        let mut parameters: Vec<Value> = vec![serde_json::to_string(&ids)
            .map_err(|_| invalid("invalid related candidates"))?
            .into()];
        parameters.extend_from_slice(values);
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(parameters))?;
        while let Some(row) = rows.next()? {
            cancel.check()?;
            let document: i64 = row.get(0)?;
            let Some(candidate) = candidates.get(&document) else {
                return Err(ReaderError::Unavailable);
            };
            let components = RelatedRankedComponents {
                graph: candidate.graph_depth.map_or(0.0, |depth| {
                    RELATED_GRAPH_WEIGHT * 0.5f64.powi(i32::from(depth))
                }),
                shared_tags: if candidate.union > 0 {
                    RELATED_SHARED_TAG_WEIGHT * candidate.shared as f64 / candidate.union as f64
                } else {
                    0.0
                },
                vector: candidate
                    .similarity
                    .map(|score| RELATED_VECTOR_WEIGHT * score.clamp(0.0, 1.0)),
            };
            hits.push(RelatedRankedHit {
                source_id: source.into(),
                path: row.get(1)?,
                revision: row.get(2)?,
                title: row.get(3)?,
                line: candidate.graph_line.or(candidate.vector_line).unwrap_or(1),
                score: components.graph + components.shared_tags + components.vector.unwrap_or(0.0),
                components,
                graph_depth: candidate.graph_depth,
                shared_tag_count: candidate.shared,
                tag_union_count: candidate.union,
                vector_similarity: candidate.similarity,
            });
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    hits.truncate(limit);
    Ok(RelatedRanked {
        anchor: Some(Reference {
            source_id: source.into(),
            path: path.into(),
            revision,
            line: 1,
        }),
        hits,
        availability,
    })
}
