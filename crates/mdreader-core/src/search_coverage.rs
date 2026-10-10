//! Bounded index observations for an already authorized search scope.
//! Counts describe retained derived rows, never a census of live folder files.
use super::{MAX_SEARCH_SOURCES, ReaderError, ReaderStore, Result, catalog, checked_path, invalid};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchDocumentReceipt {
    pub source_id: String,
    pub path: String,
    pub revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchIndexPhase {
    Idle,
    Queued,
    Indexing,
    Loading,
    Embedding,
    Done,
    ModelUnavailable,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchModelState {
    Disabled,
    NotDownloaded,
    Loading,
    Ready,
    Busy,
    Unavailable,
    RestartRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndexSnapshot {
    pub authority: String,
    pub index: Option<String>,
    pub observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMetadataState {
    NeverObserved,
    Partial,
    CompleteSelected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMetadataCoverage {
    pub state: SearchMetadataState,
    pub snapshot: Option<String>,
    pub known_documents: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSearchCoverage {
    pub source_id: String,
    pub snapshot: SearchIndexSnapshot,
    pub job_phase: SearchIndexPhase,
    pub job_generation: Option<u64>,
    pub indexed_documents: u64,
    pub indexed_chunks: u64,
    pub embedded_chunks: u64,
    /// Unknown unless an exact native document receipt supplies the denominator.
    /// No subtraction from a filename listing or old job counters is valid.
    pub eligible_documents: Option<u64>,
    /// Current enabled provider projection bodies, not cache or metadata rows.
    pub admitted_bodies: Option<u64>,
    /// Supplied by the registry owner; absent when no metadata census exists.
    pub metadata: Option<SearchMetadataCoverage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchCoverage {
    /// Opaque identity of the exact selected sources, receipts and observations.
    pub scope_token: String,
    pub sources: Vec<SourceSearchCoverage>,
    pub model: SearchModelState,
}

impl ReaderStore {
    /// Exact receipts only narrow an existing source grant. This does not read
    /// bodies, enumerate files, queue work, load a model or expand authority.
    pub fn search_coverage(
        &self,
        sources: &[String],
        exact: &[SearchDocumentReceipt],
    ) -> Result<SearchCoverage> {
        let ids = self.scope(sources)?;
        let mut seen = BTreeSet::new();
        if exact.len() > MAX_SEARCH_SOURCES {
            return Err(invalid("Search coverage receipt budget exceeded"));
        }
        for receipt in exact {
            checked_path(&receipt.path)?;
            if !ids.contains(&receipt.source_id)
                || !seen.insert(receipt.source_id.as_str())
                || receipt.revision.len() != 64
                || !receipt.revision.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid("Invalid search coverage receipt"));
            }
        }
        catalog::schema::validate(&self.library, false)?;
        catalog::schema::validate(&self.index, true)?;
        let library = self.library.unchecked_transaction()?;
        let index = self.index.unchecked_transaction()?;
        let result = ids
            .iter()
            .map(|id| self.coverage_source(id, exact.iter().find(|r| r.source_id == *id)))
            .collect::<Result<Vec<_>>>()?;
        index.commit()?;
        library.commit()?;
        // The separate derived/library databases have separate snapshots.
        // Repeat the bounded observation after releasing them and refuse an
        // intervening index/grant/job/projection change instead of mixing eras.
        for (id, observed) in ids.iter().zip(&result) {
            if self.coverage_source(id, exact.iter().find(|r| r.source_id == *id))? != *observed {
                return Err(ReaderError::Conflict);
            }
        }
        self.scope(&ids)?;
        let encoded = serde_json::to_vec(&(&result, exact))
            .map_err(|_| invalid("Invalid search coverage"))?;
        Ok(SearchCoverage {
            scope_token: blake3::hash(&encoded).to_hex().to_string(),
            sources: result,
            model: SearchModelState::Unavailable,
        })
    }

    fn coverage_source(
        &self,
        id: &str,
        exact: Option<&SearchDocumentReceipt>,
    ) -> Result<SourceSearchCoverage> {
        self.stored_root(id, true)?.verify()?;
        let authority: String = self.library.query_row(
            "SELECT epoch FROM reader_catalog_authorities_v1 WHERE source_id=?1",
            [id],
            |r| r.get(0),
        )?;
        let snapshot: Option<(String, Option<i64>)> = self
            .index
            .query_row(
                "SELECT snapshot,indexed_at_ms FROM reader_catalog_snapshots_v1 WHERE source_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if authority.len() != 32
            || !authority.bytes().all(|b| b.is_ascii_hexdigit())
            || snapshot.as_ref().is_some_and(|(token, at)| {
                token.len() != 32
                    || !token.bytes().all(|b| b.is_ascii_hexdigit())
                    || at.is_some_and(|at| at < 0)
            })
        {
            return Err(invalid("Invalid search coverage snapshot"));
        }
        let job = self.index_job_record(id)?;
        let phase = match job.as_ref().map(|j| j.phase.as_str()) {
            None => SearchIndexPhase::Idle,
            Some("queued") => SearchIndexPhase::Queued,
            Some("indexing") => SearchIndexPhase::Indexing,
            Some("loading") => SearchIndexPhase::Loading,
            Some("embedding") => SearchIndexPhase::Embedding,
            Some("done") => SearchIndexPhase::Done,
            Some("model-unavailable") => SearchIndexPhase::ModelUnavailable,
            Some("error") => SearchIndexPhase::Error,
            _ => return Err(invalid("Invalid search coverage job")),
        };
        let (documents, chunks, embedded): (u64, u64, u64) = self.index.query_row(
            "WITH chosen AS (SELECT id FROM available_documents WHERE source_id=?1 AND (?2 IS NULL OR (path=?2 AND revision=?3))), passages AS (SELECT c.id FROM chunks c JOIN chosen d ON d.id=c.document_id) SELECT (SELECT count(*) FROM chosen),(SELECT count(*) FROM passages),(SELECT count(*) FROM embedding_memberships m JOIN passages p ON p.id=m.chunk_id WHERE m.source_id=?1)",
            params![id, exact.map(|r| r.path.as_str()), exact.map(|r| r.revision.as_str())],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        )?;
        let admitted = if id.starts_with("provider:") {
            self.provider_enabled(id)?;
            Some(self.library.query_row(
                "SELECT count(*) FROM core_provider_records WHERE reader_source_id=?1",
                [id],
                |r| r.get(0),
            )?)
        } else {
            None
        };
        Ok(SourceSearchCoverage {
            source_id: id.into(),
            snapshot: SearchIndexSnapshot {
                authority,
                index: snapshot.as_ref().map(|(s, _)| s.clone()),
                observed_at_ms: snapshot.and_then(|(_, at)| at),
            },
            job_phase: phase,
            job_generation: job.map(|j| j.generation),
            indexed_documents: documents,
            indexed_chunks: chunks,
            embedded_chunks: embedded,
            eligible_documents: exact.map(|_| 1),
            admitted_bodies: admitted,
            metadata: None,
        })
    }
}

#[cfg(test)]
#[path = "search_coverage/tests.rs"]
mod tests;
