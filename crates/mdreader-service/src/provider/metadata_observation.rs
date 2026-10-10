//! Readable cold-list observations. Completed snapshots remain authoritative
//! during refresh; an incomplete first enumeration never claims completeness.
use super::*;
use rusqlite::{params, OptionalExtension};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Observation {
    pub id: Option<String>,
    pub partial: bool,
    pub progress: u64,
}

pub(super) fn snapshot(c: &Connection, binding: &SourceBinding) -> Result<Observation> {
    registry::descriptor_at(c, binding, true)?;
    let published: Option<String> = registry::db(c.query_row(
        "SELECT published FROM mdreader_provider_registry_state WHERE source=?1",
        [registry::identity(binding)],
        |r| r.get(0),
    ))?;
    if let Some(id) = published {
        Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)?;
        return Ok(Observation {
            id: Some(id),
            partial: false,
            progress: 0,
        });
    }
    let staged: Option<(String, u64)> = registry::db(c.query_row(
        "SELECT s.id,(SELECT count(*) FROM mdreader_provider_registry_seen p WHERE p.snapshot=s.id) FROM mdreader_provider_registry_snapshots s JOIN mdreader_provider_registry_state r ON r.source=s.source WHERE s.source=?1 AND s.binding=?2 AND s.phase IN ('enumerating','reconciling','ready') AND s.base_epoch=r.epoch",
        params![registry::identity(binding), binding.digest().as_slice()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional())?;
    match staged {
        Some((id, progress)) => {
            Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)?;
            Ok(Observation {
                id: Some(id),
                partial: true,
                progress,
            })
        }
        None => Ok(Observation {
            id: None,
            partial: false,
            progress: 0,
        }),
    }
}

/// A row is only an observation. Provider.read still verifies live membership,
/// revision and bytes; cache/admission repeat this exact metadata check.
pub(super) fn metadata(
    c: &Connection,
    binding: &SourceBinding,
    key: &RecordKey,
) -> Result<Metadata> {
    let observed = snapshot(c, binding)?;
    let id = observed.id.ok_or(ProviderError::Unavailable)?;
    let row = registry::metadata_in(c, &id, key)?.ok_or(ProviderError::NotFound)?;
    let descriptor = registry::descriptor_at(c, binding, true)?;
    if !super::links::allowed(&descriptor, &row) {
        return Err(ProviderError::Unauthorized);
    }
    Ok(row)
}
