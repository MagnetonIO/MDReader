//! Selected metadata census. No body downloads or index/readiness inference.
use super::*;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingCompleteness {
    NeverObserved,
    Partial,
    CompleteSelected,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataListing {
    pub known_documents: u32,
    pub completeness: ListingCompleteness,
    pub observed_at_ms: Option<i64>,
    /// Durable continuation exists; this does not claim a request is running.
    pub refresh_pending: bool,
}

pub(super) fn observe(c: &Connection, binding: &SourceBinding) -> Result<MetadataListing> {
    let observation = metadata_observation::snapshot(c, binding)?;
    let refresh_pending: bool = registry::db(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM mdreader_provider_registry_snapshots s JOIN mdreader_provider_registry_state r ON r.source=s.source WHERE s.source=?1 AND s.binding=?2 AND s.base_epoch=r.epoch AND s.phase IN ('enumerating','reconciling','ready'))",
        params![registry::identity(binding), binding.digest().as_slice()], |r| r.get(0),
    ))?;
    let Some(snapshot) = &observation.id else {
        return Ok(MetadataListing {
            known_documents: 0,
            completeness: ListingCompleteness::NeverObserved,
            observed_at_ms: None,
            refresh_pending,
        });
    };
    let (count, observed): (u32, i64) = registry::db(c.query_row(
        "SELECT (SELECT count(*) FROM mdreader_provider_registry_rows WHERE snapshot=s.id),s.observed FROM mdreader_provider_registry_snapshots s WHERE s.id=?1 AND s.source=?2 AND s.binding=?3",
        params![snapshot, registry::identity(binding), binding.digest().as_slice()], |r| Ok((r.get(0)?, r.get(1)?)),
    ))?;
    if count > 16_384 || observed < 0 {
        return Err(ProviderError::Persistence);
    }
    Ok(MetadataListing {
        known_documents: count,
        completeness: if observation.partial {
            ListingCompleteness::Partial
        } else {
            ListingCompleteness::CompleteSelected
        },
        observed_at_ms: Some(observed),
        refresh_pending,
    })
}
