//! User-verified out-of-band pairing; public packets never confer authority alone.
mod activation;
mod approval;
mod approval_challenge;
mod candidate;
mod grant;
mod material;
mod packets;
mod state;
use super::*;
pub use packets::{PairApprovalPacket, PairRequestPacket};
pub(super) use state::{activation_transition, slot_transition, validate_slot, Slot};
pub(super) fn owner_summary(record: &CustodyRecord) -> PairApprovalSummary {
    match &record.pairing {
        Some(Slot::Sending {
            state: state::ApprovalState::Prepared,
            ..
        }) => PairApprovalSummary::Prepared,
        Some(Slot::Sending {
            state: state::ApprovalState::Uncertain,
            ..
        }) => PairApprovalSummary::Uncertain,
        Some(Slot::Sending {
            state: state::ApprovalState::Confirmed,
            ..
        }) => PairApprovalSummary::Confirmed,
        _ => PairApprovalSummary::None,
    }
}
use std::time::{Duration, Instant};
use wrapping::{PairContext, PublicJwk};
use zeroize::Zeroizing;

pub(super) fn request_body(record: &CustodyRecord, challenge: &str) -> AuthorityResult<String> {
    let (signing, wrapping, _) = enrollment::bootstrap::public_material(record)?;
    serde_json::to_string(
        &serde_json::json!({"challenge":challenge,"signingJwk":signing,"wrappingJwk":wrapping}),
    )
    .map_err(|_| AuthorityError::Custody)
}

async fn persist(
    factory: &NativeAuthorityFactory,
    record: &mut CustodyRecord,
) -> AuthorityResult<()> {
    let generation = record.generation;
    record.generation = generation
        .checked_add(1)
        .ok_or(AuthorityError::Generation)?;
    factory
        .vault
        .exchange(generation, record, Instant::now() + Duration::from_secs(5))
        .await?;
    let current = factory
        .vault
        .get(&record.reference, Instant::now() + Duration::from_secs(5))
        .await?
        .ok_or(AuthorityError::Custody)?;
    if current.generation != record.generation || !current.same_material(record) {
        return Err(AuthorityError::Generation);
    }
    Ok(())
}

const MAX_PACKET: usize = 8192;

fn nonce(value: &str) -> AuthorityResult<()> {
    if value.len() != 43
        || crate::crypto::decode_canonical(value)
            .map_err(|_| AuthorityError::Scope)?
            .len()
            != 32
    {
        return Err(AuthorityError::Scope);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
