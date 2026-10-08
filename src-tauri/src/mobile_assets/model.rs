use super::*;

#[derive(Clone)]
pub(super) struct Item {
    pub(super) id: String,
    pub(super) doc: String,
    pub(super) revision: String,
    pub(super) target: String,
    pub(super) bytes: u32,
    pub(super) received: u32,
    pub(super) state: String,
    pub(super) expected: String,
    pub(super) hash: Option<String>,
    pub(super) mime: Option<String>,
    pub(super) width: Option<u32>,
    pub(super) height: Option<u32>,
}
pub(super) fn item_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        id: row.get(0)?,
        doc: row.get(1)?,
        revision: row.get(2)?,
        target: row.get(3)?,
        bytes: row.get(4)?,
        received: row.get(5)?,
        state: row.get(6)?,
        expected: row.get(7)?,
        hash: row.get(8)?,
        mime: row.get(9)?,
        width: row.get(10)?,
        height: row.get(11)?,
    })
}
pub(super) const ITEM_COLUMNS: &str =
    "id,document_id,revision,target,bytes,received,state,expected_manifest,hash,mime,width,height";
impl Item {
    pub(super) fn receipt(&self) -> Result<AssetReceipt> {
        Ok(AssetReceipt {
            asset_id: self.id.clone(),
            document_id: self.doc.clone(),
            document_revision: self.revision.clone(),
            target: self.target.clone(),
            bytes: self.bytes,
            content_hash: self.hash.clone().ok_or_else(unavailable)?,
            mime: self.mime.clone().ok_or_else(unavailable)?,
            width: self.width.ok_or_else(unavailable)?,
            height: self.height.ok_or_else(unavailable)?,
        })
    }
    pub(super) fn filename(&self) -> PathBuf {
        PathBuf::from(format!("{}.bin", self.id))
    }
}
