//! Fresh folder ancestry checks for explicitly admitted Drive selections. IDs,
//! not renderer names or cached paths, establish membership; shortcuts are never
//! followed. The original account-wide read-only contract stays separate.
use super::*;
use std::collections::{BTreeSet, VecDeque};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FolderMetadata {
    id: String,
    mime_type: String,
    #[serde(default)]
    trashed: bool,
    #[serde(default)]
    parents: Vec<String>,
}
pub(super) fn parents(values: &[String]) -> Result<Vec<RecordKey>> {
    if values.len() > 16 {
        return Err(ProviderError::TooLarge);
    }
    let mut result = Vec::new();
    for value in values {
        let key = RecordKey::new(value.clone())?;
        folder::valid_id(&key)?;
        if result.contains(&key) {
            return Err(ProviderError::Protocol);
        }
        result.push(key);
    }
    Ok(result)
}
impl Client {
    #[allow(clippy::too_many_arguments)]
    pub fn new_folder(
        descriptor: SourceDescriptor,
        root: RecordKey,
        recursive: bool,
        resolver: Arc<dyn CredentialResolver>,
        authority: Arc<dyn ProviderAuthority>,
        transport: Arc<dyn Transport>,
        cursor_key: [u8; 32],
    ) -> Result<Self> {
        folder::valid_id(&root)?;
        let scope = SelectedScope::DriveFolder {
            root: root.clone(),
            recursive,
        };
        let c = descriptor.capabilities();
        if descriptor.kind() != ProviderKind::GoogleDrive
            || descriptor.scope() != &scope
            || !c.read
            || !c.list
            || c.assets
            || c.changes
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            descriptor,
            selected: vec![],
            folder: Some(folder::FolderScope {
                root,
                recursive,
                account_wide: false,
            }),
            resolver,
            authority,
            transport,
            codec: CursorCodec::new(cursor_key),
            credentials: tokio::sync::Mutex::new(None),
        })
    }
    async fn folder_metadata(
        &self,
        key: &RecordKey,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<FolderMetadata> {
        folder::valid_id(key)?;
        let response = self.send(Method::Get,
            format!("https://www.googleapis.com/drive/v3/files/{}?supportsAllDrives=true&fields=id,mimeType,trashed,parents", key.as_str()),
            BTreeMap::new(), vec![], context, None, spent).await?;
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let value: FolderMetadata =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        let id = RecordKey::new(value.id.clone())?;
        folder::valid_id(&id)?;
        if key.as_str() != "root" && key != &id {
            return Err(ProviderError::IdentityMismatch);
        }
        if value.trashed || value.mime_type != "application/vnd.google-apps.folder" {
            return Err(ProviderError::Unauthorized);
        }
        parents(&value.parents)?;
        Ok(value)
    }
    pub(super) async fn root_id(
        &self,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<RecordKey> {
        let selected = self.folder.as_ref().ok_or(ProviderError::Unsupported)?;
        RecordKey::new(
            self.folder_metadata(&selected.root, context, spent)
                .await?
                .id,
        )
    }
    pub(super) async fn ancestry(
        &self,
        initial: Vec<RecordKey>,
        root: &RecordKey,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<()> {
        let mut queue: VecDeque<_> = initial.into();
        let mut visited = BTreeSet::new();
        while let Some(key) = queue.pop_front() {
            if &key == root {
                return Ok(());
            }
            if !visited.insert(key.clone()) {
                continue;
            }
            if visited.len() > 64 || queue.len() > 64 {
                return Err(ProviderError::TooLarge);
            }
            let folder = self.folder_metadata(&key, context, spent).await?;
            queue.extend(parents(&folder.parents)?);
        }
        Err(ProviderError::Unauthorized)
    }
    pub(super) async fn require_descendant(
        &self,
        key: &RecordKey,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<()> {
        let root = self.root_id(context, spent).await?;
        self.ancestry(vec![key.clone()], &root, context, spent)
            .await
    }
    pub(super) async fn validate_member(
        &self,
        file: &File,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<()> {
        let Some(scope) = &self.folder else {
            return Ok(());
        };
        if matches!(self.descriptor.scope(), SelectedScope::ScopedRoot { .. }) {
            return self.validate_folder_member(file);
        }
        let parents = parents(&file.parents)?;
        let root = self.root_id(context, spent).await?;
        if parents.contains(&root) {
            return Ok(());
        }
        if !scope.recursive {
            return Err(ProviderError::Unauthorized);
        }
        self.ancestry(parents, &root, context, spent).await
    }
}
