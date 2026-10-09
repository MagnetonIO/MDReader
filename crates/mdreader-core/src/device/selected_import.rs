//! Idempotent admission of an exact native OS selection. The UUID is minted by
//! the private platform queue, not an inferred source or renderer file path.
use super::*;

pub fn import_selected(
    store: &mut ReaderStore,
    directory: &Path,
    selection: uuid::Uuid,
    name: String,
    content: String,
) -> Result<DeviceDocument> {
    if selection.get_version_num() != 4 {
        return Err(Error::InvalidSelection);
    }
    imports::validate_selection(&name, &content)?;
    schema(store)?;
    imports::private_directory(directory)?;
    let id = selection.to_string();
    let saved = store.with_library_connection(|db| {
        db.query_row("SELECT id,name,revision,bytes,parent_identity,root_identity,state FROM device_import_intents_v1 WHERE id=?1", [&id], |row| {
            Ok((ImportIntent { id: row.get(0)?, name: row.get(1)?, revision: row.get(2)?, bytes: row.get(3)?, parent_identity: row.get(4)?, root_identity: row.get(5)? }, row.get::<_,String>(6)?))
        }).optional()
    }).map_err(Error::from)?;
    if let Some((intent, state)) = saved {
        if intent.name != name
            || intent.bytes as usize != content.len()
            || intent.revision != blake3::hash(content.as_bytes()).to_hex().as_str()
        {
            return Err(Error::Conflict);
        }
        if state == "published" {
            recovery::restore_published_imports(store, directory)?;
            let value = read(store, &id, Some(&intent.revision))?;
            if value.markdown != content || value.document.name != name {
                return Err(Error::Conflict);
            }
            return Ok(value.document);
        }
        if state != "reserved" {
            return Err(Error::Conflict);
        }
        // Recover only the exact completed copy. An incomplete/replaced object
        // stays reserved and visible; retries never overwrite its bytes or grant.
        return publish_import(store, directory, &intent);
    }
    // A revoked/retired published document cannot be recreated by replaying the
    // same platform selection even when its import-intent metadata is absent.
    let exists = store
        .with_library_connection(|db| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM device_documents_v1 WHERE id=?1)",
                [&id],
                |row| row.get::<_, bool>(0),
            )
        })
        .map_err(Error::from)?;
    if exists {
        return Err(Error::Conflict);
    }
    let parent =
        SourceRoot::capture(directory.canonicalize().map_err(Error::from)?).map_err(Error::from)?;
    let mut intent = imports::reserve_selected_import(store, &parent, selection, name, &content)?;
    imports::write_import(store, &parent, &mut intent, &content)?;
    publish_import(store, &parent.path, &intent)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_completed_selection_recovers_only_exact_reserved_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().join("owner");
        let documents = temp.path().join("documents");
        let mut store = ReaderStore::open(&owner).unwrap();
        imports::private_directory(&documents).unwrap();
        let parent = SourceRoot::capture(documents.canonicalize().unwrap()).unwrap();
        let id = uuid::Uuid::new_v4();
        let body = "# Original\r\n";
        let mut intent =
            imports::reserve_selected_import(&mut store, &parent, id, "original.md".into(), body)
                .unwrap();
        imports::write_import(&mut store, &parent, &mut intent, body).unwrap();
        drop(store);
        let mut store = ReaderStore::open(&owner).unwrap();
        assert!(list(&mut store).unwrap().is_empty());
        assert!(matches!(
            import_selected(
                &mut store,
                &documents,
                id,
                "original.md".into(),
                "# Different".into()
            ),
            Err(Error::Conflict)
        ));
        assert!(list(&mut store).unwrap().is_empty());
        let recovered = import_selected(
            &mut store,
            &documents,
            id,
            "original.md".into(),
            body.into(),
        )
        .unwrap();
        assert_eq!(
            read(&mut store, &recovered.id, Some(&recovered.revision))
                .unwrap()
                .markdown,
            body
        );
        assert!(recovery::pending_imports(&mut store).unwrap().is_empty());
    }
    #[test]
    fn selection_replay_preserves_original_bytes_and_user_state_without_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().join("owner");
        let documents = temp.path().join("documents");
        let mut store = ReaderStore::open(&owner).unwrap();
        let id = uuid::Uuid::new_v4();
        let body = "\u{feff}# Selected\r\n\r\nExact OS bytes.\r\n";
        let original = import_selected(
            &mut store,
            &documents,
            id,
            "selected.markdown".into(),
            body.into(),
        )
        .unwrap();
        pin(&mut store, &original.id, true).unwrap();
        position(&mut store, &original.id, &original.revision, 3).unwrap();
        drop(store);
        let mut store = ReaderStore::open(&owner).unwrap();
        let replay = import_selected(
            &mut store,
            &documents,
            id,
            "selected.markdown".into(),
            body.into(),
        )
        .unwrap();
        assert_eq!(replay.id, original.id);
        assert_eq!(replay.source_id, original.source_id);
        assert!(replay.pinned);
        assert_eq!(replay.line, 3);
        assert_eq!(list(&mut store).unwrap().len(), 1);
        assert_eq!(
            read(&mut store, &original.id, Some(&original.revision))
                .unwrap()
                .markdown,
            body
        );
        assert!(matches!(
            import_selected(
                &mut store,
                &documents,
                id,
                "selected.markdown".into(),
                "# Changed".into()
            ),
            Err(Error::Conflict)
        ));
        store
            .set_source_enabled(&original.source_id, false)
            .unwrap();
        assert!(import_selected(
            &mut store,
            &documents,
            id,
            "selected.markdown".into(),
            body.into()
        )
        .is_err());
        assert!(list(&mut store).unwrap().is_empty());
    }
}
