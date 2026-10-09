use super::*;

pub(super) fn private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(Error::AuthorityChanged),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            builder.create(path).map_err(Error::from)?;
        }
        Err(error) => return Err(Error::from(error)),
    }
    Ok(())
}
// A reservation is durable before any file write and consumes the same quota as
// a published document. An interrupted copy never creates a source grant.
pub(super) fn reserve_import(
    store: &mut ReaderStore,
    parent: &SourceRoot,
    name: String,
    content: &str,
) -> Result<ImportIntent> {
    schema(store)?;
    let intent = ImportIntent {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        revision: blake3::hash(content.as_bytes()).to_hex().to_string(),
        bytes: content.len() as u32,
        parent_identity: serde_json::to_string(&parent.identity).map_err(Error::from)?,
        root_identity: None,
    };
    store.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        let (count, bytes): (u64,u64) = tx.query_row(
            "SELECT count(*),coalesce(sum(bytes),0) FROM (SELECT bytes FROM device_documents_v1 UNION ALL SELECT bytes FROM device_import_intents_v1 WHERE state='reserved')",
            [], |r| Ok((r.get(0)?,r.get(1)?)))?;
        let (incoming_count, incoming_bytes) = publication::reserved_usage(&tx)?;
        if count + incoming_count >= MAX_DOCUMENTS || bytes + incoming_bytes + u64::from(intent.bytes) > MAX_LIBRARY_BYTES {
            return Err(Error::Quota);
        }
        tx.execute("INSERT INTO device_import_intents_v1(id,name,revision,bytes,parent_identity,state) VALUES(?1,?2,?3,?4,?5,'reserved')",
            params![intent.id,intent.name,intent.revision,intent.bytes,intent.parent_identity])?;
        tx.commit()?;
        Ok(())
    })?;
    Ok(intent)
}
pub(super) fn write_import(
    store: &mut ReaderStore,
    parent: &SourceRoot,
    intent: &mut ImportIntent,
    content: &str,
) -> Result<()> {
    let root = parent
        .create_or_capture_directory_relative(Path::new(&intent.id))
        .map_err(Error::from)?;
    parent
        .try_clone_directory_handle()
        .map_err(Error::from)?
        .sync_all()
        .map_err(Error::from)?;
    let identity = serde_json::to_string(&root.identity).map_err(Error::from)?;
    store.with_library_connection(|db|db.execute("UPDATE device_import_intents_v1 SET root_identity=?2 WHERE id=?1 AND state='reserved'",params![intent.id,identity])).map_err(Error::from)?;
    intent.root_identity = Some(identity);
    let mut file = root
        .create_new_relative(Path::new("document.md"))
        .map_err(Error::from)?;
    file.write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(Error::from)?;
    root.try_clone_directory_handle()
        .map_err(Error::from)?
        .sync_all()
        .map_err(Error::from)?;
    root.verify().map_err(Error::from)
}
pub(super) fn intent_root(directory: &Path, intent: &ImportIntent) -> Result<SourceRoot> {
    uuid::Uuid::parse_str(&intent.id).map_err(|_| Error::InvalidSelection)?;
    let parent_identity: SourceRootIdentity =
        serde_json::from_str(&intent.parent_identity).map_err(|_| Error::AuthorityChanged)?;
    let parent = SourceRoot::restore(
        directory.canonicalize().map_err(Error::from)?,
        parent_identity,
    )
    .map_err(Error::from)?;
    let identity: SourceRootIdentity = serde_json::from_str(
        intent
            .root_identity
            .as_deref()
            .ok_or(Error::IncompleteImport)?,
    )
    .map_err(|_| Error::AuthorityChanged)?;
    parent.verify().map_err(Error::from)?;
    SourceRoot::restore(parent.path.join(&intent.id), identity).map_err(Error::from)
}
pub(super) fn publish_import(
    store: &mut ReaderStore,
    directory: &Path,
    intent: &ImportIntent,
) -> Result<DeviceDocument> {
    let root = intent_root(directory, intent)?;
    let mut bytes = Vec::new();
    let mut file = root
        .open_optional_file_relative(Path::new("document.md"))
        .map_err(Error::from)?
        .ok_or(Error::IncompleteImport)?;
    Read::by_ref(&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(Error::from)?;
    if bytes.len() != intent.bytes as usize
        || blake3::hash(&bytes).to_hex().as_str() != intent.revision
        || std::str::from_utf8(&bytes).is_err()
    {
        return Err(Error::IncompleteImport);
    }
    // Sync recovery bytes too; source grant, metadata and completion share one
    // SQLite transaction and final retained-root verification.
    file.sync_all().map_err(Error::from)?;
    root.try_clone_directory_handle()
        .map_err(Error::from)?
        .sync_all()
        .map_err(Error::from)?;
    let (source,()) = store.add_local_source_with(&intent.name,root,|tx,source| {
        if !source.enabled { return Err(crate::ReaderError::Unavailable); }
        tx.execute("INSERT INTO device_documents_v1(id,name,source_id,revision,bytes) VALUES(?1,?2,?3,?4,?5)",params![intent.id,intent.name,source.id,intent.revision,intent.bytes])?;
        if tx.execute("UPDATE device_import_intents_v1 SET state='published' WHERE id=?1 AND state='reserved'",[&intent.id])? != 1 {
            return Err(crate::ReaderError::Conflict);
        }
        Ok(())
    }).map_err(Error::from)?;
    let indexed = store.index_source(&source.id).is_ok();
    if indexed {
        store
            .with_library_connection(|db| {
                db.execute(
                    "UPDATE device_documents_v1 SET indexed=1 WHERE id=?1",
                    [&intent.id],
                )
            })
            .map_err(Error::from)?;
    }
    document(store, &intent.id)
}
pub fn import(
    store: &mut ReaderStore,
    directory: &Path,
    name: String,
    content: String,
) -> Result<DeviceDocument> {
    if name.is_empty()
        || name.len() > 240
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        || !name.to_ascii_lowercase().ends_with(".md")
        || content.len() > MAX_BYTES
    {
        return Err(Error::InvalidSelection);
    }
    schema(store)?;
    private_directory(directory)?;
    let parent =
        SourceRoot::capture(directory.canonicalize().map_err(Error::from)?).map_err(Error::from)?;
    let mut intent = reserve_import(store, &parent, name, &content)?;
    write_import(store, &parent, &mut intent, &content)?;
    publish_import(store, &parent.path, &intent)
}
