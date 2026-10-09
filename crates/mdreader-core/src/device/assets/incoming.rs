//! Supplied-transaction seams for reserved incoming assets, never a second owner.
use super::*;
use crate::device::publication::AssetRecord;
use rusqlite::Transaction;
#[cfg(windows)]
#[path = "incoming_windows.rs"]
mod windows;
impl AssetStore {
    pub(in crate::device) fn incoming_owner(&self, store: &mut ReaderStore) -> Result<()> {
        self.verify_owner(store)
    }
    pub(in crate::device) fn incoming_root(&self) -> SourceRoot {
        self.root.clone()
    }
    pub(in crate::device) fn incoming_verify_tx(&self, tx: &Transaction<'_>) -> Result<()> {
        self.verify()?;
        let version: Option<String> = tx
            .query_row(
                "SELECT value FROM device_asset_meta_v1 WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)?;
        if version.as_deref() != Some("1") {
            return Err(Error::UnsupportedSchema);
        }
        let root: String = tx
            .query_row(
                "SELECT value FROM device_asset_meta_v1 WHERE key='root_identity'",
                [],
                |r| r.get(0),
            )
            .map_err(sql)?;
        let identity: SourceRootIdentity =
            serde_json::from_str(&root).map_err(|_| unavailable())?;
        if identity != self.root.identity {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(in crate::device) fn incoming_reserve(
        &self,
        tx: &Transaction<'_>,
        doc: &str,
        revision: &str,
        digest: &str,
        records: &[AssetRecord],
        bodies: &[&[u8]],
    ) -> Result<()> {
        self.incoming_verify_tx(tx)?;
        if records.len() != bodies.len() {
            return Err(unavailable());
        }
        #[cfg(windows)]
        super::store::windows_bytes::initialize(tx)?;
        let (count, bytes): (u64, u64) = tx
            .query_row(
                "SELECT count(*),coalesce(sum(bytes),0) FROM device_asset_items_v1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(sql)?;
        let added: u64 = records.iter().map(|a| u64::from(a.bytes)).sum();
        if records.len() > 8
            || added > MAX_DOC_BYTES
            || count + records.len() as u64 > 512
            || bytes + added > MAX_LIBRARY_BYTES
        {
            return Err(Error::Quota);
        }
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM device_asset_items_v1 WHERE document_id=?1 UNION ALL SELECT 1 FROM device_asset_generations_v1 WHERE document_id=?1)",
                [doc],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if exists {
            return Err(unavailable());
        }
        for (record, body) in records.iter().zip(bodies) {
            tx.execute("INSERT INTO device_asset_items_v1(id,document_id,revision,target,bytes,state,expected_manifest,hash,mime) VALUES(?1,?2,?3,?4,?5,'reserved',?6,?7,?8)",params![record.id,doc,revision,record.target,record.bytes,digest,record.hash,record.mime]).map_err(sql)?;
            #[cfg(windows)]
            self.incoming_bind_canonical(tx, record, body)?;
            #[cfg(not(windows))]
            let _ = body;
        }
        Ok(())
    }
    pub(in crate::device) fn incoming_publish(
        &self,
        tx: &Transaction<'_>,
        doc: &str,
        revision: &str,
        digest: &str,
        records: &[AssetRecord],
        rasters: &[IncomingRaster],
    ) -> Result<Vec<AssetReceipt>> {
        self.incoming_verify_tx(tx)?;
        if records.len() != rasters.len() {
            return Err(unavailable());
        }
        let count: usize = tx
            .query_row(
                "SELECT count(*) FROM device_asset_items_v1 WHERE document_id=?1",
                [doc],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if count != records.len() {
            return Err(unavailable());
        }
        let mut receipts = Vec::new();
        for (record, raster) in records.iter().zip(rasters) {
            if record.hash != raster.hash || record.mime != raster.mime {
                return Err(unavailable());
            }
            let received = if cfg!(windows) { record.bytes } else { 0 };
            let updated=tx.execute("UPDATE device_asset_items_v1 SET state='ready',received=bytes,width=?8,height=?9 WHERE id=?1 AND document_id=?2 AND revision=?3 AND target=?4 AND bytes=?5 AND state='reserved' AND received=?11 AND expected_manifest=?6 AND hash=?7 AND mime=?10",params![record.id,doc,revision,record.target,record.bytes,digest,raster.hash,raster.width,raster.height,raster.mime,received]).map_err(sql)?;
            if updated != 1 {
                return Err(unavailable());
            }
            receipts.push(AssetReceipt {
                asset_id: record.id.clone(),
                document_id: doc.into(),
                document_revision: revision.into(),
                target: record.target.clone(),
                content_hash: raster.hash.clone(),
                mime: raster.mime.clone(),
                bytes: record.bytes,
                width: raster.width,
                height: raster.height,
            });
        }
        if !records.is_empty() {
            Self::advance(tx, doc, revision).map_err(sql)?;
        }
        self.verify()?;
        Ok(receipts)
    }
}
impl AssetStore {
    pub(in crate::device) fn incoming_retire(
        &self,
        tx: &Transaction<'_>,
        doc: &str,
        digest: &str,
        records: &[AssetRecord],
    ) -> Result<()> {
        self.incoming_verify_tx(tx)?;
        for record in records {
            if tx.execute("UPDATE device_asset_items_v1 SET state='retired' WHERE id=?1 AND document_id=?2 AND bytes=?3 AND expected_manifest=?4 AND state IN('reserved','retired')",params![record.id,doc,record.bytes,digest]).map_err(sql)?!=1 {return Err(unavailable());}
        }
        Ok(())
    }
    pub(in crate::device) fn incoming_delete(
        &self,
        tx: &Transaction<'_>,
        doc: &str,
        records: &[AssetRecord],
    ) -> Result<()> {
        self.incoming_verify_tx(tx)?;
        for record in records {
            #[cfg(windows)]
            if tx
                .execute(
                    "DELETE FROM device_asset_windows_bytes_v1 WHERE id=?1",
                    [&record.id],
                )
                .map_err(sql)?
                != 1
            {
                return Err(unavailable());
            }
            if tx.execute("DELETE FROM device_asset_items_v1 WHERE id=?1 AND document_id=?2 AND state='retired'",params![record.id,doc]).map_err(sql)?!=1 {return Err(unavailable());}
        }
        Ok(())
    }
}
impl AssetStore {
    pub(in crate::device) fn incoming_revalidate(
        &self,
        tx: &Transaction<'_>,
        doc: &str,
        revision: &str,
        digest: &str,
        records: &[AssetRecord],
        rasters: &[IncomingRaster],
    ) -> Result<()> {
        self.incoming_verify_tx(tx)?;
        if records.len() != rasters.len() {
            return Err(unavailable());
        }
        let count: usize = tx
            .query_row(
                "SELECT count(*) FROM device_asset_items_v1 WHERE document_id=?1",
                [doc],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if count != records.len() {
            return Err(unavailable());
        }
        let generation: Option<u64> = tx.query_row("SELECT generation FROM device_asset_generations_v1 WHERE document_id=?1 AND revision=?2",params![doc,revision],|r|r.get(0)).optional().map_err(sql)?;
        if generation.unwrap_or(0) != u64::from(!records.is_empty()) {
            return Err(unavailable());
        }
        for (record, raster) in records.iter().zip(rasters) {
            let exact:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_asset_items_v1 WHERE id=?1 AND document_id=?2 AND revision=?3 AND target=?4 AND bytes=?5 AND received=bytes AND state='ready' AND expected_manifest=?6 AND hash=?7 AND mime=?8 AND width=?9 AND height=?10)",params![record.id,doc,revision,record.target,record.bytes,digest,raster.hash,raster.mime,raster.width,raster.height],|r|r.get(0)).map_err(sql)?;
            if !exact {
                return Err(unavailable());
            }
        }
        self.verify()
    }
}
