//! Public Drive folder metadata adapter, behind exact admitted native authority.
use super::*;
#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriveFolder {
    pub id: String,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub id: String,
    pub name: String,
}
pub(super) fn valid_folder(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(NativeProviderError::Invalid);
    }
    Ok(())
}
impl NativeProviders {
    pub(super) async fn browse_drive(
        &self,
        source_id: String,
        folder_id: String,
        cursor: Option<String>,
    ) -> Result<NativeProviderResult> {
        valid_folder(&folder_id)?;
        let source = valid_source(&source_id)?;
        let cursor = cursor
            .map(|c| {
                if c.len() > mdreader_provider::MAX_CURSOR_BYTES || !c.is_ascii() {
                    return Err(NativeProviderError::Invalid);
                }
                mdreader_provider::ProviderCursor::new(c.into_bytes()).map_err(Into::into)
            })
            .transpose()?;
        let page: mdreader_provider::FolderPage = decode(
            self.rpc
                .provider(ProviderRequest::BrowseFolders {
                    source_id: source.clone(),
                    folder_id: mdreader_provider::RecordKey::new(folder_id)?,
                    cursor,
                })
                .await?,
        )?;
        if page.folders.len() > 256 {
            return Err(NativeProviderError::InvalidResponse);
        }
        let sources: Vec<SourceSummary> = decode(self.rpc.provider(ProviderRequest::List).await?)?;
        let expected = sources
            .iter()
            .find(|s| s.enabled && s.source_id == source)
            .ok_or(NativeProviderError::Unauthorized)?;
        if expected.kind != ProviderKind::GoogleDrive
            || !matches!(expected.selected_scope, SelectionSummary::ScopedRoot { .. })
        {
            return Err(NativeProviderError::InvalidResponse);
        }
        let mut unique = std::collections::BTreeSet::new();
        let folders = page
            .folders
            .into_iter()
            .map(|f| {
                valid_folder(f.id.as_str())?;
                if !unique.insert(f.id.clone()) {
                    return Err(NativeProviderError::InvalidResponse);
                }
                Ok(FolderDto {
                    id: f.id.as_str().into(),
                    name: f.name.as_str().into(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = page
            .next
            .map(|c| {
                String::from_utf8(c.as_bytes().to_vec())
                    .map_err(|_| NativeProviderError::InvalidResponse)
            })
            .transpose()?;
        Ok(NativeProviderResult::BrowseFolders {
            folders,
            next_cursor,
        })
    }
}
