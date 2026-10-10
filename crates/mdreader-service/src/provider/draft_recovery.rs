//! Recover intended local save text without reopening a grant or write ticket.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredDraft {
    pub draft_id: Uuid,
    pub revision: String,
    pub title: DisplayText,
    pub source_name: DisplayText,
    pub state: journal::MutationState,
    pub byte_length: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredDraftPage {
    pub drafts: Vec<RecoveredDraft>,
    pub offset: u32,
    pub next_offset: Option<u32>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveredDraftBody {
    pub draft: RecoveredDraft,
    pub markdown: String,
}
fn library(c: &Connection) -> Result<LibraryId> {
    // Coordinator startup already initializes the owned registry. Reads below
    // never invoke its migration or editing admission paths.
    let id: String = registry::db(c.query_row(
        "SELECT library_id FROM main.mdreader_provider_registry_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    LibraryId::new(id)
}
fn summary(c: &Connection, draft: journal::DraftRecord) -> Result<RecoveredDraft> {
    let descriptor = registry::descriptor_at(c, &draft.binding, false);
    let source_name = match descriptor {
        Ok(d) => d.name().clone(),
        Err(ProviderError::Unauthorized | ProviderError::Unavailable) => {
            DisplayText::new("Previous cloud connection")?
        }
        Err(e) => return Err(e),
    };
    // A label may be absent after reconnect, forgetting or a newer observation.
    // Do not manufacture a path, reopen a source or infer a filename from keys.
    let title = match metadata_observation::metadata(c, &draft.binding, &draft.key) {
        Ok(row) => row.name,
        Err(ProviderError::Unauthorized | ProviderError::Unavailable | ProviderError::NotFound) => {
            DisplayText::new("Recovered cloud draft.md")?
        }
        Err(e) => return Err(e),
    };
    Ok(RecoveredDraft {
        draft_id: draft.operation_id,
        revision: hex(&draft.revision),
        title,
        source_name,
        state: draft.state,
        byte_length: u32::try_from(draft.byte_length).map_err(|_| ProviderError::TooLarge)?,
    })
}
pub(super) fn page(c: &Connection, offset: u32, limit: u32) -> Result<RecoveredDraftPage> {
    let page = journal::draft_page(c, &library(c)?, offset as usize, limit as usize)?;
    Ok(RecoveredDraftPage {
        drafts: page
            .drafts
            .into_iter()
            .map(|d| summary(c, d))
            .collect::<Result<_>>()?,
        offset,
        next_offset: page
            .next_offset
            .map(|v| u32::try_from(v).map_err(|_| ProviderError::TooLarge))
            .transpose()?,
    })
}
pub(super) fn read(c: &Connection, id: Uuid, revision: &str) -> Result<RecoveredDraftBody> {
    if revision.len() != 64
        || !revision
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return Err(ProviderError::Invalid);
    }
    let hash = ContentHash::from_hex(HashAlgorithm::Sha256, revision)?;
    let digest: [u8; 32] = hash
        .value()
        .try_into()
        .map_err(|_| ProviderError::Invalid)?;
    let (draft, body) = journal::read_draft(c, &library(c)?, id, digest)?;
    Ok(RecoveredDraftBody {
        draft: summary(c, draft)?,
        markdown: String::from_utf8(body.as_bytes().to_vec())
            .map_err(|_| ProviderError::InvalidUtf8)?,
    })
}

#[cfg(test)]
#[path = "draft_recovery_tests.rs"]
mod tests;
