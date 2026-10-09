use super::*;
use custody::{EnrollmentState, Lifecycle, Phase};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in super::super) enum ApprovalState {
    Prepared,
    Uncertain,
    Confirmed,
}
/// One bounded slot, encrypted by OS custody. Never exposed as a renderer DTO.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(in super::super) enum Slot {
    Sending {
        request: PairRequestPacket,
        body: approval::Body,
        packet: PairApprovalPacket,
        state: ApprovalState,
    },
    Receiving {
        request: PairRequestPacket,
        packet: PairApprovalPacket,
        grant: String,
        jwe: String,
    },
}
pub(in super::super) fn validate_slot(record: &CustodyRecord) -> AuthorityResult<()> {
    let Some(slot) = &record.pairing else {
        return Ok(());
    };
    match slot {
        Slot::Sending {
            request,
            body,
            packet,
            ..
        } => {
            if !matches!(
                record.lifecycle,
                Lifecycle::V2 {
                    phase: Phase::Enrolled,
                    ..
                }
            ) || record.state != EnrollmentState::Confirmed
                || packet.sender_thumb != record.scope.device_id()
            {
                return Err(AuthorityError::Custody);
            }
            request.check_scope(record.scope.account_digest(), record.scope.library_id())?;
            packet.check(request)?;
            approval::validate_body(body, request, packet)?;
        }
        Slot::Receiving {
            request,
            packet,
            grant,
            jwe,
        } => {
            if !matches!(
                record.lifecycle,
                Lifecycle::V2 {
                    phase: Phase::CandidatePending(CandidateKind::Pair),
                    ..
                }
            ) || record.state != EnrollmentState::Pending
                || record
                    .intent
                    .as_ref()
                    .is_none_or(|i| i.kind != enrollment::Kind::Pair)
                || request != &candidate::own_request(record)?
            {
                return Err(AuthorityError::Custody);
            }
            grant::Grant::verify(grant, request, packet, jwe, false)?;
            decrypt(record, request, packet, jwe)?;
        }
    }
    Ok(())
}
pub(super) fn decrypt(
    record: &CustodyRecord,
    request: &PairRequestPacket,
    packet: &PairApprovalPacket,
    jwe: &str,
) -> AuthorityResult<std::collections::BTreeMap<String, Zeroizing<Vec<u8>>>> {
    let Lifecycle::V2 { wrapping: raw, .. } = &record.lifecycle else {
        return Err(AuthorityError::Custody);
    };
    let key = wrapping::WrappingKey::from_bytes(Zeroizing::new(
        raw.as_slice()
            .try_into()
            .map_err(|_| AuthorityError::Custody)?,
    ))?;
    let plaintext = wrapping::decrypt_pair(jwe, &key, &request.context(&packet.sender_thumb)?)?;
    material::parse(plaintext.as_bytes(), request)
}
pub(in super::super) fn slot_transition(
    old: &CustodyRecord,
    new: &CustodyRecord,
    expected: Option<u64>,
) -> AuthorityResult<bool> {
    if expected != Some(old.generation)
        || new.generation
            != old
                .generation
                .checked_add(1)
                .ok_or(AuthorityError::Generation)?
        || old.state != new.state
        || old.intent != new.intent
        || old.pending_recovery != new.pending_recovery
    {
        return Ok(false);
    }
    Ok(match (&old.pairing, &new.pairing) {
        (
            None,
            Some(Slot::Sending {
                state: ApprovalState::Prepared,
                ..
            }),
        ) => old.state == EnrollmentState::Confirmed,
        (None, Some(Slot::Receiving { .. })) => old.state == EnrollmentState::Pending,
        (
            Some(Slot::Sending {
                request: a,
                body: b,
                packet: c,
                state: before,
            }),
            Some(Slot::Sending {
                request: d,
                body: e,
                packet: f,
                state: after,
            }),
        ) => {
            a == d
                && b == e
                && c == f
                && matches!(
                    (before, after),
                    (ApprovalState::Prepared, ApprovalState::Uncertain)
                        | (ApprovalState::Uncertain, ApprovalState::Confirmed)
                )
        }
        (
            Some(Slot::Sending {
                state: ApprovalState::Confirmed,
                ..
            }),
            None,
        ) => old.state == EnrollmentState::Confirmed,
        _ => false,
    })
}
pub(in super::super) fn activation_transition(
    old: &CustodyRecord,
    new: &CustodyRecord,
    expected: Option<u64>,
) -> AuthorityResult<bool> {
    let Some(Slot::Receiving {
        request,
        packet,
        grant,
        jwe,
    }) = &old.pairing
    else {
        return Ok(false);
    };
    let (
        Lifecycle::V2 {
            wrapping: a,
            recovery: None,
            phase: Phase::CandidatePending(CandidateKind::Pair),
        },
        Lifecycle::V2 {
            wrapping: b,
            recovery: None,
            phase: Phase::Enrolled,
        },
    ) = (&old.lifecycle, &new.lifecycle)
    else {
        return Ok(false);
    };
    grant::Grant::verify(grant, request, packet, jwe, false)?;
    Ok(expected == Some(old.generation)
        && new.generation
            == old
                .generation
                .checked_add(1)
                .ok_or(AuthorityError::Generation)?
        && old.reference == new.reference
        && old.registration == new.registration
        && old.scope == new.scope
        && old.pkcs8 == new.pkcs8
        && a == b
        && old.state == EnrollmentState::Pending
        && new.state == EnrollmentState::Confirmed
        && old.keys.is_empty()
        && new.keys == decrypt(old, request, packet, jwe)?
        && old.pending_recovery.is_none()
        && new.pending_recovery.is_none()
        && old.pair_renewal.is_none()
        && new.pair_renewal.is_none()
        && new.pairing.is_none()
        && match (&old.intent, &new.intent) {
            (Some(a), Some(b)) => a.permits_confirmation(b),
            _ => false,
        })
}
