//! Bounded, resumable direct-folder/BFS metadata traversal. Signed cursors bind
//! the full queue to the admitted account, root, recursion and source generation.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
const MAX_FOLDERS: usize = 64;
const MAX_SCANNED: usize = 16_384;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Walk {
    root: String,
    current: String,
    token: String,
    queue: Vec<String>,
    seen: Vec<String>,
    scanned: usize,
    pages: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default)]
    files: Vec<File>,
    next_page_token: Option<String>,
    #[serde(default)]
    incomplete_search: bool,
}
impl Walk {
    fn validate(&self, root: &RecordKey) -> Result<()> {
        if self.root != root.as_str()
            || self.seen.is_empty()
            || self.seen.len() > MAX_FOLDERS
            || self.queue.len() > MAX_FOLDERS
            || self.scanned > MAX_SCANNED
            || self.pages >= 256
            || self.token.len() > 2048
            || self.token.chars().any(char::is_control)
        {
            return Err(ProviderError::Invalid);
        }
        let mut seen = BTreeSet::new();
        for value in &self.seen {
            let key = RecordKey::new(value.clone())?;
            folder::valid_id(&key)?;
            if !seen.insert(value.clone()) {
                return Err(ProviderError::Invalid);
            }
        }
        let queued: BTreeSet<_> = self.queue.iter().collect();
        if queued.len() != self.queue.len()
            || !seen.contains(&self.current)
            || !seen.contains(&self.root)
            || self.queue.contains(&self.current)
            || self.queue.iter().any(|id| !seen.contains(id))
        {
            return Err(ProviderError::Invalid);
        }
        Ok(())
    }
}
impl Client {
    pub(super) async fn list_tree(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<MetadataPage> {
        let scope = self.folder.as_ref().ok_or(ProviderError::Unsupported)?;
        let decoded: Option<Walk> = cursor
            .map(|cursor| -> Result<Walk> {
                let value = self.codec.decode_traversal(
                    ProviderKind::GoogleDrive,
                    self.descriptor.binding(),
                    CursorKind::List,
                    cursor,
                )?;
                let walk: Walk =
                    serde_json::from_str(&value).map_err(|_| ProviderError::Invalid)?;
                let root = RecordKey::new(walk.root.clone())?;
                folder::valid_id(&root)?;
                walk.validate(&root)?;
                Ok(walk)
            })
            .transpose()?;
        let mut spent = 0;
        let root = self.root_id(context, &mut spent).await?;
        let mut walk = match decoded {
            None => Walk {
                root: root.as_str().into(),
                current: root.as_str().into(),
                token: String::new(),
                queue: vec![],
                seen: vec![root.as_str().into()],
                scanned: 0,
                pages: 0,
            },
            Some(walk) => walk,
        };
        walk.validate(&root)?;
        if !scope.recursive
            && (walk.current != walk.root || !walk.queue.is_empty() || walk.seen.len() != 1)
        {
            return Err(ProviderError::Unauthorized);
        }
        let current = RecordKey::new(walk.current.clone())?;
        if current != root {
            self.require_descendant(&current, context, &mut spent)
                .await?;
        }
        let mut query = format!("trashed = false and '{}' in parents and (mimeType = 'text/markdown' or mimeType = 'text/plain' or mimeType = 'application/octet-stream'", current.as_str());
        if scope.recursive {
            query.push_str(" or mimeType = 'application/vnd.google-apps.folder'");
        }
        query.push(')');
        let mut url = url::Url::parse("https://www.googleapis.com/drive/v3/files")
            .map_err(|_| ProviderError::Invalid)?;
        url.query_pairs_mut().append_pair("q", &query).append_pair("spaces", "drive")
            .append_pair("corpora", "user").append_pair("supportsAllDrives", "true").append_pair("includeItemsFromAllDrives", "true")
            .append_pair("pageSize", &context.page_rows().to_string())
            .append_pair("fields", "files(id,name,mimeType,version,md5Checksum,size,modifiedTime,trashed,parents),nextPageToken,incompleteSearch");
        if !walk.token.is_empty() {
            url.query_pairs_mut().append_pair("pageToken", &walk.token);
        }
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
        let page: Page =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        if page.files.len() > context.page_rows() {
            return Err(ProviderError::TooLarge);
        }
        if page.incomplete_search {
            return Err(ProviderError::Unavailable);
        }
        walk.scanned = walk
            .scanned
            .checked_add(page.files.len())
            .ok_or(ProviderError::TooLarge)?;
        walk.pages = walk.pages.checked_add(1).ok_or(ProviderError::TooLarge)?;
        if walk.scanned > MAX_SCANNED {
            return Err(ProviderError::TooLarge);
        }
        let mut rows = vec![];
        let mut unique = BTreeSet::new();
        for file in page.files {
            let key = RecordKey::new(file.id.clone())?;
            folder::valid_id(&key)?;
            if !unique.insert(key.clone())
                || file.trashed
                || !membership::parents(&file.parents)?.contains(&current)
            {
                return Err(ProviderError::Protocol);
            }
            if file.mime_type == "application/vnd.google-apps.folder" {
                if !scope.recursive {
                    return Err(ProviderError::Protocol);
                }
                if !walk.seen.contains(&file.id) {
                    if walk.seen.len() == MAX_FOLDERS {
                        return Err(ProviderError::TooLarge);
                    }
                    walk.seen.push(file.id.clone());
                    walk.queue.push(file.id);
                }
                continue;
            }
            if !markdown(&file.name) {
                continue;
            }
            file.validate(&key)?;
            let version = file.version(None)?;
            let mut metadata = file.metadata(version)?;
            metadata.display_path =
                DisplayText::new(format!("/{}/{}", scope.root.as_str(), key.as_str()))?;
            rows.push(metadata);
        }
        let remaining = if let Some(token) = page.next_page_token {
            if token.is_empty()
                || token == walk.token
                || token.len() > 2048
                || token.chars().any(char::is_control)
            {
                return Err(ProviderError::Protocol);
            }
            walk.token = token;
            true
        } else if !walk.queue.is_empty() {
            walk.current = walk.queue.remove(0);
            walk.token.clear();
            true
        } else {
            false
        };
        let next = if remaining {
            if walk.scanned == MAX_SCANNED || walk.pages >= 256 {
                return Err(ProviderError::TooLarge);
            }
            Some(self.codec.encode_traversal(
                ProviderKind::GoogleDrive,
                self.descriptor.binding(),
                CursorKind::List,
                serde_json::to_string(&walk).map_err(|_| ProviderError::Invalid)?,
            )?)
        } else {
            None
        };
        let observed = now()?;
        let coverage = if next.is_some() {
            Coverage::Partial {
                reason: DisplayText::new("Selected Drive folders continue on the next page")?,
            }
        } else {
            Coverage::MetadataCompleteSelected {
                observed_at_ms: observed,
            }
        };
        MetadataPage::new(rows, next, observed, coverage)
    }
}
