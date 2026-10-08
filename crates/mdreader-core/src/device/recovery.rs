use super::*;

pub(super) fn pending_imports(store: &mut ReaderStore) -> Result<Vec<ImportIntent>> {
    schema(store)?;
    store.with_library_connection(|db|db.prepare("SELECT id,name,revision,bytes,parent_identity,root_identity FROM device_import_intents_v1 WHERE state='reserved' ORDER BY rowid LIMIT 1000")?
        .query_map([],|r|Ok(ImportIntent{id:r.get(0)?,name:r.get(1)?,revision:r.get(2)?,bytes:r.get(3)?,parent_identity:r.get(4)?,root_identity:r.get(5)?}))?.collect::<rusqlite::Result<Vec<_>>>()).map_err(Error::from)
}
pub fn recover_imports(store: &mut ReaderStore, directory: &Path) -> Result<DeviceLibrary> {
    restore_published_imports(store, directory)?;
    // Only exact persisted intents are considered. Never crawl an app directory
    // or infer consent from a leftover file. Failed copies stay reserved/visible.
    for intent in pending_imports(store)? {
        let _ = publish_import(store, directory, &intent);
    }
    let pending = pending_imports(store)?
        .into_iter()
        .map(|intent| PendingDeviceImport {
            id: intent.id,
            name: intent.name,
            bytes: intent.bytes,
        })
        .collect();
    Ok(DeviceLibrary {
        documents: list(store)?,
        pending,
    })
}
// iOS may move the entire data container when an app is updated. Rebind only
// persisted published imports to the current native-owned directory, retaining
// both directory object identities and the original source grant. This does
// not discover files, accept replacement objects or revive a disabled source.
pub(super) fn restore_published_imports(store: &mut ReaderStore, directory: &Path) -> Result<()> {
    schema(store)?;
    let imports = store.with_library_connection(|db| {
        db.prepare("SELECT i.id,i.name,i.revision,i.bytes,i.parent_identity,i.root_identity,d.source_id FROM device_import_intents_v1 i JOIN device_documents_v1 d ON d.id=i.id JOIN sources s ON s.id=d.source_id WHERE i.state='published' AND s.enabled=1 ORDER BY i.rowid LIMIT 1000")?
            .query_map([], |r| Ok((ImportIntent { id:r.get(0)?, name:r.get(1)?, revision:r.get(2)?, bytes:r.get(3)?, parent_identity:r.get(4)?, root_identity:r.get(5)? },r.get::<_,String>(6)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
    }).map_err(Error::from)?;
    for (intent, source_id) in imports {
        let root = intent_root(directory, &intent)?;
        if let Some(source) = store
            .find_local_source(&root.path, &root.identity)
            .map_err(Error::from)?
        {
            if source.id != source_id || !source.enabled {
                return Err(Error::AuthorityChanged);
            }
            continue;
        }
        store
            .add_local_source_with(&intent.name, root, |_, source| {
                if source.id != source_id || !source.enabled {
                    return Err(crate::ReaderError::Unavailable);
                }
                Ok(())
            })
            .map_err(Error::from)?;
    }
    Ok(())
}
pub fn discard_import(store: &mut ReaderStore, directory: &Path, id: &str) -> Result<()> {
    let intent = pending_imports(store)?
        .into_iter()
        .find(|intent| intent.id == id)
        .ok_or(Error::Unavailable)?;
    uuid::Uuid::parse_str(id).map_err(|_| Error::InvalidSelection)?;
    let parent_identity =
        serde_json::from_str(&intent.parent_identity).map_err(|_| Error::AuthorityChanged)?;
    let parent = SourceRoot::restore(
        directory.canonicalize().map_err(Error::from)?,
        parent_identity,
    )
    .map_err(Error::from)?;
    if intent.root_identity.is_some() && fs::symlink_metadata(parent.path.join(id)).is_ok() {
        let root = intent_root(directory, &intent)?;
        if root
            .open_optional_file_relative(Path::new("document.md"))
            .map_err(Error::from)?
            .is_some()
        {
            root.remove_file_relative(Path::new("document.md"))
                .map_err(Error::from)?;
        }
        root.try_clone_directory_handle()
            .map_err(Error::from)?
            .sync_all()
            .map_err(Error::from)?;
        root.verify().map_err(Error::from)?;
    }
    // No recursive deletion: unexpected files prevent removal and keep the
    // quota reservation. The user's original selection is never touched.
    match fs::remove_dir(parent.path.join(id)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::from(error)),
    }
    parent
        .try_clone_directory_handle()
        .map_err(Error::from)?
        .sync_all()
        .map_err(Error::from)?;
    parent.verify().map_err(Error::from)?;
    store
        .with_library_connection(|db| {
            db.execute(
                "DELETE FROM device_import_intents_v1 WHERE id=?1 AND state='reserved'",
                [id],
            )
        })
        .map_err(Error::from)?;
    Ok(())
}
