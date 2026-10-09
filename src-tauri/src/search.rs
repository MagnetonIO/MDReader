//! Native search uses the one MDReader SQLite owner; no SDK query engine.
mod model;
use crate::{
    commands::WorkspaceState,
    engine::{Epoch, StaleEpoch},
};
use mdreader_core::SourceRoot;
use model::ranked_related_dto;
pub use model::*;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use tauri::{ipc::Channel, State};
fn basename(value: &str) -> String {
    value.rsplit(['/', '\\']).next().unwrap_or(value).into()
}
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
    device: Option<&mdreader_service::catalog::DeviceSearchReceipt>,
    explain: bool,
) -> SearchHitDto {
    let document_id = device.map(|r| r.document_id.clone()).unwrap_or_else(|| {
        uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("mdreader:{}:{}", hit.source_id, hit.path).as_bytes(),
        )
        .to_string()
    });
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
        device_document_id: device.map(|r| r.document_id.clone()),
        source_kind: None,
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
    let owned: mdreader_service::catalog::ReaderOwnedSources =
        reader.invoke(Request::ReaderOwnedSources).await?;
    sources.extend(owned.captures.iter().map(|s| s.id.clone()));
    sources.extend(owned.devices.iter().map(|s| s.source_id.clone()));
    if sources.len() > mdreader_core::MAX_SEARCH_SOURCES {
        return Err("Selected search scope exceeds the reader source budget".into());
    }
    if sources.is_empty() {
        return Err(
            "Import Markdown, enable captures, open a folder or select a cloud source for search"
                .into(),
        );
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
                let device = owned.devices.iter().find(|d| d.source_id == hit.source_id);
                if device.is_some_and(|d| d.revision != hit.revision) {
                    return Err("Imported document changed during search".into());
                }
                let kind = if device.is_some() {
                    "device"
                } else if owned.captures.iter().any(|s| s.id == hit.source_id) {
                    "capture"
                } else if canonical_provider_source(&hit.source_id) {
                    "provider"
                } else {
                    "local"
                };
                let mut value = reader_hit(root.as_ref(), hit, device, options.explain);
                value.source_kind = Some(kind.into());
                Ok(value)
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
        let current = reader.invoke(Request::ReaderOwnedSources).await?;
        if !crate::reader_owned::same(&owned, &current) {
            return Err("Reader-owned search scope changed".into());
        }
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
    let current = reader.invoke(Request::ReaderOwnedSources).await?;
    if !crate::reader_owned::same(&owned, &current) {
        return Err("Reader-owned search scope changed".into());
    }
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
mod tests;
