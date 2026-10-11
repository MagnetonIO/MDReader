//! Recovery evidence only. These locators never grant read or write authority.
use crate::{ReaderError, Result};
use serde::{Deserialize, Serialize};

pub const MAX_DRAFT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ACTIVE_DRAFTS: u64 = 64;
pub(crate) const MAX_SLOTS: u64 = 2048;
pub(crate) const MAX_GENERATION: u64 = 9_007_199_254_740_991;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftKey {
    Local { source_id: String, path: String },
    Provider { source_id: String, path: String },
    Device { document_id: String },
}
impl DraftKey {
    pub(crate) fn validate(&self) -> Result<()> {
        let valid =
            |s: &str, max| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control);
        let ok = match self {
            Self::Local { source_id, path } | Self::Provider { source_id, path } => {
                valid(source_id, 256) && valid(path, 4096)
            }
            Self::Device { document_id } => valid(document_id, 256),
        };
        if ok {
            Ok(())
        } else {
            Err(ReaderError::Invalid("draft locator exceeds bounds".into()))
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftInput {
    pub base_revision: String,
    /// Original literal-content digest from the verified editor, if available.
    /// Its algorithm stays with that editor kind; this journal grants no trust.
    pub base_hash: Option<String>,
    pub title: String,
    pub markdown: String,
}
impl DraftInput {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.base_revision.is_empty()
            || self.base_revision.len() > 512
            || self.base_revision.chars().any(char::is_control)
            || self.base_hash.as_ref().is_some_and(|hash| {
                hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            })
            || self.title.is_empty()
            || self.title.len() > 512
            || self.title.chars().any(char::is_control)
            || self.markdown.len() > MAX_DRAFT_BYTES
        {
            return Err(ReaderError::Invalid(
                "draft checkpoint exceeds bounds".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftRequest {
    Inspect {
        key: DraftKey,
    },
    Checkpoint {
        key: DraftKey,
        expected_generation: u64,
        draft: DraftInput,
    },
    List,
    Read {
        draft_id: String,
        generation: u64,
    },
    Discard {
        draft_id: String,
        expected_generation: u64,
    },
}
impl std::fmt::Debug for DraftRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EditorDraftRequest(<private recovery evidence>)")
    }
}
#[derive(Serialize, Deserialize)]
pub struct DraftMetadata {
    pub draft_id: String,
    pub key: DraftKey,
    pub generation: u64,
    pub base_revision: String,
    pub base_hash: Option<String>,
    pub title: String,
    pub draft_hash: String,
    pub bytes: u64,
    pub modified_ms: u64,
}
#[derive(Serialize, Deserialize)]
pub struct DraftSlot {
    pub draft_id: String,
    pub generation: u64,
    pub draft: Option<DraftMetadata>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum DraftReply {
    Slot {
        slot: DraftSlot,
    },
    Listed {
        drafts: Vec<DraftMetadata>,
    },
    Read {
        draft: DraftMetadata,
        markdown: String,
    },
}
