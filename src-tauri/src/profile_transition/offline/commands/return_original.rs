//! Same-operation continuation for a requested archive rollback. An uncertain
//! candidate handoff cannot become a fresh Preview or preparation operation.
use super::*;

pub(super) fn run(app: &AppHandle, active: &mut Pending) -> Result<MigrationReply> {
    if !active.rollback_pending {
        return Err(Error::Conflict);
    }
    let archive = active.archive.as_mut().ok_or(Error::Conflict)?;
    // Absence after releasing original guards means launch was already issued
    // without a confirmed result. Do not re-launch or reuse the old baseline.
    let legacy = active
        .legacy
        .as_mut()
        .ok_or(Error::ReconciliationRequired)?;
    let launch = legacy.launch_identity()?;
    legacy.original_unchanged(archive.path())?;
    archive.preserve_candidate()?;
    legacy.original_unchanged(archive.path())?;
    launch.verify()?;
    let mut result = reply("rollback_launch_requested", Some(active.id));
    result.origin = Some("legacy_catdb");
    result.coverage = Some(archive.coverage()?);
    result.imported = archive.imported()?;
    crate::reader_profile::forget_imported(app, archive.selected_profile()?)?;
    // Release exact original locks only after complete candidate preservation.
    // The new reader binary never opens or migrates the old CatDB profile.
    active.legacy.take();
    launch.launch()?;
    active.rollback_pending = false;
    Ok(result)
}
