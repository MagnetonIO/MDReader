//! Project a durable exact import reservation under private NTFS capabilities.
//! No directory flush claim: SQLite retains original bytes before file creation.
use super::*;
use mdreader_capture::{same_open_file, PrivateDirectory};
use std::io::{Seek, SeekFrom};

const BODY: &str = "document.md";

pub(super) fn private_directory(path: &Path) -> Result<()> {
    PrivateDirectory::create(path)
        .map_err(|_| Error::AuthorityChanged)?
        .verify()
        .map_err(|_| Error::AuthorityChanged)
}

fn parent(directory: &Path, intent: &ImportIntent) -> Result<SourceRoot> {
    uuid::Uuid::parse_str(&intent.id).map_err(|_| Error::InvalidSelection)?;
    let identity =
        serde_json::from_str(&intent.parent_identity).map_err(|_| Error::AuthorityChanged)?;
    let root = SourceRoot::restore(directory.to_path_buf(), identity)?;
    PrivateDirectory::open(&root.path)
        .map_err(|_| Error::AuthorityChanged)?
        .verify()
        .map_err(|_| Error::AuthorityChanged)?;
    Ok(root)
}

pub(super) fn write_import(
    store: &mut ReaderStore,
    parent: &SourceRoot,
    intent: &mut ImportIntent,
    content: &str,
) -> Result<()> {
    let bytes = super::windows_body::load(store, intent)?;
    if content.as_bytes() != bytes {
        return Err(Error::Conflict);
    }
    let _ = materialize(store, parent, intent, &bytes)?;
    Ok(())
}

fn materialize(
    store: &mut ReaderStore,
    parent: &SourceRoot,
    intent: &mut ImportIntent,
    bytes: &[u8],
) -> Result<(SourceRoot, PrivateDirectory, fs::File)> {
    parent.verify()?;
    if serde_json::to_string(&parent.identity)? != intent.parent_identity {
        return Err(Error::AuthorityChanged);
    }
    let private_parent =
        PrivateDirectory::open(&parent.path).map_err(|_| Error::AuthorityChanged)?;
    let private = if intent.root_identity.is_some() {
        let root = intent_root(&parent.path, intent)?;
        PrivateDirectory::open(&root.path).map_err(|_| Error::AuthorityChanged)?
    } else {
        // A collision is not an interrupted object: it has no retained identity.
        let child = private_parent
            .create_child(Path::new(&intent.id))
            .map_err(|_| Error::AuthorityChanged)?;
        let root = SourceRoot::capture(parent.path.join(&intent.id))?;
        let identity = serde_json::to_string(&root.identity)?;
        store.with_library_connection(|db| -> Result<()> {
            let tx = db.transaction()?;
            super::windows_body::fence(&tx, intent, bytes)?;
            if tx.execute("UPDATE device_import_intents_v1 SET root_identity=?2 WHERE id=?1 AND root_identity IS NULL AND state='reserved'", params![intent.id,identity])? != 1 {
                return Err(Error::Conflict);
            }
            child.verify().map_err(|_| Error::AuthorityChanged)?;
            parent.verify()?;
            tx.commit()?;
            Ok(())
        })?;
        intent.root_identity = Some(identity);
        child
    };
    let root = intent_root(&parent.path, intent)?;
    let mut file = private
        .projection_file(Path::new(BODY), true)
        .map_err(|_| Error::AuthorityChanged)?
        .ok_or(Error::IncompleteImport)?;
    let mut existing = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut existing)?;
    // Resume only a prefix of the canonical exact body. Never truncate or
    // overwrite changed bytes, even though this is an unpublished reservation.
    if !bytes.starts_with(&existing) {
        return Err(Error::IncompleteImport);
    }
    file.seek(SeekFrom::Start(existing.len() as u64))?;
    file.write_all(&bytes[existing.len()..])?;
    file.sync_all()?;
    verify_projection(&root, &private, &file, bytes.len())?;
    private_parent
        .verify()
        .map_err(|_| Error::AuthorityChanged)?;
    Ok((root, private, file))
}

