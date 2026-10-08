//! Selected deletion uses the ordinary native retirement transition. Its exact
//! durable outgoing identity cannot bypass head, dirty-generation or session fences.
use super::*;
use crate::sync::outgoing::association;

pub(super) fn check(
    owner: &mut ReaderStore,
    scope: &mdreader_sync::SyncScope,
    identity: &identity::Identity,
    origin: &publication::IncomingOrigin,
) -> Result<Option<PendingReason>> {
    let intent = owner.with_library_connection(|db| -> Result<_> {
        let Some(stored) = association::by_revision(db, scope, &identity.change.revision_id)?
        else {
            if identity.change.device == scope.device_id() {
                let tx = db.transaction()?;
                let local =
                    mdreader_sync::outbox_revision_in(&tx, scope, &identity.change.revision_id)?
                        .is_some();
                tx.commit()?;
                if local {
                    return Ok(Err(PendingReason::BaselineUnknown));
                }
            }
            return Ok(Ok(None));
        };
        let intent = stored.intent;
        let tx = db.transaction()?;
        let proof = mdreader_sync::published_tombstone_proof_in(
            &tx,
            scope,
            &intent.operation.operation_id,
            &identity.change,
        )?;
        if proof.entry().operation != intent.operation || !intent.operation.deleted {
            return Err(Error::Conflict);
        }
        if stored.applied.is_some() {
            return Ok(Ok(None));
        }
        if intent.origin != *origin {
            return Ok(Err(PendingReason::ReauthorizationRequired));
        }
        let mut expected = intent.head.clone().ok_or(Error::Conflict)?;
        let actual = heads::get(&tx, &identity::scope_key(scope)?, &expected.item)?
            .ok_or(Error::Conflict)?;
        // A prior interrupted application may already have observed this exact
        // anti-resurrection tombstone, without retiring or advancing the cursor.
        if actual.tombstone.as_deref() == Some(identity.change.revision_id.as_str()) {
            expected.tombstone = actual.tombstone.clone();
        }
        if actual != expected {
            return Ok(Err(PendingReason::HeadConflict));
        }
        Ok(Ok(Some(intent)))
    })?;
    let intent = match intent {
        Err(reason) => return Ok(Some(reason)),
        Ok(None) => return Ok(None),
        Ok(Some(intent)) => intent,
    };
    let current = mdreader_core::device::state::current(owner, &intent.native_document)
        .map_err(publication::PublicationError::from)?;
    if current.generation != intent.generation
        || current.document.revision != intent.native_revision
    {
        return Ok(Some(PendingReason::LocalStateChanged));
    }
    Ok(None)
}
