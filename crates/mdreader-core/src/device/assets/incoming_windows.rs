//! Canonical incoming image bytes use the existing Windows asset namespace.
use super::*;
use rusqlite::Connection;
impl AssetStore {
    pub(in crate::device) fn incoming_bind_canonical(
        &self,
        tx: &Transaction<'_>,
        record: &AssetRecord,
        body: &[u8],
    ) -> Result<()> {
        if body.len() != record.bytes as usize {
            return Err(unavailable());
        }
        tx.execute(
            "INSERT INTO device_asset_windows_bytes_v1(id,body) VALUES(?1,?2)",
            params![record.id, body],
        )
        .map_err(sql)?;
        if tx.execute("UPDATE device_asset_items_v1 SET received=bytes WHERE id=?1 AND state='reserved' AND received=0",[&record.id]).map_err(sql)?!=1 { return Err(unavailable()); }
        super::super::store::windows_bytes::check(tx)?;
        self.incoming_exact(tx, record, body)
    }
    pub(in crate::device) fn incoming_canonical(
        &self,
        db: &Connection,
        record: &AssetRecord,
    ) -> Result<Vec<u8>> {
        super::super::store::windows_bytes::check(db)?;
        let bytes:Vec<u8>=db.query_row("SELECT body FROM device_asset_windows_bytes_v1 WHERE id=?1 AND length(body)<=4194304",[&record.id],|r|r.get(0)).map_err(sql)?;
        self.incoming_exact(db, record, &bytes)?;
        Ok(bytes)
    }
    pub(in crate::device) fn incoming_exact(
        &self,
        db: &Connection,
        record: &AssetRecord,
        body: &[u8],
    ) -> Result<()> {
        super::super::store::windows_bytes::check(db)?;
        let exact:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM device_asset_items_v1 i JOIN device_asset_windows_bytes_v1 b ON b.id=i.id WHERE i.id=?1 AND i.target=?2 AND i.bytes=?3 AND i.received=i.bytes AND i.hash=?4 AND i.mime=?5 AND b.body=?6)",params![record.id,record.target,record.bytes,record.hash,record.mime,body],|r|r.get(0)).map_err(sql)?;
        if !exact {
            return Err(unavailable());
        }
        self.verify()
    }
}
