//! Registry observations only: no cache bodies, network, handles or credentials.
use super::*;
use mdreader_core::{SearchMetadataCoverage, SearchMetadataState};
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;

pub(crate) fn observe(
    c: &mut Connection,
    sources: &[String],
) -> Result<BTreeMap<String, SearchMetadataCoverage>> {
    let mut result = BTreeMap::new();
    let selected: Vec<_> = sources
        .iter()
        .filter_map(|s| s.strip_prefix("provider:"))
        .collect();
    if selected.is_empty() {
        return Ok(result);
    }
    if selected.len() > 128 {
        return Err(ProviderError::TooLarge);
    }
    let exists: bool = registry::db(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='mdreader_provider_registry_sources')", [], |r| r.get(0)))?;
    // Projection-only synthetic/core clients have no provider registry census.
    if !exists {
        return Ok(result);
    }
    let tx = registry::db(c.transaction())?;
    for selected in selected {
        let raw = registry::db(tx.query_row("SELECT substr(descriptor,1,65537),substr(digest,1,33),substr(coverage,1,32769) FROM mdreader_provider_registry_sources WHERE lower(hex(binding))=?1", [selected], |r| Ok((r.get::<_, String>(0)?,r.get::<_, Vec<u8>>(1)?,r.get::<_, String>(2)?))).optional())?;
        let Some((text, digest, coverage)) = raw else {
            continue;
        };
        if body_hash(text.as_bytes()).as_slice() != digest {
            return Err(ProviderError::Persistence);
        }
        let descriptor: SourceDescriptor = registry::decode(&text, 65536)?;
        let binding = descriptor.binding();
        let id = format!("provider:{}", hex(&binding.digest()));
        if id != format!("provider:{selected}") {
            return Err(ProviderError::Persistence);
        }
        registry::descriptor_at(&tx, binding, true)?;
        let before = metadata_observation::snapshot(&tx, binding)?;
        let coverage: Coverage = registry::decode(&coverage, 32768)?;
        let state = if before.id.is_none() {
            SearchMetadataState::NeverObserved
        } else if before.partial
            || matches!(coverage, Coverage::Partial { .. } | Coverage::NeverIndexed)
        {
            SearchMetadataState::Partial
        } else {
            SearchMetadataState::CompleteSelected
        };
        let mut known = 0;
        {
            let mut s = registry::db(tx.prepare("SELECT substr(metadata,1,32769),substr(digest,1,33) FROM mdreader_provider_registry_rows WHERE snapshot=?1 LIMIT 16385"))?;
            let rows = registry::db(s.query_map([before.id.as_deref()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            }))?;
            for row in rows {
                let (text, digest) = registry::db(row)?;
                if body_hash(text.as_bytes()).as_slice() != digest {
                    return Err(ProviderError::Persistence);
                }
                let metadata: Metadata = registry::decode(&text, 32768)?;
                if !super::links::allowed(&descriptor, &metadata) {
                    return Err(ProviderError::Unauthorized);
                }
                known += 1;
                if known > 16384 {
                    return Err(ProviderError::TooLarge);
                }
            }
        }
        result.insert(
            id,
            SearchMetadataCoverage {
                state,
                snapshot: before.id,
                known_documents: known,
            },
        );
    }
    registry::db(tx.commit())?;
    Ok(result)
}
