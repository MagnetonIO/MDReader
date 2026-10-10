//! Private resumed discovery input. This is metadata knowledge, never a read
//! receipt or a replacement selected scope. Actual provider reads stay live.
use super::*;
use rusqlite::params;

pub(super) fn read(c: &mut Connection, binding: &SourceBinding) -> Result<Vec<Metadata>> {
    let pending = resume(c, binding)?;
    let Some(pending) = pending else {
        return coordinator::published_metadata(c, binding, 0, 16_384);
    };
    if !registry::snapshot_is_current(c, &pending.snapshot)? {
        return coordinator::published_metadata(c, binding, 0, 16_384);
    }
    let descriptor = registry::descriptor_at(c, binding, true)?;
    let mut statement = registry::db(c.prepare("SELECT metadata,digest FROM mdreader_provider_registry_rows WHERE snapshot=?1 ORDER BY key LIMIT 16385"))?;
    let records = registry::db(
        statement.query_map(params![pending.snapshot.id().to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        }),
    )?;
    let mut result = Vec::new();
    for record in records {
        let (text, digest) = registry::db(record)?;
        if body_hash(text.as_bytes()).as_slice() != digest {
            return Err(ProviderError::Persistence);
        }
        let metadata: Metadata = registry::decode(&text, 32 * 1024)?;
        if !links::allowed(&descriptor, &metadata) {
            return Err(ProviderError::Unauthorized);
        }
        result.push(metadata);
    }
    if result.len() > 16_384 {
        return Err(ProviderError::TooLarge);
    }
    Ok(result)
}
