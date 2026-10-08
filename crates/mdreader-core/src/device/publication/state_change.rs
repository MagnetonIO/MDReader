use super::{
    current::{guard_current, verify_live},
    *,
};
use crate::device::{self, state::DocumentState};
use rusqlite::Transaction;

pub enum PositionUpdate {
    Preserve,
    Start,
    Line(u32),
}
pub struct StateUpdate {
    pub pinned: bool,
    pub position: PositionUpdate,
}
pub struct StateReceipt {
    pub(super) state: DocumentState,
}
impl StateReceipt {
    pub fn state(&self) -> &DocumentState {
        &self.state
    }
}

pub fn publish_state(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
    update: StateUpdate,
) -> PublicationResult<StateReceipt> {
    verify_live(tx, guard, current)?;
    let line = match update.position {
        PositionUpdate::Preserve => current.plan.state.document.line,
        PositionUpdate::Start => 1,
        PositionUpdate::Line(line) => line,
    };
    if line == 0 || line > current.max_line {
        return Err(device::Error::InvalidPosition.into());
    }
    device::state::update_in(tx, &current.plan.state, update.pinned, line)?;
    let state = device::state::load(tx, &current.plan.state.document.id, true)?;
    let receipt = StateReceipt { state };
    revalidate_state(tx, guard, current, &receipt)?;
    Ok(receipt)
}
/// Caller invokes after all outer receipt/cursor writes, immediately precommit.
pub fn revalidate_state(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
    receipt: &StateReceipt,
) -> PublicationResult<()> {
    guard_current(tx, guard, current)?;
    let mut expected = current.plan.state.clone();
    expected.generation = expected
        .generation
        .checked_add(1)
        .ok_or(device::Error::StateLimit)?;
    expected.document.pinned = receipt.state.document.pinned;
    expected.document.line = receipt.state.document.line;
    if expected != receipt.state
        || expected.document.line == 0
        || expected.document.line > current.max_line
        || device::state::load(tx, &expected.document.id, true)? != expected
    {
        return Err(PublicationError::Conflict);
    }
    current
        .plan
        .assets
        .verify_current_assets_in(tx, &current.plan.asset_state)?;
    Ok(())
}
