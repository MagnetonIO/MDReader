//! Accepted create receipts publish metadata, exact body, projection and journal
//! together. Cold enumeration retains its checkpoint and stale page handles fail.
use super::*;
use mdreader_provider::creation as create_journal;
pub fn publish_created(
    c: &mut Connection,
    binding: &SourceBinding,
    operation: Uuid,
    projection: &dyn ProjectionAdmission,
    observed_at_ms: i64,
) -> Result<ProjectionReceipt> {
    initialize(c)?;
    descriptor_at(c, binding, true)?;
    let pending =
        create_journal::load(c, binding, operation)?.ok_or(ProviderError::Unauthorized)?;
    if let Some(receipt) = pending.projection {
        return decode(&receipt, 16384);
    }
    if pending.state != journal::MutationState::Accepted {
        return Err(ProviderError::LocalPublicationPending);
    }
    let metadata = pending.metadata.ok_or(ProviderError::Persistence)?;
    let descriptor = descriptor_at(c, binding, true)?;
    if !descriptor.capabilities().guarded_write
        || !super::super::links::allowed(&descriptor, &metadata)
        || !matches!(
            descriptor.scope(),
            SelectedScope::DriveFolder { .. } | SelectedScope::ScopedRoot { .. }
        )
    {
        return Err(ProviderError::Unauthorized);
    }
    let document = DocumentBody {
        key: metadata.key.clone(),
        version: metadata.version.clone(),
        body: pending.body,
        observed_at_ms,
    };
    let tx = db(c.transaction())?;
    let observed = super::super::metadata_observation::snapshot(&tx, binding)?;
    let id = observed.id.ok_or(ProviderError::LocalPublicationPending)?;
    if let Some(prior) = metadata_in(&tx, &id, &metadata.key)? {
        if prior.version != metadata.version {
            return Err(ProviderError::Conflict);
        }
    }
    let epoch: u64 = db(tx.query_row(
        "SELECT epoch FROM mdreader_provider_registry_state WHERE source=?1",
        [identity(binding)],
        |r| r.get(0),
    ))?;
    insert_row(
        &tx,
        &Snapshot {
            id: Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)?,
            binding: binding.clone(),
            epoch,
        },
        &metadata,
    )?;
    let receipt = store_body(&tx, binding, &metadata, &document, Some(projection))?
        .ok_or(ProviderError::Unsupported)?;
    db(tx.execute(
        "UPDATE mdreader_provider_registry_state SET epoch=epoch+1 WHERE source=?1",
        [identity(binding)],
    ))?;
    if observed.partial {
        db(tx.execute("UPDATE mdreader_provider_registry_snapshots SET base_epoch=?2 WHERE id=?1 AND base_epoch=?3",params![id,epoch.checked_add(1).ok_or(ProviderError::TooLarge)?,epoch]))?;
    }
    create_journal::published(&tx, binding, operation, &encode(&receipt, 16384)?)?;
    db(tx.commit())?;
    Ok(receipt)
}
