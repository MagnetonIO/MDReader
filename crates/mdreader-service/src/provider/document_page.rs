//! UI metadata pages share the catalog's retained observation and recent order.
//! This never changes the complete metadata input passed to provider factories.
use super::*;
use rusqlite::params;

pub(super) struct Page {
    pub rows: Vec<Metadata>,
    pub next_offset: Option<usize>,
}

pub(super) fn read(c: &Connection, b: &SourceBinding, offset: usize, limit: usize) -> Result<Page> {
    if limit == 0 || limit > 256 || offset > 16_384 {
        return Err(ProviderError::Invalid);
    }
    let observation = metadata_observation::snapshot(c, b)?;
    let descriptor = registry::descriptor_at(c, b, true)?;
    let mut rows = Vec::new();
    if let Some(snapshot) = &observation.id {
        let mut query = registry::db(c.prepare(
            "SELECT metadata,digest FROM mdreader_provider_registry_rows WHERE snapshot=?1 ORDER BY COALESCE(json_extract(metadata,'$.modified_at_ms'),-9223372036854775808) DESC,key LIMIT ?2 OFFSET ?3",
        ))?;
        let found = registry::db(query.query_map(params![snapshot, limit + 1, offset], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        }))?;
        for row in found {
            let (text, digest) = registry::db(row)?;
            if body_hash(text.as_bytes()).as_slice() != digest {
                return Err(ProviderError::Persistence);
            }
            let row: Metadata = registry::decode(&text, 32 * 1024)?;
            if !links::allowed(&descriptor, &row) {
                return Err(ProviderError::Unauthorized);
            }
            rows.push(row);
        }
    }
    if metadata_observation::snapshot(c, b)? != observation {
        return Err(ProviderError::Cancelled);
    }
    let next_offset = (rows.len() > limit).then_some(offset + limit);
    rows.truncate(limit);
    Ok(Page { rows, next_offset })
}
