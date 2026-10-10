//! Explicit native current read replaces only its captured exact observation.
//! It cannot regrant a key, silently rewrite edit expectations or replay a write.
use super::super::metadata_observation::{self, Observation};
use super::*;

pub(crate) struct Head {
    binding: SourceBinding,
    observation: Observation,
    epoch: u64,
    row: Metadata,
    digest: Vec<u8>,
}
fn epoch(c: &Connection, binding: &SourceBinding) -> Result<u64> {
    db(c.query_row(
        "SELECT epoch FROM mdreader_provider_registry_state WHERE source=?1",
        [identity(binding)],
        |r| r.get(0),
    ))
}
fn digest(c: &Connection, snapshot: &str, key: &RecordKey) -> Result<Vec<u8>> {
    db(c.query_row(
        "SELECT digest FROM mdreader_provider_registry_rows WHERE snapshot=?1 AND key=?2",
        params![snapshot, key.as_str()],
        |r| r.get(0),
    ))
}
pub(crate) fn capture(
    c: &mut Connection,
    binding: &SourceBinding,
    key: &RecordKey,
) -> Result<Head> {
    initialize(c)?;
    let observation = metadata_observation::snapshot(c, binding)?;
    let row = metadata_observation::metadata(c, binding, key)?;
    let id = observation.id.as_deref().ok_or(ProviderError::Conflict)?;
    Ok(Head {
        binding: binding.clone(),
        epoch: epoch(c, binding)?,
        digest: digest(c, id, key)?,
        observation,
        row,
    })
}
fn exact_row(tx: &Transaction<'_>, snapshot: &Snapshot, row: &Metadata) -> Result<()> {
    // Listing may preserve an old token when it omits one. A fresh native
    // observation is authoritative, including safely withdrawing a validator.
    db(tx.execute(
        "DELETE FROM mdreader_provider_registry_rows WHERE snapshot=?1 AND key=?2",
        params![snapshot.id.to_string(), row.key.as_str()],
    ))?;
    insert_row(tx, snapshot, row)
}
pub(crate) fn cache(
    c: &mut Connection,
    head: Head,
    fresh: &Metadata,
    document: &DocumentBody,
) -> Result<()> {
    initialize(c)?;
    let tx = db(c.transaction())?;
    let descriptor = descriptor_at(&tx, &head.binding, true)?;
    let current = metadata_observation::snapshot(&tx, &head.binding)?;
    if current != head.observation || epoch(&tx, &head.binding)? != head.epoch {
        return Err(ProviderError::Conflict);
    }
    let id = current.id.as_deref().ok_or(ProviderError::Conflict)?;
    if digest(&tx, id, &head.row.key)? != head.digest
        || encode(
            &metadata_observation::metadata(&tx, &head.binding, &head.row.key)?,
            MAX_METADATA,
        )? != encode(&head.row, MAX_METADATA)?
        || fresh.key != head.row.key
        || document.key != head.row.key
    {
        return Err(ProviderError::Conflict);
    }
    if !super::super::links::allowed(&descriptor, fresh) {
        return Err(ProviderError::Unauthorized);
    }
    validate_body(fresh, document)?;
    if head.row.version.provider_revision == fresh.version.provider_revision
        && head.row.version.content_hash == fresh.version.content_hash
    {
        let prior: Option<(Vec<u8>,Vec<u8>)> = db(tx.query_row("SELECT version,digest FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND key=?3",params![identity(&head.binding),head.binding.digest().as_slice(),fresh.key.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
        if prior.is_some_and(|(version, digest)| {
            version == version_digest(&head.row.version)
                && digest != body_hash(document.body.as_bytes())
        }) {
            return Err(ProviderError::Conflict);
        }
    }
    if encode(fresh, MAX_METADATA)? != encode(&head.row, MAX_METADATA)? {
        exact_row(
            &tx,
            &Snapshot {
                id: Uuid::parse_str(id).map_err(|_| ProviderError::Persistence)?,
                binding: head.binding.clone(),
                epoch: head.epoch,
            },
            fresh,
        )?;
        let pending: Option<String> = db(tx.query_row("SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1 AND binding=?2 AND phase!='published' AND base_epoch=?3",params![identity(&head.binding),head.binding.digest().as_slice(),head.epoch],|r|r.get(0)).optional())?;
        if let Some(pending) = pending.as_ref().filter(|pending| pending.as_str() != id) {
            // Preserve an in-progress rebuild, while its old page handles fail.
            // Only this verified current key is rebased; all other rows/cursors stay.
            exact_row(
                &tx,
                &Snapshot {
                    id: Uuid::parse_str(pending).map_err(|_| ProviderError::Persistence)?,
                    binding: head.binding.clone(),
                    epoch: head.epoch,
                },
                fresh,
            )?;
        }
        let next = head.epoch.checked_add(1).ok_or(ProviderError::TooLarge)?;
        db(tx.execute(
            "UPDATE mdreader_provider_registry_state SET epoch=?2 WHERE source=?1 AND epoch=?3",
            params![identity(&head.binding), next, head.epoch],
        ))?;
        if let Some(pending) = pending {
            db(tx.execute("UPDATE mdreader_provider_registry_snapshots SET base_epoch=?2 WHERE id=?1 AND base_epoch=?3",params![pending,next,head.epoch]))?;
        }
    }
    store_body(&tx, &head.binding, fresh, document, None)?;
    db(tx.commit())
}
#[cfg(test)]
#[path = "fresh_read_tests.rs"]
mod tests;
