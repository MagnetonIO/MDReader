use super::*;
use std::sync::Arc;

/// Local to one live query connection. Never serialize this as a public cursor.
#[derive(Clone)]
pub struct Snapshot {
    pub(super) owner: Arc<()>,
    pub(super) data_version: i64,
    pub(super) changes: u64,
    pub(super) digest: [u8; 32],
}
impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
            && self.data_version == other.data_version
            && self.changes == other.changes
            && self.digest == other.digest
    }
}
impl Eq for Snapshot {}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub document_id: String,
    pub source_id: String,
    pub revision: String,
}
pub struct Page {
    pub documents: Vec<DeviceDocument>,
    pub next_after: Option<String>,
    pub snapshot: Option<Snapshot>,
}

pub fn receipt(document: &DeviceDocument) -> Receipt {
    Receipt {
        document_id: document.id.clone(),
        source_id: document.source_id.clone(),
        revision: document.revision.clone(),
    }
}
