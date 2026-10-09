//! Explicit closed-reader Windows export. The native selection and dispatched
//! service scope stay retained across retries and Settings remounts.
use super::super::{owner::ClosedOwner, shutdown_handoff::ShutdownHandoff};
use super::*;

pub(super) struct Retained {
    root: SourceRoot,
    handoff: ShutdownHandoff,
    original: Option<ClosedOwner>,
    prepared: Option<super::super::windows_workspace::Prepared>,
}
impl Pending {
    pub(super) fn handoff_issued(&self) -> bool {
        self.windows.handoff.issued()
    }
    pub(super) fn handoff_failure(&self) -> Option<&'static str> {
        self.windows.handoff.failure_kind()
    }
    fn waiting(&self) -> MigrationReply {
        let mut result = reply(
            match self.handoff_failure() {
                Some("background_service_unsupported") => "background_service_unsupported",
                _ if self.handoff_issued() => "service_handoff_pending",
                _ => "close_original_required",
            },
            Some(self.id),
        );
        result.origin = Some("reader");
        result
    }
    fn finish(&mut self) -> Result<MigrationReply> {
        if let Some(prepared) = self.windows.prepared.as_mut() {
            let mut result = reply(prepared.state()?, Some(self.id));
            result.origin = Some("reader");
            result.imported = prepared.imported()?;
            return Ok(result);
        }
        self.windows.root.verify().map_err(|_| Error::Conflict)?;
        if self.windows.original.is_none() {
            let acquired =
                ClosedOwner::acquire_retaining(&self.windows.root.path, &mut self.windows.handoff);
            match acquired {
                Ok(owner) => self.windows.original = Some(owner),
                Err(Error::NotQuiescent | Error::ReconciliationRequired) => {
                    return Ok(self.waiting())
                }
                Err(error) => return Err(error),
            }
        }
        let owner = self.windows.original.as_mut().ok_or(Error::Conflict)?;
        super::super::super::backup_inventory::selections(owner.path(), &Default::default())?;
        let store = owner.store()?;
        let documents = super::super::conversion::documents(&store)?
            .into_iter()
            .map(|document| Document {
                document_id: document.id,
                name: document.name,
                revision: document.revision,
                bytes: document.bytes,
                pinned: document.pinned,
                line: document.line,
            })
            .collect();
        owner.revalidate()?;
        let mut result = reply("preview", Some(self.id));
        result.origin = Some("reader");
        result.documents = documents;
        Ok(result)
    }
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
        if let Some(active) = pending.as_mut() {
            return if active.windows.original.is_some() {
                active.finish()
            } else {
                Ok(active.waiting())
            };
        }
        // Resume of a prepared candidate will be added with the durable selector;
        // it must not silently repick or treat an export as an active candidate.
        if matches!(request, MigrationRequest::Resume) {
            return Err(Error::Unsupported);
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
        mdreader_profile::RootBinding::capture_private(&root.path)?;
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
                prepared: None,
            },
        });
        return pending.as_mut().ok_or(Error::Conflict)?.finish();
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
    match request {
        MigrationRequest::Retry { .. } => active.finish(),
        MigrationRequest::Close { .. } => {
            if active.handoff_issued() {
                return Err(Error::ReconciliationRequired);
            }
            *pending = None;
            Ok(reply("closed", Some(operation)))
        }
        MigrationRequest::Activate { .. } => {
            let prepared = active.windows.prepared.as_mut().ok_or(Error::Conflict)?;
            let original = active.windows.original.as_mut().ok_or(Error::Conflict)?;
            prepared.activate(original)?;
            let directory = prepared.launch_directory();
            let selected = super::super::windows_workspace::selected_for_launch(&directory)?;
            crate::reader_profile::remember_imported(app, &selected.path)?;
            let executable = std::env::current_exe().map_err(|_| Error::Storage)?;
            std::process::Command::new(executable)
                .arg("--reader-selection")
                .arg(directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|_| Error::ReconciliationRequired)?;
            active.finish()
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
                if Uuid::parse_str(&selection.document_id).is_err()
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
            active.finish()
        }
        MigrationRequest::Export { .. } => {
            let owner = active.windows.original.as_mut().ok_or(Error::Conflict)?;
            let Some(parent) = selected_parent(app, owner.path())? else {
                return Ok(reply("cancelled", Some(operation)));
            };
            let name = format!("MDReader-archive-{}", Uuid::new_v4());
            let selected = mdreader_capture::SelectedDirectory::open(&parent.path)
                .map_err(|_| Error::Conflict)?;
            let private = selected
                .create_private_child(Path::new(&name))
                .map_err(|_| Error::Storage)?;
            let limits = mdreader_profile::Limits::default();
            let choices = super::super::super::backup_inventory::selections(owner.path(), &limits)?;
            let inventory = mdreader_profile::Inventory::capture(owner.path(), &choices, &limits)?;
            let snapshot = mdreader_profile::stage(
                &mut super::super::super::BorrowedFreeze { frozen: owner },
                inventory,
                &parent.path.join(name).join("original"),
                &limits,
            )?;
            snapshot.revalidate(&limits)?;
            private.verify().map_err(|_| Error::Conflict)?;
            selected.verify().map_err(|_| Error::Conflict)?;
            parent.verify().map_err(|_| Error::Conflict)?;
            let mut result = reply("exported", Some(operation));
            result.origin = Some("reader");
            Ok(result)
        }
        _ => Err(Error::Unsupported),
    }
}
