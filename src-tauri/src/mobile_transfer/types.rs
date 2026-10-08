use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reference {
    pub document_id: String,
    pub source_id: String,
    pub path: String,
    pub revision: String,
}
impl Reference {
    pub(super) fn receipt(&self) -> Result<mdreader_core::device::catalog::Receipt, String> {
        if uuid::Uuid::parse_str(&self.document_id).is_err()
            || self.path != "document.md"
            || self.source_id.len() > 128
            || self.revision.len() != 64
            || !self
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("stale".into());
        }
        Ok(mdreader_core::device::catalog::Receipt {
            document_id: self.document_id.clone(),
            source_id: self.source_id.clone(),
            revision: self.revision.clone(),
        })
    }
}
#[derive(Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Export,
    Share,
}
#[derive(Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Exported,
    Presented,
    Cancelled,
}
#[derive(Serialize, Type)]
pub struct Reply {
    pub state: TransferState,
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub export: bool,
    pub share: bool,
    pub reading_links: bool,
}
#[derive(Serialize, Type)]
pub struct PendingLinks {
    pub links: Vec<String>,
    pub truncated: bool,
}
