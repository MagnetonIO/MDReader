//! Closed selected receipt and readonly association dispositions. No source grant.
use super::*;
use mdreader_core::device::{catalog::Receipt, DeviceDocument};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectedDisposition {
    Absent,
    Queued,
    Applied { sequence: u64 },
    ReauthorizationRequired,
}
pub(crate) fn validate_selected(receipt: &Receipt) -> Result<()> {
    let hash = |s: &str| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if !Uuid::parse_str(&receipt.document_id)
        .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == receipt.document_id)
        || !hash(&receipt.source_id)
        || !hash(&receipt.revision)
    {
        return Err(Error::Scope);
    }
    Ok(())
}
pub(crate) fn matches_selected(receipt: &Receipt, document: &DeviceDocument) -> bool {
    receipt.document_id == document.id
        && receipt.source_id == document.source_id
        && receipt.revision == document.revision
}
