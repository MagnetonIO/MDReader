use super::*;
pub(super) fn execute(
    app: &AppHandle,
    request: MigrationRequest,
    pending: &mut Option<Pending>,
) -> Result<MigrationReply> {
    if matches!(request, MigrationRequest::Resume) {
        if pending.is_some() {
            return Err(Error::Conflict);
        }
        let Some(folder) = app
            .dialog()
            .file()
            .set_title("Reopen a retained MDReader migration folder")
            .blocking_pick_folder()
        else {
            return Ok(reply("cancelled", None));
        };
        let root = picked_root(folder.into_path().map_err(|_| Error::Invalid)?)?;
        if root
            .path
            .join("reader-profile.json")
            .try_exists()
            .map_err(|_| Error::Storage)?
        {
            let archive = super::archive_import::ArchiveImport::resume(&root.path)?;
            let coverage = archive.coverage()?;
            let imported = archive.imported()?;
            let id = Uuid::new_v4();
            *pending = Some(Pending {
                id,
                original: None,
                legacy: None,
                workspace: None,
                archive: Some(archive),
            });
            let mut result = reply("prepared", Some(id));
            result.imported = imported;
            result.origin = Some("legacy_catdb");
            result.coverage = Some(coverage);
            return Ok(result);
        }
        let mut workspace = Workspace::resume(&root.path)?;
        root.verify().map_err(|_| Error::Conflict)?;
        let state = workspace.status()?;
        let imported = workspace.imported()?;
        let id = Uuid::new_v4();
        *pending = Some(Pending {
            id,
            original: None,
            legacy: None,
            archive: None,
            workspace: Some(workspace),
        });
        let mut result = reply(state, Some(id));
        result.imported = imported;
        return Ok(result);
    }
    if matches!(request, MigrationRequest::Preview) {
        if pending.is_some() {
            return Err(Error::Conflict);
        }
        let Some(folder) = app
            .dialog()
            .file()
            .set_title("Choose a closed MDReader profile — the original is retained")
            .blocking_pick_folder()
        else {
            return Ok(reply("cancelled", None));
        };
        let selected = picked_root(folder.into_path().map_err(|_| Error::Invalid)?)?;
        let current = crate::reader_profile::directory(app)
            .map_err(|_| Error::Conflict)?
            .canonicalize()
            .map_err(|_| Error::Conflict)?;
        if selected.path.starts_with(&current) || current.starts_with(&selected.path) {
            return Err(Error::NotQuiescent);
        }
        if selected
            .path
            .join("manifest.json")
            .try_exists()
            .map_err(|_| Error::Storage)?
        {
            let archive = super::archive_import::ArchiveImport::preview(&selected.path)?;
            let documents = archive.documents();
            let coverage = archive.coverage()?;
            let id = Uuid::new_v4();
            *pending = Some(Pending {
                id,
                original: None,
                legacy: None,
                workspace: None,
                archive: Some(archive),
            });
            let mut result = reply("preview", Some(id));
            result.documents = documents;
            result.origin = Some("legacy_catdb");
            result.coverage = Some(coverage);
            return Ok(result);
        }
        if super::legacy_source::recognized(&selected)? {
            let legacy = super::legacy_source::LegacySource::capture(selected)?;
            let id = Uuid::new_v4();
            *pending = Some(Pending {
                id,
                original: None,
                legacy: Some(legacy),
                archive: None,
                workspace: None,
            });
            let mut result = reply("close_original_required", Some(id));
            result.origin = Some("legacy_catdb");
            return Ok(result);
        }
        let original = ClosedOwner::acquire(&selected.path)?;
        // Reject unknown stores BEFORE promising a usable conversion. Known
        // library SQLite is archived in full, including unconverted records.
        super::super::super::backup_inventory::selections(original.path(), &Default::default())?;
        let store = ReaderStore::open_durable_readonly(&original.path().join("reader-v1"))
            .map_err(|_| Error::Unsupported)?;
        let documents = conversion::documents(&store)?
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
        let id = Uuid::new_v4();
        *pending = Some(Pending {
            id,
            original: Some(original),
            legacy: None,
            archive: None,
            workspace: None,
        });
        let mut result = reply("preview", Some(id));
        result.origin = Some("reader");
        result.documents = documents;
        return Ok(result);
    }
    let operation = match &request {
        MigrationRequest::Retry { operation_id }
        | MigrationRequest::Export { operation_id }
        | MigrationRequest::Prepare { operation_id, .. }
        | MigrationRequest::Activate { operation_id }
        | MigrationRequest::Close { operation_id }
        | MigrationRequest::Rollback { operation_id } => operation_id,
        MigrationRequest::Preview | MigrationRequest::Resume => return Err(Error::Invalid),
    };
    let id = Uuid::parse_str(operation).map_err(|_| Error::Invalid)?;
    let active = pending.as_mut().ok_or(Error::Conflict)?;
    if id != active.id {
        return Err(Error::Conflict);
    }
    match request {
        MigrationRequest::Retry { .. } => {
            if active.archive.is_none() {
                let legacy = active.legacy.as_mut().ok_or(Error::Conflict)?;
                match legacy.snapshot() {
                    Ok(archive) => active.archive = Some(archive),
                    Err(Error::NotQuiescent) => {
                        let mut result = reply("close_original_required", Some(id));
                        result.origin = Some("legacy_catdb");
                        return Ok(result);
                    }
                    Err(error) => return Err(error),
                }
            }
            let archive = active.archive.as_ref().ok_or(Error::Conflict)?;
            let mut result = reply("preview", Some(id));
            result.origin = Some("legacy_catdb");
            result.documents = archive.documents();
            result.coverage = Some(archive.coverage()?);
            Ok(result)
        }
        MigrationRequest::Close { .. } => {
            *pending = None;
            Ok(reply("closed", Some(id)))
        }
        MigrationRequest::Export { .. } => {
            if let Some(archive) = &active.archive {
                let Some(parent) = selected_parent(app, archive.path())? else {
                    return Ok(reply("cancelled", Some(id)));
                };
                archive.export(&parent.path)?;
                parent.verify().map_err(|_| Error::Conflict)?;
                let mut result = reply("exported", Some(id));
                result.origin = Some("legacy_catdb");
                result.coverage = Some(archive.coverage()?);
                return Ok(result);
            }
            let original = active.original.as_mut().ok_or(Error::Conflict)?;
            let Some(parent) = selected_parent(app, original.path())? else {
                return Ok(reply("cancelled", Some(id)));
            };
            super::workspace::export(original, &parent.path)?.revalidate(&Default::default())?;
            parent.verify().map_err(|_| Error::Conflict)?;
            Ok(reply("exported", Some(id)))
        }
        MigrationRequest::Prepare { documents, .. } => {
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
            if let Some(archive) = &mut active.archive {
                let Some(parent) = selected_parent(app, archive.path())? else {
                    return Ok(reply("cancelled", Some(id)));
                };
                let imported = archive.prepare(&parent.path, &documents)?;
                parent.verify().map_err(|_| Error::Conflict)?;
                let mut result = reply("prepared", Some(id));
                result.origin = Some("legacy_catdb");
                result.coverage = Some(archive.coverage()?);
                result.imported = imported;
                return Ok(result);
            }
            let original = active.original.as_ref().ok_or(Error::Conflict)?;
            let Some(parent) = selected_parent(app, original.path())? else {
                return Ok(reply("cancelled", Some(id)));
            };
            let original = active.original.take().ok_or(Error::Conflict)?;
            // On partial failure the archive/candidate remain in the selected
            // destination; no reset, deletion, retry or source regrant occurs.
            active.workspace = Some(Workspace::prepare(original, &parent.path, &documents)?);
            parent.verify().map_err(|_| Error::Conflict)?;
            let mut result = reply("prepared", Some(id));
            result.imported = active
                .workspace
                .as_mut()
                .ok_or(Error::Conflict)?
                .imported()?;
            Ok(result)
        }
        MigrationRequest::Activate { .. } | MigrationRequest::Rollback { .. } => {
            let rollback = matches!(request, MigrationRequest::Rollback { .. });
            if let Some(archive) = &mut active.archive {
                if rollback {
                    let legacy = active.legacy.as_mut().ok_or(Error::Unsupported)?;
                    let launch = legacy.launch_identity()?;
                    legacy.original_unchanged(archive.path())?;
                    archive.preserve_candidate()?;
                    legacy.original_unchanged(archive.path())?;
                    launch.verify()?;
                    crate::reader_profile::forget_imported(app, archive.selected_profile()?)?;
                    // Release exact original owner locks before launching the
                    // retained old application. A new reader binary never
                    // opens or migrates the original CatDB profile.
                    active.legacy.take();
                    launch.launch()?;
                    let mut result = reply("rollback_launch_requested", Some(id));
                    result.origin = Some("legacy_catdb");
                    result.coverage = Some(archive.coverage()?);
                    result.imported = archive.imported()?;
                    return Ok(result);
                }
                crate::reader_profile::remember_imported(app, archive.selected_profile()?)?;
                archive.launch()?;
                let mut result = reply(
                    if active.legacy.is_some() {
                        "activated"
                    } else {
                        "launch_requested"
                    },
                    Some(id),
                );
                result.origin = Some("legacy_catdb");
                result.coverage = Some(archive.coverage()?);
                result.imported = archive.imported()?;
                return Ok(result);
            }
            let workspace = active.workspace.as_mut().ok_or(Error::Conflict)?;
            let directory = workspace.launch_directory();
            if rollback {
                workspace.rollback()?;
            } else {
                workspace.activate()?;
            }
            // Actual closed-owner guards end before the independently owned
            // selected profile starts. Current GUI/drafts remain untouched.
            active.workspace.take();
            let executable = std::env::current_exe().map_err(|_| Error::Storage)?;
            std::process::Command::new(executable)
                .arg("--reader-selection")
                .arg(directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|_| Error::ReconciliationRequired)?;
            Ok(reply(
                if rollback {
                    "rollback_launch_requested"
                } else {
                    "launch_requested"
                },
                Some(id),
            ))
        }
        MigrationRequest::Preview | MigrationRequest::Resume => Err(Error::Invalid),
    }
}