fn verify_projection(
    root: &SourceRoot,
    private: &PrivateDirectory,
    file: &fs::File,
    size: usize,
) -> Result<()> {
    root.verify()?;
    private.verify().map_err(|_| Error::AuthorityChanged)?;
    let named = root
        .open_optional_file_relative(Path::new(BODY))?
        .ok_or(Error::IncompleteImport)?;
    if !same_open_file(file, &named).map_err(|_| Error::AuthorityChanged)?
        || file.metadata()?.len() != size as u64
    {
        return Err(Error::AuthorityChanged);
    }
    Ok(())
}

pub(super) fn publish_import(
    store: &mut ReaderStore,
    directory: &Path,
    intent: &ImportIntent,
) -> Result<DeviceDocument> {
    let bytes = super::windows_body::load(store, intent)?;
    let parent = parent(directory, intent)?;
    let mut intent = intent.clone();
    let (root, private, file) = materialize(store, &parent, &mut intent, &bytes)?;
    let retained = root.clone();
    // The writable handle denies other writers/deletion across this short TX;
    // root pins and exact body reservation remain live until actual commit.
    let (source, ()) = store.add_local_source_with(&intent.name, root, |tx, source| {
        super::windows_body::fence(tx, &intent, &bytes).map_err(|_| crate::ReaderError::Conflict)?;
        if !source.enabled { return Err(crate::ReaderError::Unavailable); }
        tx.execute("INSERT INTO device_documents_v1(id,name,source_id,revision,bytes) VALUES(?1,?2,?3,?4,?5)",params![intent.id,intent.name,source.id,intent.revision,intent.bytes])?;
        if tx.execute("UPDATE device_import_intents_v1 SET state='published' WHERE id=?1 AND state='reserved'",[&intent.id])? != 1 { return Err(crate::ReaderError::Conflict); }
        super::windows_body::published(tx, &intent, &bytes).map_err(|_| crate::ReaderError::Conflict)?;
        verify_projection(&retained, &private, &file, bytes.len()).map_err(|_| crate::ReaderError::Unavailable)?;
        parent.verify()?;
        Ok(())
    })?;
    drop(file);
    if store.index_source(&source.id).is_ok() {
        store.with_library_connection(|db| {
            db.execute(
                "UPDATE device_documents_v1 SET indexed=1 WHERE id=?1",
                [&intent.id],
            )
        })?;
    }
    document(store, &intent.id)
}

/// Release an untouched reservation only. Removing a pinned published namespace
/// needs its own consuming directory capability; never delete partially here.
pub fn discard_import(store: &mut ReaderStore, directory: &Path, id: &str) -> Result<()> {
    let intent = super::recovery::pending_imports(store)?
        .into_iter()
        .find(|intent| intent.id == id)
        .ok_or(Error::Unavailable)?;
    let parent = parent(directory, &intent)?;
    if intent.root_identity.is_some() {
        return Err(Error::IncompleteImport);
    }
    absent(&parent, id)?;
    store.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        if !super::windows_body::check(&tx)? { return Err(Error::IncompleteImport); }
        let bytes: Vec<u8> = tx.query_row("SELECT body FROM device_windows_bodies_v1 WHERE id=?1", [id], |r| r.get(0))?;
        super::windows_body::fence(&tx, &intent, &bytes)?;
        tx.execute("DELETE FROM device_windows_bodies_v1 WHERE id=?1", [id])?;
        if tx.execute("DELETE FROM device_import_intents_v1 WHERE id=?1 AND state='reserved' AND root_identity IS NULL", [id])? != 1 { return Err(Error::Conflict); }
        parent.verify()?;
        absent(&parent, id)?;
        tx.commit()?;
        Ok(())
    })
}

fn absent(parent: &SourceRoot, id: &str) -> Result<()> {
    match fs::symlink_metadata(parent.path.join(id)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::from(error)),
        Ok(_) => Err(Error::IncompleteImport),
    }
}

#[cfg(test)]
#[path = "windows_import_tests.rs"]
mod tests;
