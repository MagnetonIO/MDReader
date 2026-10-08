//! Output-only editor evidence. No body is interpreted as a file/write grant.
use super::super::{PersistenceError, RendererIdentity, bridge::FlushRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(super) const MAX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EditorSubmission {
    pub request: FlushRequest,
    pub generation: String,
    pub entries: Vec<EditorEntry>,
    pub provider_edits_json: String,
}
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EditorEntry {
    pub entry_id: String,
    pub tab_id: String,
    pub path: String,
    pub mode: EditorMode,
    pub buffer: String,
    pub saved_content: String,
    pub base_hash: String,
    pub dirty: bool,
    pub conflict: Option<EditorConflict>,
}
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EditorMode {
    Read,
    Source,
    Split,
}
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EditorConflict {
    pub current_hash: String,
    pub source: ConflictSource,
    pub phase: ConflictPhase,
}
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConflictSource {
    External,
    Save,
}
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConflictPhase {
    Choose,
    Confirm,
}
#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EditorReceipt {
    pub request_id: String,
    pub identity: RendererIdentity,
    pub generation: String,
    pub sha256: String,
    pub entry_count: u32,
}
impl EditorSubmission {
    pub(super) fn archive(&self) -> Result<Vec<u8>, PersistenceError> {
        let generation = self
            .generation
            .parse::<u64>()
            .map_err(|_| PersistenceError::Invalid)?;
        if generation > 9_007_199_254_740_991
            || generation.to_string() != self.generation
            || self.entries.len() > 64
            || self.provider_edits_json.len() > MAX_BYTES
        {
            return Err(PersistenceError::TooLarge);
        }
        let mut size = self.provider_edits_json.len();
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            if entry.entry_id.len() > 8192
                || entry.tab_id.is_empty()
                || entry.tab_id.len() > 256
                || entry.path.is_empty()
                || entry.path.len() > 8192
                || !ids.insert(&entry.entry_id)
                || entry.entry_id != format!("{} {}", entry.tab_id, entry.path)
                || entry.base_hash.len() > 256
                || entry.dirty != (entry.buffer != entry.saved_content)
                || entry
                    .conflict
                    .as_ref()
                    .is_some_and(|value| value.current_hash.len() > 256)
            {
                return Err(PersistenceError::Invalid);
            }
            size = size
                .checked_add(entry.buffer.len())
                .and_then(|v| v.checked_add(entry.saved_content.len()))
                .ok_or(PersistenceError::TooLarge)?;
            if size > MAX_BYTES {
                return Err(PersistenceError::TooLarge);
            }
        }
        let providers: serde_json::Value = serde_json::from_str(&self.provider_edits_json)
            .map_err(|_| PersistenceError::Invalid)?;
        if providers.as_array().is_none_or(|values| values.len() > 32) {
            return Err(PersistenceError::Invalid);
        }
        // Kept as an opaque archival string; it never becomes a save ticket or
        // credential. Parsing here only bounds structural admission.
        let bytes = serde_json::to_vec(self).map_err(|_| PersistenceError::Invalid)?;
        if bytes.len() > MAX_BYTES {
            return Err(PersistenceError::TooLarge);
        }
        Ok(bytes)
    }
}
