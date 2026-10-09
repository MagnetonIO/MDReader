//! A selected GUI never bypasses its own JSON/editor owner. It requests a
//! distinct private controller, then the user closes it normally before retry.
use super::*;
pub(super) fn resume(app: &AppHandle, pending: &mut Option<Pending>) -> Result<MigrationReply> {
    let target = crate::reader_profile::return_target(app)?;
    let migration = target.path.parent().ok_or(Error::Invalid)?;
    if let Some(active) = pending.as_mut() {
        if active
            .windows
            .prepared
            .as_ref()
            .is_none_or(|prepared| prepared.launch_directory() != migration)
        {
            return Err(Error::Conflict);
        }
        return active.finish(app);
    }
    let mut prepared = super::super::super::windows_workspace::Prepared::resume(migration)?;
    if prepared.state()? != "launch_requested" {
        return Err(Error::Conflict);
    }
    let root = prepared.original_root()?;
    let id = prepared.native_operation();
    *pending = Some(Pending {
        id,
        windows: Retained {
            root,
            handoff: Default::default(),
            original: None,
            prepared: Some(prepared),
            rollback_pending: true,
            controller: None,
        },
    });
    pending.as_mut().ok_or(Error::Conflict)?.finish(app)
}
pub(super) fn execute(app: &AppHandle, active: &mut Pending) -> Result<MigrationReply> {
    if let Some(child) = active.windows.controller.as_mut() {
        if child
            .try_wait()
            .map_err(|_| Error::ReconciliationRequired)?
            .is_none()
        {
            return Ok(active.waiting());
        }
        // A terminal retained handle is proof that this controller cannot still
        // publish. Re-read the durable lifecycle below; retry only if unfinished.
        active.windows.controller = None;
        active.windows.rollback_pending = true;
    }
    let prepared = active.windows.prepared.as_mut().ok_or(Error::Conflict)?;
    let migration = prepared.launch_directory();
    if prepared.state()? == "rolled_back" {
        crate::reader_profile::returned_to_original(app, &migration.join("candidate"))?;
        active.windows.rollback_pending = false;
        let mut result = reply("rolled_back", Some(active.id));
        result.origin = Some("reader");
        return Ok(result);
    }
    let current = crate::reader_profile::directory(app).map_err(|_| Error::Conflict)?;
    if prepared.state()? == "launch_requested" {
        let candidate = super::super::super::windows_workspace::selected_for_launch(&migration)?;
        if current == candidate.path {
            if !crate::reader_profile::is_imported_selected(app, &candidate.path)? {
                return Err(Error::Conflict);
            }
            // Retain the actual child handle. A Windows CreateProcess failure
            // returns no child; Retry can dispatch again without a stranded flag.
            active.windows.rollback_pending = true;
            let child =
                std::process::Command::new(std::env::current_exe().map_err(|_| Error::Storage)?)
                    .arg("--reader-return-controller")
                    .arg(candidate.path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|_| Error::ReconciliationRequired)?;
            active.windows.controller = Some(child);
            return Ok(active.waiting());
        }
    }
    active.windows.rollback_pending = true;
    active.windows.root.verify().map_err(|_| Error::Conflict)?;
    if active.windows.original.is_none() {
        let owner =
            ClosedOwner::acquire_retaining(&active.windows.root.path, &mut active.windows.handoff)?;
        active.windows.root.verify().map_err(|_| Error::Conflict)?;
        active.windows.original = Some(owner);
    }
    let original = active.windows.original.as_mut().ok_or(Error::Conflict)?;
    prepared.rollback(original)?;
    // The exact default slot must still reference this native selector. The
    // portable rollback selected its original, including nonstandard roots.
    crate::reader_profile::returned_to_original(app, &migration.join("candidate"))?;
    let selected = super::super::super::windows_workspace::selected_for_launch(&migration)?;
    if selected.path != active.windows.root.path {
        return Err(Error::Conflict);
    }
    active.windows.rollback_pending = false;
    let mut result = reply("rollback_launch_requested", Some(active.id));
    result.origin = Some("reader");
    Ok(result)
}
pub(super) fn launch(migration: PathBuf) -> Result<()> {
    std::process::Command::new(std::env::current_exe().map_err(|_| Error::Storage)?)
        .arg("--reader-selection")
        .arg(migration)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| Error::ReconciliationRequired)?;
    Ok(())
}
