//! Refine an exact listed revision with the validator observed by native read.
//! This never substitutes another revision, hash, membership or renderer input.
use super::*;
pub(super) fn observed(
    tx: &Transaction<'_>,
    binding: &SourceBinding,
    document: &DocumentBody,
) -> Result<Metadata> {
    let mut row = super::super::metadata_observation::metadata(tx, binding, &document.key)?;
    if row.version != document.version {
        if row.version.provider_revision != document.version.provider_revision
            || row.version.content_hash != document.version.content_hash
            || row.version.conditional_write_token.is_some()
            || document.version.conditional_write_token.is_none()
        {
            return Err(ProviderError::Conflict);
        }
        let prior: Option<(Vec<u8>, Vec<u8>)> = db(tx.query_row(
            "SELECT version,digest FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND key=?3",
            params![identity(binding), binding.digest().as_slice(), document.key.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?))).optional())?;
        if prior.is_some_and(|(version, digest)| {
            version == version_digest(&row.version) && digest != body_hash(document.body.as_bytes())
        }) {
            return Err(ProviderError::Conflict);
        }
        let observed = super::super::metadata_observation::snapshot(tx, binding)?;
        let id = observed.id.ok_or(ProviderError::Conflict)?;
        let epoch = db(tx.query_row(
            "SELECT epoch FROM mdreader_provider_registry_state WHERE source=?1",
            [identity(binding)],
            |r| r.get(0),
        ))?;
        row.version = document.version.clone();
        validate_body(&row, document)?;
        insert_row(
            tx,
            &Snapshot {
                id: Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)?,
                binding: binding.clone(),
                epoch,
            },
            &row,
        )?;
    }
    Ok(row)
}
