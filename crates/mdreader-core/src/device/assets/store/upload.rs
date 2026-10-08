use super::*;

impl AssetStore {
    pub fn begin(
        &self,
        store: &mut ReaderStore,
        id: &str,
        revision: &str,
        target: &str,
        bytes: u32,
        expected: &str,
    ) -> Result<String> {
        let current = self.manifest(store, id, revision)?;
        let target = canonical_target(target)?;
        if bytes == 0
            || bytes > MAX_FILE
            || current.manifest != expected
            || !current.targets.contains(&target)
        {
            return Err(Error::SelectionChanged);
        }
        let item = Item {
            id: uuid::Uuid::new_v4().to_string(),
            doc: id.into(),
            revision: revision.into(),
            target,
            bytes,
            received: 0,
            state: "reserved".into(),
            expected: expected.into(),
            hash: None,
            mime: None,
            width: None,
            height: None,
        };
        store.with_library_connection(|db| -> Result<()> {
            let tx = db.transaction()?;
            let (count, total): (u64, u64) = tx.query_row(
                "SELECT count(*), coalesce(sum(bytes),0) FROM device_asset_items_v1",
                [], |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let (doc_count, doc_bytes): (u64, u64) = tx.query_row(
                "SELECT count(*), coalesce(sum(bytes),0) FROM device_asset_items_v1 WHERE document_id=?1",
                [id], |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if count >= 512 || total + u64::from(bytes) > MAX_LIBRARY_BYTES
                || doc_count >= 8 || doc_bytes + u64::from(bytes) > MAX_DOC_BYTES {
                return Err(Error::Quota);
            }
            tx.execute(
                "INSERT INTO device_asset_items_v1(id,document_id,revision,target,bytes,state,expected_manifest) VALUES(?1,?2,?3,?4,?5,'reserved',?6)",
                params![item.id,item.doc,item.revision,item.target,item.bytes,item.expected],
            ).map_err(|error| match error {
                rusqlite::Error::SqliteFailure(code, _) if matches!(code.extended_code, rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE | rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY) => Error::Quota,
                _ => Error::Metadata,
            })?;
            tx.commit()?;
            Ok(())
        })?;
        self.root
            .create_new_relative(&item.filename())
            .map_err(|_| unavailable())?
            .sync_all()
            .map_err(|_| unavailable())?;
        self.sync()?;
        Ok(item.id)
    }
    pub fn write(
        &self,
        store: &mut ReaderStore,
        id: &str,
        offset: u32,
        encoded: &str,
    ) -> Result<u32> {
        if encoded.len() > MAX_BASE64 {
            return Err(Error::ChunkBounds);
        }
        let chunk = STANDARD.decode(encoded).map_err(|_| Error::InvalidChunk)?;
        let item = self.item(store, id)?;
        document(store, &item.doc, &item.revision)?;
        if item.state != "reserved"
            || chunk.is_empty()
            || chunk.len() > MAX_CHUNK
            || u64::from(offset) + chunk.len() as u64 > u64::from(item.bytes)
            || offset > item.received
        {
            return Err(Error::InvalidOffset);
        }
        let mut file = self
            .root
            .open_optional_file_relative(&item.filename())
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        let size = file.metadata().map_err(|_| unavailable())?.len();
        if size > u64::from(item.bytes) || size < u64::from(item.received) {
            return Err(unavailable());
        }
        file.seek(SeekFrom::Start(u64::from(offset)))
            .map_err(|_| unavailable())?;
        let existing = (size - u64::from(offset)).min(chunk.len() as u64) as usize;
        let mut prefix = vec![0; existing];
        file.read_exact(&mut prefix).map_err(|_| unavailable())?;
        if prefix != chunk[..existing]
            || (offset < item.received
                && u64::from(offset) + chunk.len() as u64 > u64::from(item.received))
        {
            return Err(Error::ReplayConflict);
        }
        if offset < item.received {
            return Ok(item.received);
        }
        if size > u64::from(offset) + chunk.len() as u64 {
            return Err(Error::RecoveryOffset);
        }
        let mut append = self
            .root
            .open_append_relative(&item.filename())
            .map_err(|_| unavailable())?;
        append
            .write_all(&chunk[existing..])
            .and_then(|_| append.sync_all())
            .map_err(|_| unavailable())?;
        self.verify()?;
        let next = offset + chunk.len() as u32;
        store.with_library_connection(|db|db.execute("UPDATE device_asset_items_v1 SET received=?2 WHERE id=?1 AND state='reserved' AND received=?3",params![id,next,item.received])).map_err(sql)?;
        Ok(next)
    }
    /// Prepare under owner access; decoding/hash work is deliberately separate.
    pub fn prepare_commit(
        &self,
        store: &mut ReaderStore,
        id: &str,
        permit: ValidationPermit,
    ) -> Result<PreparedAsset> {
        let item = self.item(store, id)?;
        document(store, &item.doc, &item.revision)?;
        if item.state != "reserved" || item.received != item.bytes {
            return Err(Error::IncompleteUpload);
        }
        Ok(PreparedAsset {
            _permit: permit,
            bytes: self.read_file(&item)?,
            item,
            root: self.root.clone(),
            owner: self.owner.clone(),
        })
    }
    pub fn publish(
        &self,
        store: &mut ReaderStore,
        validated: ValidatedAsset,
    ) -> Result<(AssetReceipt, String)> {
        self.verify_owner(store)?;
        self.same_owner_root(&validated.owner, &validated.root)?;
        let item = self.item(store, &validated.item.id)?;
        document(store, &item.doc, &item.revision)?;
        let items = self.items(store, &item.doc, &item.revision)?;
        if item.state != "reserved"
            || Self::stamp(store, &item.doc, &item.revision, &items)? != item.expected
        {
            return Err(Error::ManifestChanged);
        }
        store.with_library_connection(|db| {
            let tx = db.transaction()?;
            tx.execute(
                "UPDATE device_asset_items_v1 SET state='retired' WHERE document_id=?1 AND revision=?2 AND target=?3 AND state='ready'",
                params![item.doc,item.revision,item.target],
            )?;
            if tx.execute(
                "UPDATE device_asset_items_v1 SET state='ready',hash=?2,mime=?3,width=?4,height=?5 WHERE id=?1 AND state='reserved' AND received=bytes",
                params![item.id,validated.hash,validated.mime,validated.width,validated.height],
            )? != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            Self::advance(&tx, &item.doc, &item.revision)?;
            tx.commit()
        }).map_err(sql)?;
        let receipt = self.item(store, &item.id)?.receipt()?;
        let items = self.items(store, &item.doc, &item.revision)?;
        let stamp = Self::stamp(store, &item.doc, &item.revision, &items)?;
        Ok((receipt, stamp))
    }
}
