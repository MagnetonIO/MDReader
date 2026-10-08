use super::{
    current::{guard_current, verify_live},
    *,
};
use crate::device::{
    self,
    state::{DocumentState, RetirementKind, RetirementRecord},
};
use rusqlite::{params, OptionalExtension, Transaction};

pub struct RetirementReceipt {
    pub(super) state: DocumentState,
}
impl RetirementReceipt {
    pub fn state(&self) -> &DocumentState {
        &self.state
    }
}
pub fn retire_current(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
    record: RetirementRecord,
) -> PublicationResult<RetirementReceipt> {
    verify_live(tx, guard, current)?;
    record.validate()?;
    let origin: IncomingOrigin = serde_json::from_str(&guard.origin)?;
    if record.operation_id != origin.wire_revision {
        return Err(PublicationError::Conflict);
    }
    let old = &current.plan.state;
    if old.generation >= device::state::MAX_GENERATION {
        return Err(device::Error::StateLimit.into());
    }
    if let Some(replacement) = record.replacement {
        let new = device::state::load(tx, &replacement.to_string(), true)?;
        if new.document.id == old.document.id || new.document.source_id == old.document.source_id {
            return Err(PublicationError::Conflict);
        }
    }
    if tx.execute("UPDATE device_documents_v1 SET retired=1,retirement=?1,state_generation=state_generation+1 WHERE id=?2 AND revision=?3 AND state_generation=?4 AND retired=0",params![serde_json::to_string(&record)?,old.document.id,old.document.revision,old.generation])?!=1 {return Err(PublicationError::Conflict);}
    if tx.execute(
        "UPDATE sources SET enabled=0 WHERE id=?1 AND enabled=1",
        [&old.document.source_id],
    )? != 1
    {
        return Err(PublicationError::Conflict);
    }
    current
        .plan
        .assets
        .retire_current_assets_in(tx, &current.plan.asset_state)?;
    tx.execute("UPDATE index_jobs SET phase='error',error='Source retired',generation=generation+1 WHERE source_id=?1",[&old.document.source_id])?;
    let receipt = RetirementReceipt {
        state: device::state::load(tx, &old.document.id, false)?,
    };
    revalidate_retirement(tx, guard, current, &receipt)?;
    Ok(receipt)
}
/// Pure authority revocation; original/pending files and rows remain exportable.
pub fn revalidate_retirement(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
    receipt: &RetirementReceipt,
) -> PublicationResult<()> {
    guard_current(tx, guard, current)?;
    let mut expected = current.plan.state.clone();
    expected.generation = expected
        .generation
        .checked_add(1)
        .ok_or(device::Error::StateLimit)?;
    expected.retirement = receipt.state.retirement.clone();
    if expected.retirement.is_none()
        || expected != receipt.state
        || device::state::load(tx, &expected.document.id, false)? != expected
    {
        return Err(PublicationError::Conflict);
    }
    let enabled: bool = tx.query_row(
        "SELECT enabled FROM sources WHERE id=?1",
        [&expected.document.source_id],
        |r| r.get(0),
    )?;
    let job: Option<String> = tx
        .query_row(
            "SELECT phase FROM index_jobs WHERE source_id=?1",
            [&expected.document.source_id],
            |r| r.get(0),
        )
        .optional()?;
    if enabled || job.as_deref().is_some_and(|phase| phase != "error") {
        return Err(PublicationError::Conflict);
    }
    current
        .plan
        .assets
        .verify_retired_assets_in(tx, &current.plan.asset_state)?;
    if let Some(record) = &expected.retirement {
        if record.kind == RetirementKind::Replaced {
            let replacement = record.replacement.ok_or(PublicationError::Conflict)?;
            device::state::load(tx, &replacement.to_string(), true)?;
        }
    }
    Ok(())
}
