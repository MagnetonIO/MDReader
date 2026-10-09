//! Depth-first metadata traversal: document pages, then one child at a time.
//! Folder width never accumulates in the signed cursor.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
const MAX_DEPTH: usize = 64;
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Documents,
    Folders,
    Done,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    folder: String,
    phase: Phase,
    token: String,
}
impl Frame {
    fn documents(folder: String) -> Self {
        Self {
            folder,
            phase: Phase::Documents,
            token: String::new(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Walk {
    v: u8,
    root: String,
    stack: Vec<Frame>,
}
impl Walk {
    fn validate(&self, root: &RecordKey) -> Result<()> {
        if self.v != 2
            || self.root != root.as_str()
            || self.stack.is_empty()
            || self.stack.len() > MAX_DEPTH
            || self.stack[0].folder != self.root
        {
            return Err(ProviderError::Invalid);
        }
        let mut ancestors = BTreeSet::new();
        for frame in &self.stack {
            folder::valid_id(&RecordKey::new(frame.folder.clone())?)?;
            if !ancestors.insert(&frame.folder)
                || frame.token.len() > 2048
                || frame.token.chars().any(char::is_control)
                || frame.phase == Phase::Done && !frame.token.is_empty()
            {
                return Err(ProviderError::Invalid);
            }
        }
        if self.stack.last().is_some_and(|f| f.phase == Phase::Done) {
            return Err(ProviderError::Invalid);
        }
        Ok(())
    }
    fn finish_folder(&mut self) {
        self.stack.pop();
        while self.stack.last().is_some_and(|f| f.phase == Phase::Done) {
            self.stack.pop();
        }
    }
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
impl Client {
    pub(super) async fn list_tree(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<MetadataPage> {
        let scope = self.folder.as_ref().ok_or(ProviderError::Unsupported)?;
        let decoded = cursor
            .map(|cursor| -> Result<Walk> {
                let value = self.codec.decode_traversal(
                    ProviderKind::GoogleDrive,
                    self.descriptor.binding(),
                    CursorKind::List,
                    cursor,
                )?;
                let shape: serde_json::Value =
                    serde_json::from_str(&value).map_err(|_| ProviderError::Invalid)?;
                // Previous signed cursors describe disposable metadata staging.
                // Restart it without changing published copies or source grants.
                if shape.get("v").is_none()
                    && shape.get("queue").is_some()
                    && shape.get("seen").is_some()
                    && shape.get("current").is_some()
                {
                    return Err(ProviderError::ResetCursor);
                }
                serde_json::from_value(shape).map_err(|_| ProviderError::Invalid)
            })
            .transpose()?;
        let mut spent = 0;
        let root = self.root_id(context, &mut spent).await?;
        let mut walk = decoded.unwrap_or_else(|| Walk {
            v: 2,
            root: root.as_str().into(),
            stack: vec![Frame::documents(root.as_str().into())],
        });
        walk.validate(&root)?;
        if !scope.recursive && (walk.stack.len() != 1 || walk.stack[0].phase != Phase::Documents) {
            return Err(ProviderError::Unauthorized);
        }
        let frame = walk.stack.last().ok_or(ProviderError::Invalid)?;
        let current = RecordKey::new(frame.folder.clone())?;
        if current != root {
            self.require_descendant(&current, context, &mut spent)
                .await?;
        }
        let folders = frame.phase == Phase::Folders;
        let query = if folders {
            format!(
                "trashed = false and '{}' in parents and mimeType = '{FOLDER_MIME}'",
                current.as_str()
            )
        } else {
            format!("trashed = false and '{}' in parents and (mimeType = 'text/markdown' or mimeType = 'text/plain' or mimeType = 'application/octet-stream')", current.as_str())
        };
        let page_rows = if folders { 1 } else { context.page_rows() };
        let mut url = url::Url::parse("https://www.googleapis.com/drive/v3/files")
            .map_err(|_| ProviderError::Invalid)?;
        url.query_pairs_mut().append_pair("q", &query).append_pair("spaces", "drive")
            .append_pair("corpora", "user").append_pair("supportsAllDrives", "true")
            .append_pair("includeItemsFromAllDrives", "true")
            .append_pair("pageSize", &page_rows.to_string())
            .append_pair("fields", "files(id,name,mimeType,version,md5Checksum,size,modifiedTime,trashed,parents),nextPageToken,incompleteSearch");
        if !frame.token.is_empty() {
            url.query_pairs_mut().append_pair("pageToken", &frame.token);
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
        if page.files.len() > page_rows {
            return Err(ProviderError::TooLarge);
        }
        if page.incomplete_search {
            return Err(ProviderError::Unavailable);
        }
        if page.next_page_token.as_ref().is_some_and(|token| {
            token.is_empty()
                || token == &frame.token
                || token.len() > 2048
                || token.chars().any(char::is_control)
        }) {
            return Err(ProviderError::Protocol);
        }
        let mut rows = Vec::new();
        let mut unique = BTreeSet::new();
        let mut child = None;
        for file in page.files {
            let key = RecordKey::new(file.id.clone())?;
            folder::valid_id(&key)?;
            if !unique.insert(key.clone())
                || file.trashed
                || !membership::parents(&file.parents)?.contains(&current)
            {
                return Err(ProviderError::Protocol);
            }
            if folders {
                if file.mime_type != FOLDER_MIME || walk.stack.iter().any(|f| f.folder == file.id) {
                    return Err(ProviderError::Protocol);
                }
                child = Some(file.id);
            } else {
                if file.mime_type == FOLDER_MIME {
                    return Err(ProviderError::Protocol);
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
        }
        let frame = walk.stack.last_mut().ok_or(ProviderError::Invalid)?;
        match page.next_page_token {
            Some(token) => frame.token = token,
            None => {
                frame.token.clear();
                if !folders && scope.recursive {
                    frame.phase = Phase::Folders;
                } else {
                    frame.phase = Phase::Done;
                }
            }
        }
        if let Some(folder) = child {
            if walk.stack.len() == MAX_DEPTH {
                return Err(ProviderError::TooLarge);
            }
            walk.stack.push(Frame::documents(folder));
        } else if walk.stack.last().is_some_and(|f| f.phase == Phase::Done) {
            walk.finish_folder();
        }
        let next = if walk.stack.is_empty() {
            None
        } else {
            walk.validate(&root)?;
            Some(self.codec.encode_traversal(
                ProviderKind::GoogleDrive,
                self.descriptor.binding(),
                CursorKind::List,
                serde_json::to_string(&walk).map_err(|_| ProviderError::Invalid)?,
            )?)
        };
        self.require_descendant(&current, context, &mut spent)
            .await?;
        let observed = now()?;
        let coverage = if next.is_some() {
            Coverage::Partial {
                reason: DisplayText::new("Continue loading selected Drive folders")?,
            }
        } else {
            Coverage::MetadataCompleteSelected {
                observed_at_ms: observed,
            }
        };
        MetadataPage::new(rows, next, observed, coverage)
    }
}
