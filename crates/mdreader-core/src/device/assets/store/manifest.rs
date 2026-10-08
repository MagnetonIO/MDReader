use super::*;

impl AssetStore {
    pub fn manifest(
        &self,
        store: &mut ReaderStore,
        id: &str,
        revision: &str,
    ) -> Result<AssetManifest> {
        self.verify_owner(store)?;
        let markdown = document(store, id, revision)?;
        let items = self.items(store, id, revision)?;
        let manifest = Self::stamp(store, id, revision, &items)?;
        let assets = items
            .iter()
            .filter(|i| i.state == "ready")
            .map(Item::receipt)
            .collect::<Result<Vec<_>>>()?;
        let pending = items
            .into_iter()
            .filter(|i| i.state != "ready")
            .map(|i| AssetPending {
                intent_id: i.id,
                target: i.target,
                bytes: i.bytes,
                received: i.received,
                state: i.state,
            })
            .collect();
        let mut targets = image_targets(&markdown);
        let targets_truncated = targets.len() > 64;
        targets.truncate(64);
        Ok(AssetManifest {
            document_id: id.into(),
            document_revision: revision.into(),
            manifest,
            assets,
            pending,
            targets,
            targets_truncated,
        })
    }
    /// Metadata only: no original document reads, inferred source grants or image decode.
    pub fn inventory(&self, store: &mut ReaderStore) -> Result<AssetInventory> {
        self.verify_owner(store)?;
        let items = store.with_library_connection(|db| {
            db.prepare(&format!("SELECT {ITEM_COLUMNS} FROM device_asset_items_v1 ORDER BY document_id,revision,target,id LIMIT 513"))?
                .query_map([], item_row)?.collect::<rusqlite::Result<Vec<_>>>()
        }).map_err(sql)?;
        if items.len() > 512 {
            return Err(Error::InventoryBounds);
        }
        let reserved_bytes: u64 = items.iter().map(|i| u64::from(i.bytes)).sum();
        if reserved_bytes > MAX_LIBRARY_BYTES {
            return Err(Error::InventoryQuota);
        }
        let reserved_bytes = u32::try_from(reserved_bytes).map_err(|_| unavailable())?;
        let items = items
            .into_iter()
            .map(|i| AssetInventoryItem {
                asset_id: i.id,
                document_id: i.doc,
                document_revision: i.revision,
                target: i.target,
                bytes: i.bytes,
                received: i.received,
                state: i.state,
            })
            .collect();
        Ok(AssetInventory {
            items,
            reserved_bytes,
            max_bytes: u32::try_from(MAX_LIBRARY_BYTES).map_err(|_| unavailable())?,
            max_items: 512,
        })
    }
}
