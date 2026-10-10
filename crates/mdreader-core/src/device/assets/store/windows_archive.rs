//! Exact canonical archive reads and read-only final body comparisons.
use super::*;
impl AssetStore {
    pub(in crate::device::assets) fn archive_item_in(
        &self,
        tx: &Transaction<'_>,
        item: &Item,
    ) -> Result<Vec<u8>> {
        self.incoming_verify_tx(tx)?;
        selected(tx, item)
    }
    pub(in crate::device::assets) fn verify_archive_item_in(
        &self,
        tx: &Transaction<'_>,
        item: &Item,
        bytes: &[u8],
    ) -> Result<()> {
        self.incoming_verify_tx(tx)?;
        if !check(tx)? || bytes.len() != item.received as usize {
            return Err(unavailable());
        }
        let current: Item = tx.query_row(
            &format!("SELECT {ITEM_COLUMNS} FROM device_asset_items_v1 WHERE id=?1"),
            [&item.id],
            item_row,
        )?;
        if &current != item {
            return Err(Error::SelectionChanged);
        }
        let exact: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM device_asset_windows_bytes_v1 WHERE id=?1 AND body=?2)",
            params![item.id, bytes],
            |r| r.get(0),
        )?;
        if !exact {
            return Err(Error::SelectionChanged);
        }
        Ok(())
    }
}
