//! Explicit removal of one source and its native sign-in.
//!
//! Order matters: core authority is revoked and derived copies are forgotten
//! first, then custody is deleted, then the registry row is removed, and only
//! then is the provider asked to revoke. A custody failure leaves a paused,
//! forgotten source that can be disconnected again; a revoke failure is
//! reported but never undoes the local removal. A grant already out of
//! custody is still revoked when removing the row fails. A factory without
//! custody refuses before any local change. Mutation journal, conflict and
//! create evidence keyed by the old binding is retained, as with forget.
use super::*;
use mdreader_credentials::CredentialReference;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub(super) async fn disconnect<O: Owner>(
    owner: &O,
    projection: &dyn ProjectionAdmission,
    factory: &dyn ProviderFactory,
    binding: &SourceBinding,
    context: &IoContext,
) -> Result<Value> {
    // Without custody the sign-in could not be deleted, so nothing is changed.
    if !factory.has_credential_custody() {
        return Err(ProviderError::Unsupported);
    }
    let (reference, shared) = owner.with_connection(|c| {
        forget_materialization(c, binding, projection)?;
        let current = configuration(c, binding.source_id())?;
        if current.descriptor.binding() != binding {
            return Err(ProviderError::Conflict);
        }
        let shared = shares_reference(c, binding, &current.credential_reference)?;
        Ok((current.credential_reference, shared))
    })?;
    let retired = if shared {
        None
    } else {
        let reference = CredentialReference::new(reference.as_str().into())?;
        factory.delete_credential(&reference, context).await?
    };
    // The grant is already out of custody: revoke it even if removing the row
    // fails, so a retry is never left holding a live, unreachable grant.
    let removed = owner.with_connection(|c| remove(c, binding));
    let revocation = match retired {
        Some(retired) => factory.revoke_credential(retired, context).await,
        None => Revocation::NotAttempted,
    };
    removed?;
    Ok(json!({
        "disconnected": true,
        "credential": if shared { "shared" } else { "deleted" },
        "revocation": revocation,
    }))
}

/// References are unique per admission today; this guard keeps a future shared
/// reference from deleting another source's sign-in.
fn shares_reference(
    c: &Connection,
    binding: &SourceBinding,
    reference: &OpaqueValue,
) -> Result<bool> {
    registry::db(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM mdreader_provider_registry_sources WHERE credential_reference=?1 AND identity!=?2)",
        params![reference.as_str(), registry::identity(binding)],
        |r| r.get(0),
    ))
}

/// Admission receipts stay so a retired reference can never be replayed.
pub(super) fn remove(c: &mut Connection, binding: &SourceBinding) -> Result<()> {
    let id = registry::identity(binding);
    let tx = registry::db(c.transaction())?;
    let enabled: Option<bool> = registry::db(
        tx.query_row(
            "SELECT enabled FROM mdreader_provider_registry_sources WHERE identity=?1 AND binding=?2",
            params![id, binding.digest().as_slice()],
            |r| r.get(0),
        )
        .optional(),
    )?;
    // Only a source this request already disabled and forgot may be removed.
    if enabled != Some(false) {
        return Err(ProviderError::Conflict);
    }
    registry::db(tx.execute(
        "DELETE FROM mdreader_provider_registry_cache WHERE source=?1",
        [&id],
    ))?;
    registry::db(tx.execute(
        "DELETE FROM mdreader_provider_registry_state WHERE source=?1",
        [&id],
    ))?;
    registry::db(tx.execute(
        "DELETE FROM mdreader_provider_registry_sources WHERE identity=?1",
        [&id],
    ))?;
    registry::db(tx.commit())
}
