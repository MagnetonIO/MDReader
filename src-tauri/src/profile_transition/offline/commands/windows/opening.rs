//! Native selected roots and retained pre-exit observations are acquired here.
use super::*;
pub(super) fn execute(
    app: &AppHandle,
    request: MigrationRequest,
    pending: &mut Option<Pending>,
) -> Result<MigrationReply> {
    if let Some(active) = pending.as_mut() {
        return if active.windows.original.is_some()
            || active.windows.prepared.is_some()
            || active.windows.observer.is_some()
            || active.windows.legacy.is_some()
            || active.windows.controller_root.is_some()
            || active.windows.reobserve.is_some()
        {
            active.finish(app)
        } else {
            Ok(active.waiting())
        };
    }
    if matches!(request, MigrationRequest::Resume) {
        let Some(folder) = app
            .dialog()
            .file()
            .set_title("Choose the separate MDReader migration folder")
            .blocking_pick_folder()
        else {
            return Ok(reply("cancelled", None));
        };
        let picked = picked_root(folder.into_path().map_err(|_| Error::Invalid)?)?;
        if let Some((id, original, launch)) = workspace::legacy::observation(&picked.path)? {
            *pending = Some(Pending {
                id,
                windows: Retained {
                    root: original,
                    handoff: Default::default(),
                    original: None,
                    legacy: None,
                    observer: None,
                    reobserve: Some((picked, launch)),
                    preserved_draft: true,
                    draft: None,
                    live_controller: None,
                    controller_root: None,
                    legacy_imported: 0,
                    prepared: None,
                    rollback_pending: false,
                    controller: None,
                },
            });
            return pending.as_mut().ok_or(Error::Conflict)?.finish(app);
        }
        if workspace::legacy_selection(&picked.path)? {
            let (id, original, imported, controller_root, reobserve) =
                match workspace::controller::resume(&picked.path) {
                    Ok((id, original, imported)) => (id, original, imported, Some(picked), None),
                    Err(Error::ReconciliationRequired) => {
                        let (id, original, imported, launch) =
                            workspace::controller::observation(&picked.path)?;
                        (id, original, imported, None, Some((picked, launch)))
                    }
                    Err(error) => return Err(error),
                };
            *pending = Some(Pending {
                id,
                windows: Retained {
                    root: original,
                    handoff: Default::default(),
                    original: None,
                    legacy: None,
                    observer: None,
                    reobserve,
                    preserved_draft: false,
                    draft: None,
                    live_controller: None,
                    controller_root,
                    legacy_imported: imported,
                    prepared: None,
                    rollback_pending: false,
                    controller: None,
                },
            });
            return pending.as_mut().ok_or(Error::Conflict)?.finish(app);
        }
        let mut prepared = workspace::Prepared::resume(&picked.path)?;
        let root = prepared.original_root()?;
        let id = prepared.native_operation();
        picked.verify().map_err(|_| Error::Conflict)?;
        // Native state exists before any cooperating writer is dispatched.
        *pending = Some(Pending {
            id,
            windows: Retained {
                root,
                handoff: Default::default(),
                original: None,
                legacy: None,
                observer: None,
                reobserve: None,
                preserved_draft: false,
                draft: None,
                live_controller: None,
                controller_root: None,
                legacy_imported: 0,
                prepared: Some(prepared),
                rollback_pending: false,
                controller: None,
            },
        });
        return pending.as_mut().ok_or(Error::Conflict)?.finish(app);
    }
    let Some(folder) = app
        .dialog()
        .file()
        .set_title("Choose a closed MDReader profile — the original is retained")
        .blocking_pick_folder()
    else {
        return Ok(reply("cancelled", None));
    };
    let root = picked_root(folder.into_path().map_err(|_| Error::Invalid)?)?;
    let observer = if crate::profile_transition::offline::legacy_profile(&root)? {
        let Some(executable) = app
            .dialog()
            .file()
            .set_title("Choose the installed original MDReader.exe while it is running")
            .add_filter("MDReader application", &["exe"])
            .blocking_pick_file()
        else {
            return Ok(reply("cancelled", None));
        };
        Some(LegacyApplicationFence::capture(
            &root,
            &executable.into_path().map_err(|_| Error::Invalid)?,
        )?)
    } else {
        mdreader_profile::RootBinding::capture_private(&root.path)?;
        None
    };
    let current = crate::reader_profile::directory(app).map_err(|_| Error::Conflict)?;
    if root.path.starts_with(&current) || current.starts_with(&root.path) {
        return Err(Error::NotQuiescent);
    }
    *pending = Some(Pending {
        id: Uuid::new_v4(),
        windows: Retained {
            root,
            handoff: Default::default(),
            original: None,
            legacy: None,
            observer,
            reobserve: None,
            preserved_draft: false,
            draft: None,
            live_controller: None,
            controller_root: None,
            legacy_imported: 0,
            prepared: None,
            rollback_pending: false,
            controller: None,
        },
    });
    pending.as_mut().ok_or(Error::Conflict)?.finish(app)
}
