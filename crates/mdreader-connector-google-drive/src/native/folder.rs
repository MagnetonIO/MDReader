//! Fresh read-only Drive metadata traversal. No Picker fallback, body prefetch,
//! write dispatch or filename-contains query (Drive contains is prefix matching).
use super::*;
use serde::{Deserialize, Serialize};
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const MAX_SCANNED: usize = 16_384;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Traversal {
    folder: String,
    token: String,
    scanned: usize,
    pages: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "T: Deserialize<'de>"))]
struct Page<T> {
    #[serde(default)]
    files: Vec<T>,
    next_page_token: Option<String>,
    #[serde(default)]
    incomplete_search: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Folder {
    id: String,
    name: String,
    mime_type: String,
    #[serde(default)]
    trashed: bool,
    #[serde(default)]
    parents: Vec<String>,
}
pub(super) fn valid_id(key: &RecordKey) -> Result<()> {
    if key.as_str().len() > 256
        || !key
            .as_str()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(ProviderError::Invalid);
    }
    Ok(())
}
impl Client {
    pub fn new_readonly_folder(
        descriptor: SourceDescriptor,
        folder: RecordKey,
        resolver: Arc<dyn CredentialResolver>,
        authority: Arc<dyn ProviderAuthority>,
        transport: Arc<dyn Transport>,
        cursor_key: [u8; 32],
    ) -> Result<Self> {
        valid_id(&folder)?;
        let expected = SelectedScope::ScopedRoot {
            root: folder.clone(),
            recursive: folder.as_str() == "root",
        };
        let c = descriptor.capabilities();
        if descriptor.kind() != ProviderKind::GoogleDrive
            || descriptor.scope() != &expected
            || !c.read
            || !c.list
            || c.assets
            || c.guarded_write
            || c.changes
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            descriptor,
            selected: vec![],
            folder: Some(folder),
            resolver,
            authority,
            transport,
            codec: CursorCodec::new(cursor_key),
            credentials: tokio::sync::Mutex::new(None),
        })
    }
    pub(super) fn validate_folder_member(&self, file: &File) -> Result<()> {
        if let Some(folder) = &self.folder {
            if file.parents.len() > 16 {
                return Err(ProviderError::Protocol);
            }
            for p in &file.parents {
                valid_id(&RecordKey::new(p.clone())?)?;
            }
            if folder.as_str() != "root" && !file.parents.iter().any(|p| p == folder.as_str()) {
                return Err(ProviderError::Unauthorized);
            }
        }
        Ok(())
    }
    fn traversal(
        &self,
        folder: &RecordKey,
        cursor: Option<&ProviderCursor>,
        kind: CursorKind,
    ) -> Result<Traversal> {
        match cursor {
            None => Ok(Traversal {
                folder: folder.as_str().into(),
                token: String::new(),
                scanned: 0,
                pages: 0,
            }),
            Some(c) => {
                let raw = self.codec.decode(
                    ProviderKind::GoogleDrive,
                    self.descriptor.binding(),
                    kind,
                    c,
                )?;
                let t: Traversal =
                    serde_json::from_str(raw.as_str()).map_err(|_| ProviderError::Invalid)?;
                if t.folder != folder.as_str()
                    || t.scanned > MAX_SCANNED
                    || t.pages == 0
                    || t.pages >= 256
                    || t.token.is_empty()
                    || t.token.len() > 2048
                    || t.token.chars().any(char::is_control)
                {
                    return Err(ProviderError::Invalid);
                }
                Ok(t)
            }
        }
    }
    fn next(
        &self,
        mut t: Traversal,
        token: Option<String>,
        rows: usize,
        kind: CursorKind,
    ) -> Result<Option<ProviderCursor>> {
        t.pages = t.pages.checked_add(1).ok_or(ProviderError::TooLarge)?;
        t.scanned = t.scanned.checked_add(rows).ok_or(ProviderError::TooLarge)?;
        if t.scanned > MAX_SCANNED {
            return Err(ProviderError::TooLarge);
        }
        match token {
            None => Ok(None),
            Some(token) => {
                if token.is_empty()
                    || token == t.token
                    || token.len() > 2048
                    || token.chars().any(char::is_control)
                    || t.scanned == MAX_SCANNED
                    || t.pages >= 256
                {
                    return Err(ProviderError::TooLarge);
                }
                t.token = token;
                self.cursor(
                    kind,
                    &serde_json::to_string(&t).map_err(|_| ProviderError::Invalid)?,
                )
                .map(Some)
            }
        }
    }
    async fn page<T: serde::de::DeserializeOwned>(
        &self,
        t: &Traversal,
        folders: bool,
        context: &IoContext,
    ) -> Result<Page<T>> {
        let mut q = if folders {
            format!("trashed = false and mimeType = '{FOLDER_MIME}'")
        } else {
            "trashed = false and (mimeType = 'text/markdown' or mimeType = 'text/plain' or mimeType = 'application/octet-stream')".into()
        };
        // 'root' metadata source spans the accessible account. Folder browsing,
        // including root browsing, remains direct-child and never follows shortcuts.
        if folders || t.folder != "root" {
            q.push_str(&format!(" and '{}' in parents", t.folder));
        }
        let mut url = url::Url::parse("https://www.googleapis.com/drive/v3/files")
            .map_err(|_| ProviderError::Invalid)?;
        url.query_pairs_mut().append_pair("q", &q).append_pair("spaces", "drive")
            .append_pair("corpora", "user").append_pair("supportsAllDrives", "true")
            .append_pair("includeItemsFromAllDrives", "true")
            .append_pair("pageSize", &context.page_rows().to_string())
            .append_pair("fields", "files(id,name,mimeType,version,md5Checksum,size,modifiedTime,trashed,parents),nextPageToken,incompleteSearch");
        if !t.token.is_empty() {
            url.query_pairs_mut().append_pair("pageToken", &t.token);
        }
        let mut spent = 0;
        let response = self
            .send(
                Method::Get,
                url.into(),
                BTreeMap::new(),
                vec![],
                context,
                None,
                &mut spent,
            )
            .await?;
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let page: Page<T> =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        if page.files.len() > context.page_rows() {
            return Err(ProviderError::TooLarge);
        }
        if page.incomplete_search {
            return Err(ProviderError::Unavailable);
        }
        Ok(page)
    }
    pub(super) async fn list_folder(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<MetadataPage> {
        let folder = self.folder.as_ref().ok_or(ProviderError::Unsupported)?;
        let t = self.traversal(folder, cursor, CursorKind::List)?;
        let page: Page<File> = self.page(&t, false, context).await?;
        let next = self.next(t, page.next_page_token, page.files.len(), CursorKind::List)?;
        let mut rows = vec![];
        let mut unique = std::collections::BTreeSet::new();
        for file in page.files {
            let key = RecordKey::new(file.id.clone())?;
            valid_id(&key)?;
            if !unique.insert(key.clone()) || file.trashed {
                return Err(ProviderError::Protocol);
            }
            self.validate_folder_member(&file)?;
            if !markdown(&file.name) {
                continue;
            }
            file.validate(&key)?;
            let version = file.version(None)?;
            let mut metadata = file.metadata(version)?;
            metadata.display_path =
                DisplayText::new(format!("/{}/{}", folder.as_str(), key.as_str()))?;
            rows.push(metadata);
        }
        let observed = now()?;
        let coverage = if next.is_some() {
            Coverage::Partial {
                reason: DisplayText::new("Read-only Drive metadata traversal continues")?,
            }
        } else {
            Coverage::MetadataCompleteSelected {
                observed_at_ms: observed,
            }
        };
        MetadataPage::new(rows, next, observed, coverage)
    }
    pub(super) async fn browse_folders(
        &self,
        folder: &RecordKey,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<FolderPage> {
        let root = self.folder.as_ref().ok_or(ProviderError::Unsupported)?;
        valid_id(folder)?;
        if root.as_str() != "root" && root != folder {
            return Err(ProviderError::Unauthorized);
        }
        let t = self.traversal(folder, cursor, CursorKind::Folders)?;
        let page: Page<Folder> = self.page(&t, true, context).await?;
        let next = self.next(
            t,
            page.next_page_token,
            page.files.len(),
            CursorKind::Folders,
        )?;
        let mut folders = vec![];
        let mut unique = std::collections::BTreeSet::new();
        for f in page.files {
            let id = RecordKey::new(f.id)?;
            valid_id(&id)?;
            if f.trashed
                || f.mime_type != FOLDER_MIME
                || !unique.insert(id.clone())
                || f.parents.len() > 16
                || (folder.as_str() != "root" && !f.parents.iter().any(|p| p == folder.as_str()))
            {
                return Err(ProviderError::Protocol);
            }
            folders.push(ProviderFolder {
                id,
                name: DisplayText::new(f.name)?,
            });
        }
        Ok(FolderPage { folders, next })
    }
}
