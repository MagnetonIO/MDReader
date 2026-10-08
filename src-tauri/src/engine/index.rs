use super::*;
const MODEL_LOAD_DEADLINE: Duration = Duration::from_secs(120);
fn publish(
    epoch: &Arc<Epoch>,
    workspace: &WorkspaceState,
    channel: &Channel<ImportProgress>,
    progress: ImportProgress,
) {
    record_status(epoch, &progress);
    // EPOCH-GUARDED: stream to the UI only while this epoch is current.
    if epoch.finish(workspace, ()).is_ok() {
        let _ = channel.send(progress);
    }
}

/// Validate an import `root` against `epoch`'s grant (containment). A foreign
/// path is a hard error; the engine always imports the epoch's own grant, never
/// an arbitrary caller-supplied path, so one workspace maps to exactly one index.
pub(super) fn validate_import_root(epoch: &Epoch, root: &str) -> Result<(), String> {
    epoch.validate_root()?;
    let candidate = resolve_request_root(epoch, root)?;
    if !candidate.starts_with(&epoch.root) {
        return Err(format!("path escapes the workspace: {root}"));
    }
    Ok(())
}

/// Resolve a webview-provided directory under a captured epoch. The exact lossy
/// display of the already-granted root is an identifier only: map it back to the
/// epoch's native `PathBuf` rather than treating that display text as fresh
/// filesystem authority. Subdirectory requests retain the ordinary
/// canonicalize-and-contain check.
pub(super) fn resolve_request_root(epoch: &Epoch, requested: &str) -> Result<PathBuf, String> {
    if display_names_root(&epoch.root, requested) {
        return Ok(epoch.root.clone());
    }
    resolve_dir(&epoch.root, Path::new(requested))
}

pub(super) fn display_names_root(root: &Path, requested: &str) -> bool {
    root.to_string_lossy() == requested
}

pub async fn start_import(
    app: AppHandle,
    root: String,
    on_event: Channel<ImportProgress>,
    workspace: State<'_, WorkspaceState>,
) -> Result<ImportProgress, String> {
    let epoch = workspace.current().ok_or("no workspace has been opened")?;
    validate_import_root(&epoch, &root)?;
    let reader = app.state::<crate::reader::ReaderBackend>().inner().clone();
    let (owns_progress, source) = queue_reader_index(&reader, &epoch).await?;
    if !owns_progress {
        return finish_import_progress(&epoch, &workspace, epoch.status());
    }
    let _guard = ActiveGuard {
        epoch: Arc::clone(&epoch),
    };
    let outcome:Result<ImportProgress,String>=async {
        let deadline=std::time::Instant::now()+MODEL_LOAD_DEADLINE;
        loop {
            if !workspace.is_current(&epoch) { return Ok(epoch.status()); }
            epoch.validate_root()?;
            let job:Option<mdreader_core::IndexJob>=reader.invoke(mdreader_service::Request::JobStatus {source_id:source.clone()}).await?;
            let job=job.ok_or("Index job disappeared")?;
            let progress=ImportProgress {
                phase:match job.phase.as_str() {"queued"|"indexing"=>"importing","embedding"=>"reembedding",phase=>phase}.into(),
                files_done:job.documents as u32,files_total:job.documents as u32,chunks_embedded:job.embedded as u32,error:job.error,
            };
            publish(&epoch,&workspace,&on_event,progress.clone());
            if ["done","error","model-unavailable"].contains(&progress.phase.as_str()) {return Ok(progress);}
            if std::time::Instant::now()>=deadline {
                let mut unavailable=progress;
                unavailable.phase="model-unavailable".into();
                unavailable.error=Some("Indexing continues in the reader service; local semantic search is not ready yet".into());
                return Ok(unavailable);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }.await;
    let progress = outcome.unwrap_or_else(|error| ImportProgress::failed(0, error));
    publish(&epoch, &workspace, &on_event, progress.clone());
    finish_import_progress(&epoch, &workspace, progress)
}

/// Coalesce progress listeners, never source-change notifications. Every call
/// durably advances the service job generation so a change during an active
/// pass cannot be lost when that older pass finishes.
pub(crate) async fn queue_reader_index(
    reader: &crate::reader::ReaderBackend,
    epoch: &Arc<Epoch>,
) -> Result<(bool, String), String> {
    epoch.validate_root()?;
    let source = reader.admit_async(epoch.source_root()).await?;
    let _: serde_json::Value = reader
        .invoke(mdreader_service::Request::StartIndex {
            source_id: source.clone(),
        })
        .await?;
    Ok((claim_active(epoch)?, source))
}

/// Preserve the intentionally quiet cancellation of a superseded import while
/// surfacing replacement of the current epoch's admitted root.
fn finish_import_progress(
    epoch: &Arc<Epoch>,
    workspace: &WorkspaceState,
    progress: ImportProgress,
) -> Result<ImportProgress, String> {
    match epoch.finish(workspace, progress) {
        Ok(progress) => Ok(progress),
        Err(error) if error.is_superseded() => Ok(epoch.status()),
        Err(error) => Err(error.to_string()),
    }
}

/// The latest import status for the CURRENT workspace epoch — for the StatusBar's
/// initial render and any poll. A superseded epoch's status is never read, so a
/// switch leaves this reading `idle` until the new epoch's import publishes.
pub fn import_status(workspace: State<'_, WorkspaceState>) -> Result<ImportProgress, String> {
    import_status_guarded(&workspace, || {})
}

/// Poll the captured epoch and validate both epoch and source identity at the
/// completion boundary. A superseding workspace switch intentionally yields
/// idle for the new epoch; replacement of the current epoch's admitted root is
/// an explicit refusal.
pub(super) fn import_status_guarded(
    workspace: &WorkspaceState,
    gate: impl FnOnce(),
) -> Result<ImportProgress, String> {
    match workspace.current() {
        Some(epoch) => {
            let status = epoch.status();
            gate();
            // EPOCH-GUARDED
            match epoch.finish(workspace, status) {
                Ok(status) => Ok(status),
                Err(error) if error.is_superseded() => Ok(ImportProgress::default()),
                Err(error) => Err(error.to_string()),
            }
        }
        None => {
            gate();
            Ok(ImportProgress::default())
        }
    }
}
