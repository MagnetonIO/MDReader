//! Bind device receipts to the explicitly composed cache directory, retaining
//! each existing child grant without inventing parent history for old imports.
use super::*;
use std::path::Path;

pub fn document_in(
    store: &ReaderStore,
    directory: &Path,
    expected: &Receipt,
) -> Result<DeviceDocument> {
    let document = document(store, &expected.document_id)?;
    if receipt(&document) != *expected {
        return Err(Error::Conflict);
    }
    let metadata = std::fs::symlink_metadata(directory).map_err(|_| Error::AuthorityChanged)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::AuthorityChanged);
    }
    let directory = directory
        .canonicalize()
        .map_err(|_| Error::AuthorityChanged)?;
    let child = store.device_source_root(&document.source_id)?;
    if child.path.parent() != Some(directory.as_path())
        || child.path.file_name().and_then(|name| name.to_str())
            != Some(expected.document_id.as_str())
    {
        return Err(Error::AuthorityChanged);
    }
    child.verify()?;
    Ok(document)
}

pub fn read_in(store: &ReaderStore, directory: &Path, expected: &Receipt) -> Result<DeviceRead> {
    let fence = read_fence(store)?;
    let before = document_in(store, directory, expected)?;
    let body = read(store, expected)?;
    let after = document_in(store, directory, expected)?;
    if before != after || read_fence(store)? != fence {
        return Err(Error::Conflict);
    }
    Ok(body)
}

pub fn page_in(
    store: &ReaderStore,
    directory: &Path,
    after: Option<&str>,
    expected: Option<&Snapshot>,
    limit: usize,
) -> Result<Page> {
    let page = page(store, after, expected, limit)?;
    for document in &page.documents {
        document_in(store, directory, &receipt(document))?;
    }
    if snapshot(store)? != page.snapshot {
        return Err(Error::Conflict);
    }
    Ok(page)
}

/// Readonly current pin/line CAS identity within the explicitly configured owner.
pub fn state_in(
    store: &ReaderStore,
    directory: &Path,
    expected: &Receipt,
) -> Result<super::super::state::DocumentState> {
    let document = document_in(store, directory, expected)?;
    let db = store.device_connection();
    if readonly::namespace(db)? != Some(readonly::Namespace::Current) {
        return Err(Error::UnsupportedSchema);
    }
    readonly::validate_rows(db, readonly::Namespace::Current)?;
    let state = super::super::state::load(db, &expected.document_id, true)?;
    if state.document != document {
        return Err(Error::Conflict);
    }
    Ok(state)
}
