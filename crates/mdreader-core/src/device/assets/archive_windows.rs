//! Owner-transaction snapshots of canonical Windows bytes, including partial
//! and retired uploads. A rebuildable .bin projection grants no authority here.
use super::*;

impl AssetStore {
    pub(in crate::device) fn archived_bytes_in(
        &self,
        tx: &Transaction<'_>,
        expected: &CurrentAssets,
    ) -> Result<Vec<Vec<u8>>> {
        super::super::store::windows_bytes::check(tx)?;
        self.verify_current_assets_in(tx, expected)?;
        let bytes = expected
            .items
            .iter()
            .map(|item| self.archive_item_in(tx, item))
            .collect::<Result<Vec<_>>>()?;
        self.verify_current_assets_in(tx, expected)?;
        self.verify()?;
        Ok(bytes)
    }
    pub(in crate::device) fn verify_archived_bytes_in(
        &self,
        tx: &Transaction<'_>,
        expected: &CurrentAssets,
        bytes: &[Option<&[u8]>],
    ) -> Result<()> {
        super::super::store::windows_bytes::check(tx)?;
        self.verify_current_assets_in(tx, expected)?;
        if bytes.len() != expected.items.len() {
            return Err(unavailable());
        }
        for (item, body) in expected.items.iter().zip(bytes) {
            self.verify_archive_item_in(tx, item, body.ok_or_else(unavailable)?)?;
        }
        self.verify()
    }
    pub(in crate::device) fn verify_retired_archived_bytes_in(
        &self,
        tx: &Transaction<'_>,
        expected: &CurrentAssets,
        bytes: &[Option<&[u8]>],
    ) -> Result<()> {
        super::super::store::windows_bytes::check(tx)?;
        self.verify_retired_assets_in(tx, expected)?;
        if bytes.len() != expected.items.len() {
            return Err(unavailable());
        }
        for (item, body) in expected.items.iter().zip(bytes) {
            let mut retired = item.clone();
            if retired.state == "ready" || retired.state == "reserved" {
                retired.state = "retired".into();
            }
            self.verify_archive_item_in(tx, &retired, body.ok_or_else(unavailable)?)?;
        }
        self.verify()
    }
}
