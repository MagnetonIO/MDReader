//! Explicit closed-reader Windows export. The native selection and dispatched
//! service scope stay retained across retries and Settings remounts.
use super::super::{owner::ClosedOwner, shutdown_handoff::ShutdownHandoff};
use super::*;
mod exporting;
mod opening;
mod pending;
mod returning;
use super::super::windows_workspace as workspace;
use mdreader_profile::legacy_windows::{ClosedOriginal, LegacyApplicationFence};

pub(super) struct Retained {
    root: SourceRoot,
    handoff: ShutdownHandoff,
    original: Option<ClosedOwner>,
    legacy: Option<ClosedOriginal>,
    observer: Option<LegacyApplicationFence>,
    reobserve: Option<(SourceRoot, mdreader_profile::legacy_windows::LaunchRecord)>,
    preserved_draft: bool,
    draft: Option<workspace::legacy::Draft>,
    live_controller: Option<workspace::controller::LiveController>,
    controller_root: Option<SourceRoot>,
    legacy_imported: u32,
    prepared: Option<super::super::windows_workspace::Prepared>,
    rollback_pending: bool,
    controller: Option<std::process::Child>,
}
pub(super) fn execute(
    app: &AppHandle,
    request: MigrationRequest,
    pending: &mut Option<Pending>,
) -> Result<MigrationReply> {
    if matches!(
        request,
        MigrationRequest::Preview | MigrationRequest::Resume
    ) {
        return opening::execute(app, request, pending);
    }
    if matches!(request, MigrationRequest::ResumeReturn) {
        return returning::resume(app, pending);
    }
    let operation = match &request {
        MigrationRequest::Retry { operation_id }
        | MigrationRequest::Export { operation_id }
        | MigrationRequest::Close { operation_id }
        | MigrationRequest::Prepare { operation_id, .. }
        | MigrationRequest::Activate { operation_id }
        | MigrationRequest::Rollback { operation_id } => operation_id,
        _ => return Err(Error::Unsupported),
    };
    let operation = Uuid::parse_str(operation).map_err(|_| Error::Invalid)?;
    let active = pending.as_mut().ok_or(Error::Conflict)?;
    if active.id != operation {
        return Err(Error::Conflict);
    }
    if (active.windows.rollback_pending || active.windows.controller.is_some())
        && !matches!(
            request,
            MigrationRequest::Retry { .. }
                | MigrationRequest::Rollback { .. }
                | MigrationRequest::Close { .. }
        )
    {
        return Err(Error::ReconciliationRequired);
    }
    match request {
        MigrationRequest::Retry { .. } => {
            if active.windows.rollback_pending || active.windows.controller.is_some() {
                let result = returning::execute(app, active)?;
                if result.state == "rollback_launch_requested" {
                    let migration = active
                        .windows
                        .prepared
                        .as_ref()
                        .ok_or(Error::Conflict)?
                        .launch_directory();
                    *pending = None;
                    returning::launch(migration)?;
                }
                return Ok(result);
            }
            if active.windows.prepared.is_some() {
                if active.windows.handoff.issued() {
                    let owner = ClosedOwner::acquire_retaining(
                        &active.windows.root.path,
                        &mut active.windows.handoff,
                    )?;
                    active.windows.root.verify().map_err(|_| Error::Conflict)?;
                    active.windows.original = Some(owner);
                }
                active
                    .windows
                    .prepared
                    .as_mut()
                    .ok_or(Error::Conflict)?
                    .reconcile_handoff()?;
            }
            active.finish(app)
        }
        MigrationRequest::Close { .. } => {
            if active.handoff_issued()
                || active.windows.rollback_pending
                || active.windows.controller.is_some()
                || active.windows.live_controller.is_some()
            {
                return Err(Error::ReconciliationRequired);
            }
            *pending = None;
            Ok(reply("closed", Some(operation)))
        }
        MigrationRequest::Rollback { .. } => {
            let result = returning::execute(app, active)?;
            if result.state == "rollback_launch_requested" {
                let migration = active
                    .windows
                    .prepared
                    .as_ref()
                    .ok_or(Error::Conflict)?
                    .launch_directory();
                // Close every original/candidate lock before child setup.
                *pending = None;
                returning::launch(migration)?;
            }
            Ok(result)
        }
        MigrationRequest::Activate { .. } => {
            let prepared = active.windows.prepared.as_mut().ok_or(Error::Conflict)?;
            if prepared.state()? != "launch_requested" {
                if let Some(original) = active.windows.legacy.as_mut() {
                    prepared.activate(original)?;
                } else {
                    if active.windows.original.is_none() {
                        let owner = ClosedOwner::acquire_retaining(
                            &active.windows.root.path,
                            &mut active.windows.handoff,
                        )?;
                        if owner.binding()
                            != &mdreader_profile::RootBinding::capture_private(
                                &active.windows.root.path,
                            )?
                        {
                            return Err(Error::Conflict);
                        }
                        active.windows.original = Some(owner);
                    }
                    prepared.activate(active.windows.original.as_mut().ok_or(Error::Conflict)?)?;
                }
            }
            let directory = prepared.launch_directory();
            let selected = super::super::windows_workspace::selected_for_launch(&directory)?;
            crate::reader_profile::remember_imported(app, &selected.path)?;
            if let Some(original) = active.windows.legacy.take() {
                let imported = prepared.imported()?;
                let root = SourceRoot::capture(directory.clone()).map_err(|_| Error::Conflict)?;
                let prepared = active.windows.prepared.take().ok_or(Error::Conflict)?;
                active.windows.live_controller = Some(workspace::controller::start(
                    app.clone(),
                    original,
                    prepared,
                )?);
                active.windows.controller_root = Some(root);
                active.windows.legacy_imported = imported;
            }
            let executable = std::env::current_exe().map_err(|_| Error::Storage)?;
            std::process::Command::new(executable)
                .arg("--reader-selection")
                .arg(directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|_| Error::ReconciliationRequired)?;
            active.finish(app)
        }
        MigrationRequest::Prepare { documents, .. } => {
            if active.windows.prepared.is_some() {
                return Err(Error::Conflict);
            }
            if documents.is_empty() || documents.len() > 1000 {
                return Err(Error::Bounds);
            }
            let mut seen = std::collections::HashSet::new();
            for selection in &documents {
                if (active.windows.legacy.is_none()
                    && Uuid::parse_str(&selection.document_id).is_err())
                    || selection.document_id.is_empty()
                    || selection.document_id.len() > 16384
                    || selection.revision.len() != 64
                    || !selection
                        .revision
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                    || !seen.insert(&selection.document_id)
                {
                    return Err(Error::Invalid);
                }
            }
            if let Some(original) = active.windows.legacy.as_mut() {
                let draft = active.windows.draft.as_mut().ok_or(Error::Conflict)?;
                active.windows.prepared = Some(draft.finish(original, &documents)?);
                active.windows.draft = None;
                return active.finish(app);
            }
            let owner = active.windows.original.as_mut().ok_or(Error::Conflict)?;
            let Some(parent) = selected_parent(app, owner.path())? else {
                return Ok(reply("cancelled", Some(operation)));
            };
            let prepared = super::super::windows_workspace::Prepared::prepare(
                owner,
                &parent.path,
                &documents,
                operation,
            )?;
            parent.verify().map_err(|_| Error::Conflict)?;
            active.windows.prepared = Some(prepared);
            active.finish(app)
        }
        MigrationRequest::Export { .. } => exporting::execute(app, active),
        _ => Err(Error::Unsupported),
    }
}
