//! Separate private native owner for returning a currently selected library.
use super::*;
use mdreader_profile::{Error, Result};

pub(super) struct Target(pub(super) SourceRoot);
pub(super) fn validate(
    slot: &default_selection::DefaultSelection,
    path: &Path,
) -> Result<SourceRoot> {
    let target = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
    if slot.selected()?.as_ref() != Some(&target) {
        return Err(Error::Conflict);
    }
    crate::profile_transition::offline::verify_return_candidate(&target.path)?;
    target.verify().map_err(|_| Error::Conflict)?;
    if slot.selected()?.as_ref() != Some(&target) {
        return Err(Error::Conflict);
    }
    Ok(target)
}
pub(super) fn target(app: &tauri::AppHandle) -> Result<SourceRoot> {
    let target = app.try_state::<Target>().ok_or(Error::Unsupported)?;
    let slot = app
        .try_state::<default_selection::DefaultSelection>()
        .ok_or(Error::Conflict)?;
    let current = validate(&slot, &target.0.path)?;
    if current != target.0 {
        return Err(Error::Conflict);
    }
    Ok(current)
}
