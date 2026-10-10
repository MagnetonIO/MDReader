//! Renderer receives readonly intended text, never an edit/dispatch capability.
use super::*;
use mdreader_service::provider::{RecoveredDraft, RecoveredDraftBody, RecoveredDraftPage};

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecoveredDraftDto {
    pub draft_id: String,
    pub revision: String,
    pub title: String,
    pub source_name: String,
    pub state: MutationStateDto,
    pub byte_length: u32,
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn summary(value: RecoveredDraft) -> Result<RecoveredDraftDto> {
    valid_uuid(&value.draft_id.to_string())?;
    if !digest(&value.revision) || value.byte_length as usize > MAX_MARKDOWN {
        return Err(NativeProviderError::InvalidResponse);
    }
    Ok(RecoveredDraftDto {
        draft_id: value.draft_id.to_string(),
        revision: value.revision,
        title: value.title.as_str().into(),
        source_name: value.source_name.as_str().into(),
        state: mutation_state(value.state),
        byte_length: value.byte_length,
    })
}
impl NativeProviders {
    pub(super) async fn recovered_drafts(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<NativeProviderResult> {
        if offset > 1000 || !(1..=32).contains(&limit) {
            return Err(NativeProviderError::Invalid);
        }
        let value: RecoveredDraftPage = decode(
            self.rpc
                .provider(ProviderRequest::RecoveredDrafts { offset, limit })
                .await?,
        )?;
        if value.offset != offset
            || value.drafts.len() > limit as usize
            || value
                .next_offset
                .is_some_and(|next| next != offset + value.drafts.len() as u32 || next > 1000)
        {
            return Err(NativeProviderError::InvalidResponse);
        }
        let mut ids = std::collections::BTreeSet::new();
        let drafts = value
            .drafts
            .into_iter()
            .map(|draft| {
                if !ids.insert(draft.draft_id) {
                    return Err(NativeProviderError::InvalidResponse);
                }
                summary(draft)
            })
            .collect::<Result<_>>()?;
        Ok(NativeProviderResult::RecoveredDrafts {
            drafts,
            offset: value.offset,
            next_offset: value.next_offset,
        })
    }
    pub(super) async fn read_recovered_draft(
        &self,
        draft_id: String,
        revision: String,
    ) -> Result<NativeProviderResult> {
        let id = valid_uuid(&draft_id)?;
        if !digest(&revision) {
            return Err(NativeProviderError::Invalid);
        }
        let value: RecoveredDraftBody = decode(
            self.rpc
                .provider(ProviderRequest::ReadRecoveredDraft {
                    draft_id: id,
                    revision: revision.clone(),
                })
                .await?,
        )?;
        checked_body(value, &draft_id, &revision)
    }
}

fn checked_body(
    value: RecoveredDraftBody,
    draft_id: &str,
    revision: &str,
) -> Result<NativeProviderResult> {
    let draft = summary(value.draft)?;
    let hash: String = Sha256::digest(value.markdown.as_bytes())
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    if draft.draft_id != draft_id
        || draft.revision != revision
        || value.markdown.len() > MAX_MARKDOWN
        || value.markdown.len() != draft.byte_length as usize
        || hash != revision
    {
        return Err(NativeProviderError::InvalidResponse);
    }
    Ok(NativeProviderResult::ReadRecoveredDraft {
        draft,
        markdown: value.markdown,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readonly_draft_projection_requires_exact_id_digest_and_literal_byte_length() {
        let id = Uuid::new_v4();
        let markdown = "# intended\r\n📝";
        let revision: String = Sha256::digest(markdown.as_bytes())
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        let make = || RecoveredDraftBody {
            draft: RecoveredDraft {
                draft_id: id,
                revision: revision.clone(),
                title: DisplayText::new("Public draft.md").unwrap(),
                source_name: DisplayText::new("Previous cloud connection").unwrap(),
                state: mdreader_provider::journal::MutationState::Unknown,
                byte_length: markdown.len() as u32,
            },
            markdown: markdown.into(),
        };
        assert!(matches!(
            checked_body(make(), &id.to_string(), &revision),
            Ok(NativeProviderResult::ReadRecoveredDraft { .. })
        ));
        let mut wrong = make();
        wrong.markdown.push('x');
        assert!(matches!(
            checked_body(wrong, &id.to_string(), &revision),
            Err(NativeProviderError::InvalidResponse)
        ));
        assert!(matches!(
            checked_body(make(), &Uuid::new_v4().to_string(), &revision),
            Err(NativeProviderError::InvalidResponse)
        ));
        let mut wrong = make();
        wrong.draft.byte_length += 1;
        assert!(matches!(
            checked_body(wrong, &id.to_string(), &revision),
            Err(NativeProviderError::InvalidResponse)
        ));
    }
}
