use super::index::display_names_root;
use super::*;
pub fn list_workspace_files(
    root: String,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, crate::reader::ReaderBackend>,
) -> Result<WorkspaceListing, ListingError> {
    list_reader_files(&reader, &workspace, &root)
}

pub(crate) fn list_reader_files(
    reader: &crate::reader::ReaderBackend,
    workspace: &WorkspaceState,
    root: &str,
) -> Result<WorkspaceListing, ListingError> {
    let epoch = workspace
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    epoch.validate_root()?;
    let authority = epoch.source_root();
    let relative = if display_names_root(&authority.path, root) {
        PathBuf::new()
    } else {
        authority
            .relative_path(Path::new(root))
            .map_err(|e| ListingError::from(e.to_string()))?
    };
    authority
        .verify_directory_relative(&relative)
        .map_err(|e| ListingError::from_source(&authority.path.join(&relative), &e))?;
    let source = reader.admit(&authority)?;
    let status: mdreader_core::IndexStatus = reader.typed(mdreader_service::Request::Status {
        sources: vec![source.clone()],
    })?;
    let job: Option<mdreader_core::IndexJob> =
        reader.typed(mdreader_service::Request::JobStatus {
            source_id: source.clone(),
        })?;
    let indexed = status.documents > 0
        || job.is_some_and(|j| {
            ["loading", "embedding", "done", "model-unavailable"].contains(&j.phase.as_str())
        });
    let listing = if indexed {
        let documents: Vec<mdreader_core::ListedDocument> =
            reader.typed(mdreader_service::Request::ListDocuments { source_id: source })?;
        WorkspaceListing {
            from_index: true,
            entries: documents
                .into_iter()
                .filter(|doc| Path::new(&doc.path).starts_with(&relative))
                .map(|doc| {
                    let metadata = authority.metadata_relative(Path::new(&doc.path)).ok();
                    file_entry(&authority.path.join(&doc.path), false, metadata.as_ref())
                })
                .collect(),
        }
    } else {
        WorkspaceListing {
            from_index: false,
            entries: fs_listing(&authority, &relative)?,
        }
    };
    epoch
        .finish(workspace, listing)
        .map_err(|e| ListingError::from(e.to_string()))
}
pub(super) fn fs_listing(
    source: &SourceRoot,
    requested: &Path,
) -> Result<Vec<FileEntry>, ListingError> {
    let kept: HashSet<String> = scan_source_root(source, &scan_options(true))
        .map_err(|err| ListingError::from_walk(&source.path.join(requested), &err))?
        .into_iter()
        .filter(|file| Path::new(&file.rel).starts_with(requested))
        .map(|file| file.rel)
        .collect();
    let all = scan_source_root(source, &scan_options(false))
        .map_err(|err| ListingError::from_walk(&source.path.join(requested), &err))?;

    let mut entries = Vec::new();
    for file in all
        .into_iter()
        .filter(|file| Path::new(&file.rel).starts_with(requested))
    {
        let ignored = !kept.contains(&file.rel);
        let metadata = file.metadata().ok();
        entries.push(file_entry(&file.abs, ignored, metadata.as_ref()));
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

/// Assemble one [`FileEntry`] from an absolute path, statting it best-effort.
fn file_entry(abs: &Path, ignored: bool, metadata: Option<&std::fs::Metadata>) -> FileEntry {
    FileEntry {
        path: abs.to_string_lossy().into_owned(),
        name: abs
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        dir: abs
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default(),
        mtime: mtime_millis(metadata.and_then(|value| value.modified().ok())),
        size: metadata.map(|value| value.len() as f64).unwrap_or(0.0),
        ignored,
    }
}

// --------------------------------------------------------------------------
pub(super) fn wikilink_guard(epoch: &Epoch, from_doc: &str) -> Result<(), String> {
    if !from_doc.is_empty() {
        resolve_within(&epoch.root, Path::new(from_doc))?;
    }
    Ok(())
}

/// Resolve a `[[wikilink]]` against the index. `from_doc` is the absolute path of
/// the document the link sits in — it must live inside the granted workspace;
/// `target` is the raw link text; `heading` (optional) is the `#heading` suffix,
/// resolved to a source line in the target document. Matching is filename-first
/// (stem, case-insensitive), then document title; several matches yield
/// `ambiguous` with the candidate set. Captures the epoch once (engine + root
/// consistent) and re-checks it is current AT THE RETURN — after the heading
/// lookup and result construction — so a switch mid-resolution can never surface
/// a previous workspace's document.
pub fn resolve_wikilink(
    from_doc: String,
    target: String,
    heading: Option<String>,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, crate::reader::ReaderBackend>,
) -> Result<WikiResolution, String> {
    let epoch = workspace.current().ok_or("no workspace has been opened")?;
    epoch.validate_root()?;
    wikilink_guard(&epoch, &from_doc)?;
    let source = reader.admit(&epoch.source_root())?;
    let from = if from_doc.is_empty() {
        "__root__.md".into()
    } else {
        epoch
            .source_root()
            .relative_path(Path::new(&from_doc))
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/")
    };
    let target = if let Some(heading) = heading {
        format!("{}#{heading}", target.split('#').next().unwrap_or(""))
    } else {
        target
    };
    let references: Vec<mdreader_core::Reference> =
        reader.typed(mdreader_service::Request::Resolve {
            source_id: source,
            from,
            target,
        })?;
    let resolution = match references.as_slice() {
        [] => WikiResolution::unresolved(),
        [reference] => WikiResolution::resolved(
            epoch
                .root
                .join(&reference.path)
                .to_string_lossy()
                .into_owned(),
            basename(&reference.path),
            Some(reference.line),
        ),
        _ => WikiResolution {
            status: "ambiguous".into(),
            path: None,
            name: None,
            line: None,
            candidates: references
                .into_iter()
                .map(|reference| WikiCandidate {
                    path: epoch
                        .root
                        .join(&reference.path)
                        .to_string_lossy()
                        .into_owned(),
                    name: basename(&reference.path),
                })
                .collect(),
        },
    };
    match epoch.finish(&workspace, resolution) {
        Ok(value) => Ok(value),
        Err(error) if error.is_superseded() => Ok(WikiResolution::unresolved()),
        Err(error) => Err(error.to_string()),
    }
}

fn scan_options(respect_gitignore: bool) -> ScanOptions {
    ScanOptions {
        include_mdx: false,
        max_file_size: 2 * 1024 * 1024,
        respect_gitignore,
    }
}
fn basename(value: &str) -> String {
    value.rsplit(['/', '\\']).next().unwrap_or(value).into()
}
