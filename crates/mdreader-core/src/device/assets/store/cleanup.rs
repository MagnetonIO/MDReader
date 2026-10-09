use super::*;

impl AssetStore {
    /// Explicitly forget one tracked private copy, including revoked/unreadable documents.
    /// Retire its authority transactionally before deleting only its generated filename.
    pub fn forget(&self, store: &mut ReaderStore, id: &str) -> Result<()> {
        let item = self.item(store, id)?;
        if item.state == "ready" {
            store.with_library_connection(|db| {
                let tx = db.transaction()?;
                if tx.execute("UPDATE device_asset_items_v1 SET state='retired' WHERE id=?1 AND state='ready'", [id])? != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Self::advance(&tx, &item.doc, &item.revision)?;
                tx.commit()
            }).map_err(sql)?;
        }
        self.discard(store, id)
    }
    /// Removing authority is durable before physical deletion. Failed deletion
    /// remains a visible retired item and continues consuming reserved quota.
    pub fn remove(
        &self,
        store: &mut ReaderStore,
        doc: &str,
        revision: &str,
        id: &str,
        expected: &str,
    ) -> Result<()> {
        let current = self.manifest(store, doc, revision)?;
        let item = self.item(store, id)?;
        if current.manifest != expected
            || item.doc != doc
            || item.revision != revision
            || item.state != "ready"
        {
            return Err(unavailable());
        }
        store
            .with_library_connection(|db| {
                let tx = db.transaction()?;
                if tx.execute(
                    "UPDATE device_asset_items_v1 SET state='retired' WHERE id=?1 AND state='ready'",
                    [id],
                )? != 1 { return Err(rusqlite::Error::QueryReturnedNoRows); }
                Self::advance(&tx, doc, revision)?;
                tx.commit()
            })
            .map_err(sql)?;
        self.discard(store, id)
    }
    pub fn discard(&self, store: &mut ReaderStore, id: &str) -> Result<()> {
        let item = self.item(store, id)?;
        if item.state == "ready" {
            return Err(Error::PublishedAsset);
        }
        #[cfg(windows)]
        return self.discard_windows(store, &item);
        #[cfg(not(windows))]
        {
            if self
                .root
                .open_optional_file_relative(&item.filename())
                .map_err(|_| unavailable())?
                .is_some()
            {
                self.root
                    .remove_file_relative(&item.filename())
                    .map_err(|_| unavailable())?;
            }
            self.sync()?;
            store
                .with_library_connection(|db| {
                    db.execute(
                        "DELETE FROM device_asset_items_v1 WHERE id=?1 AND state!='ready'",
                        [id],
                    )
                })
                .map_err(sql)?;
            Ok(())
        }
    }
}
