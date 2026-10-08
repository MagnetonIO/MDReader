use super::*;

mod cleanup;
mod manifest;
mod read;
mod schema;
mod upload;

/// Private asset authority bound to one live reader owner and retained directory.
#[derive(Clone)]
pub struct AssetStore {
    pub(super) root: SourceRoot,
    pub(super) owner: Arc<()>,
}
impl AssetStore {
    /// Bound to one live reader owner. Reopen with a fresh store/runtime.
    pub(super) fn verify_owner(&self, store: &mut ReaderStore) -> Result<()> {
        if !Arc::ptr_eq(&self.owner, &store.device_owner_session()) {
            return Err(unavailable());
        }
        self.verify()?;
        let (version, root): (Option<String>, Option<String>) = store
            .with_library_connection(|db| {
                Ok::<_, rusqlite::Error>((
                    db.query_row(
                        "SELECT value FROM device_asset_meta_v1 WHERE key='schema_version'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?,
                    db.query_row(
                        "SELECT value FROM device_asset_meta_v1 WHERE key='root_identity'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?,
                ))
            })
            .map_err(sql)?;
        if version.as_deref() != Some("1") {
            return Err(Error::UnsupportedSchema);
        }
        let identity: SourceRootIdentity =
            serde_json::from_str(root.as_deref().ok_or_else(unavailable)?)
                .map_err(|_| unavailable())?;
        if identity != self.root.identity {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(super) fn same_owner_root(&self, owner: &Arc<()>, root: &SourceRoot) -> Result<()> {
        if !Arc::ptr_eq(&self.owner, owner)
            || self.root.path != root.path
            || self.root.identity != root.identity
        {
            return Err(unavailable());
        }
        root.verify().map_err(|_| unavailable())
    }
    /// Revalidate authority during a read without reparsing Markdown's image AST
    /// for every 64 KiB chunk. The original revision is still checked exactly.
    pub(super) fn check_selection(
        &self,
        store: &mut ReaderStore,
        receipt: &AssetReceipt,
        expected: &str,
    ) -> Result<()> {
        self.verify_owner(store)?;
        document(store, &receipt.document_id, &receipt.document_revision)?;
        let items = self.items(store, &receipt.document_id, &receipt.document_revision)?;
        if Self::stamp(
            store,
            &receipt.document_id,
            &receipt.document_revision,
            &items,
        )? != expected
            || !items
                .iter()
                .any(|item| item.id == receipt.asset_id && item.state == "ready")
        {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.root.verify().map_err(|_| unavailable())
    }
    pub(super) fn item(&self, store: &mut ReaderStore, id: &str) -> Result<Item> {
        valid_id(id)?;
        self.verify_owner(store)?;
        store
            .with_library_connection(|db| {
                db.query_row(
                    &format!("SELECT {ITEM_COLUMNS} FROM device_asset_items_v1 WHERE id=?1"),
                    [id],
                    item_row,
                )
                .optional()
            })
            .map_err(sql)?
            .ok_or_else(unavailable)
    }
    pub(super) fn items(
        &self,
        store: &mut ReaderStore,
        id: &str,
        revision: &str,
    ) -> Result<Vec<Item>> {
        self.verify_owner(store)?;
        store.with_library_connection(|db|db.prepare(&format!("SELECT {ITEM_COLUMNS} FROM device_asset_items_v1 WHERE document_id=?1 AND revision=?2 ORDER BY target,id LIMIT 513"))?.query_map(params![id,revision],item_row)?.collect::<rusqlite::Result<Vec<_>>>()).map_err(sql)
    }
    pub(super) fn stamp(
        store: &mut ReaderStore,
        id: &str,
        revision: &str,
        items: &[Item],
    ) -> Result<String> {
        let generation: u64 = store.with_library_connection(|db| {
            db.query_row("SELECT generation FROM device_asset_generations_v1 WHERE document_id=?1 AND revision=?2", params![id, revision], |r| r.get(0)).optional().map(|g| g.unwrap_or(0))
        }).map_err(sql)?;
        let mut hash = Sha256::new();
        hash.update(b"mdreader-device-assets-manifest-v1\0");
        for field in [id, revision] {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        hash.update(generation.to_le_bytes());
        for item in items.iter().filter(|item| item.state == "ready") {
            for field in [&item.id, &item.target, item.hash.as_deref().unwrap_or("")] {
                hash.update((field.len() as u64).to_le_bytes());
                hash.update(field.as_bytes());
            }
        }
        Ok(format!("{:x}", hash.finalize()))
    }
    pub(super) fn advance(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        revision: &str,
    ) -> rusqlite::Result<()> {
        if tx.execute("INSERT INTO device_asset_generations_v1(document_id,revision,generation) VALUES(?1,?2,1) ON CONFLICT(document_id,revision) DO UPDATE SET generation=generation+1 WHERE generation<9223372036854775807", params![id, revision])? != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }
    pub(super) fn sync(&self) -> Result<()> {
        self.root
            .try_clone_directory_handle()
            .map_err(|_| unavailable())?
            .sync_all()
            .map_err(|_| unavailable())?;
        self.verify()
    }
}
