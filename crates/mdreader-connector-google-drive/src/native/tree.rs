//! Bounded mixed metadata pages, then depth-first children. The signed cursor
//! owns every deferred child; bodies are never fetched while listing.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
const MAX_DEPTH: usize = 64;
const MAX_PENDING: usize = 256;
const MAX_SIGNED_CURSOR_BYTES: usize = 32 * 1024;
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    folder: String,
    token: String,
    exhausted: bool,
    pending: VecDeque<String>,
}
impl Frame {
    fn new(folder: String) -> Self {
        Self {
            folder,
            token: String::new(),
            exhausted: false,
            pending: VecDeque::new(),
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
        if !matches!(self.v, 3 | 4)
            || self.root != root.as_str()
            || self.stack.is_empty()
            || self.stack.len() > MAX_DEPTH
            || self.stack[0].folder != self.root
        {
            return Err(ProviderError::Invalid);
        }
        let mut unique = BTreeSet::new();
        for frame in &self.stack {
            folder::valid_id(&RecordKey::new(frame.folder.clone())?)?;
            if !unique.insert(&frame.folder)
                || frame.token.len() > 2048
                || frame.token.chars().any(char::is_control)
                || frame.exhausted && !frame.token.is_empty()
            {
                return Err(ProviderError::Invalid);
            }
        }
        for child in self.stack.iter().flat_map(|f| &f.pending) {
            folder::valid_id(&RecordKey::new(child.clone())?)?;
            if !unique.insert(child) {
                return Err(ProviderError::Invalid);
            }
        }
        if self.pending() > MAX_PENDING || self.stack.last().is_some_and(|f| f.exhausted) {
            return Err(ProviderError::Invalid);
        }
        Ok(())
    }
    fn pending(&self) -> usize {
        self.stack.iter().map(|f| f.pending.len()).sum()
    }
    fn cursor(&self, codec: &CursorCodec, binding: &SourceBinding) -> Result<ProviderCursor> {
        let cursor = codec.encode_traversal(
            ProviderKind::GoogleDrive,
            binding,
            CursorKind::List,
            serde_json::to_string(self).map_err(|_| ProviderError::Invalid)?,
        )?;
        if cursor.as_bytes().len() > MAX_SIGNED_CURSOR_BYTES {
            return Err(ProviderError::TooLarge);
        }
        Ok(cursor)
    }
    fn page_rows(
        &self,
        context: &IoContext,
        codec: &CursorCodec,
        binding: &SourceBinding,
    ) -> Result<usize> {
        let frame = self.stack.last().ok_or(ProviderError::Invalid)?;
        let traversal = serde_json::to_string(self).map_err(|_| ProviderError::Invalid)?;
        let escaped = serde_json::to_vec(&traversal)
            .map_err(|_| ProviderError::Invalid)?
            .len();
        let cursor = self.cursor(codec, binding)?;
        // Decode length of canonical unpadded base64, without exposing content.
        // This includes the signed envelope, binding and HMAC, whose size may
        // differ between sources. Reserve against the actual envelope limit.
        let envelope = cursor.as_bytes().len() * 3 / 4 - escaped;
        let old_token = serde_json::to_string(&frame.token).map_err(|_| ProviderError::Invalid)?;
        let escaped_token = serde_json::to_vec(&old_token)
            .map_err(|_| ProviderError::Invalid)?
            .len()
            - 2;
        // A 2,048-byte token may escape twice: once in the traversal and again
        // in the signed payload. ASCII IDs are <=256 bytes; reserve 264 bytes
        // per deferred child and 512 for the next depth-first frame.
        let retained = escaped.saturating_sub(escaped_token);
        let budget =
            (MAX_SIGNED_CURSOR_BYTES / 4 * 3).saturating_sub(envelope + retained + 8196 + 512);
        let rows = context
            .page_rows()
            .min(MAX_PENDING.saturating_sub(self.pending()))
            .min(budget / 264);
        if rows == 0 {
            return Err(ProviderError::TooLarge);
        }
        Ok(rows)
    }
    fn advance(&mut self) -> Result<()> {
        while let Some(frame) = self.stack.last_mut() {
            if let Some(child) = frame.pending.pop_front() {
                if self.stack.len() == MAX_DEPTH {
                    return Err(ProviderError::TooLarge);
                }
                self.stack.push(Frame::new(child));
                break;
            }
            if !frame.exhausted {
                break;
            }
            self.stack.pop();
        }
        Ok(())
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
                // Restart only disposable staging. Published copies and grants stay.
                if shape.get("v").is_some_and(|v| v == 2)
                    || shape.get("v").is_none()
                        && shape.get("queue").is_some()
                        && shape.get("seen").is_some()
                        && shape.get("current").is_some()
                {
                    return Err(ProviderError::ResetCursor);
                }
                if cursor.as_bytes().len() > MAX_SIGNED_CURSOR_BYTES {
                    return Err(ProviderError::TooLarge);
                }
                serde_json::from_value(shape).map_err(|_| ProviderError::Invalid)
            })
            .transpose()?;
        let mut spent = 0;
        let root = self.root_id(context, &mut spent).await?;
        let mut walk = decoded.unwrap_or_else(|| Walk {
            v: 4,
            root: root.as_str().into(),
            stack: vec![Frame::new(root.as_str().into())],
        });
        walk.validate(&root)?;
        if !scope.recursive && (walk.stack.len() != 1 || walk.pending() != 0) {
            return Err(ProviderError::Unauthorized);
        }
        let frame = walk.stack.last().ok_or(ProviderError::Invalid)?;
        let current = RecordKey::new(frame.folder.clone())?;
        if current != root {
            // The root was just read in this operation. Do not fetch it twice.
            self.ancestry(vec![current.clone()], &root, context, &mut spent)
                .await?;
        }
        let page_rows = walk.page_rows(context, &self.codec, self.descriptor.binding())?;
        let query = format!("trashed = false and '{}' in parents", current.as_str());
        let mut url = url::Url::parse("https://www.googleapis.com/drive/v3/files")
            .map_err(|_| ProviderError::Invalid)?;
        url.query_pairs_mut().append_pair("q", &query).append_pair("spaces", "drive")
            .append_pair("corpora", "user").append_pair("supportsAllDrives", "true")
            .append_pair("includeItemsFromAllDrives", "true")
            .append_pair("pageSize", &page_rows.to_string())
            .append_pair("fields", "files(id,name,mimeType,version,md5Checksum,size,modifiedTime,trashed,parents),nextPageToken,incompleteSearch");
        // Retain v3 continuation query semantics rather than restarting an
        // already-long scan. Fresh v4 traversals request recent items first.
        if walk.v == 4 {
            url.query_pairs_mut()
                .append_pair("orderBy", "modifiedTime desc,name");
        }
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
        let mut children = VecDeque::new();
        for file in page.files {
            let key = RecordKey::new(file.id.clone())?;
            folder::valid_id(&key)?;
            if !unique.insert(key.clone())
                || file.trashed
                || !membership::parents(&file.parents)?.contains(&current)
            {
                return Err(ProviderError::Protocol);
            }
            if file.mime_type == FOLDER_MIME {
                if walk
                    .stack
                    .iter()
                    .any(|f| f.folder == file.id || f.pending.contains(&file.id))
                {
                    return Err(ProviderError::Protocol);
                }
                if scope.recursive {
                    children.push_back(file.id);
                }
            } else if markdown(&file.name) && downloadable(&file.mime_type) {
                file.validate(&key)?;
                let version = file.version(None)?;
                let mut metadata = file.metadata(version)?;
                metadata.display_path =
                    DisplayText::new(format!("/{}/{}", scope.root.as_str(), key.as_str()))?;
                rows.push(metadata);
            }
        }
        let frame = walk.stack.last_mut().ok_or(ProviderError::Invalid)?;
        frame.pending.extend(children);
        frame.exhausted = page.next_page_token.is_none();
        frame.token = page.next_page_token.unwrap_or_default();
        walk.advance()?;
        let next = if walk.stack.is_empty() {
            None
        } else {
            walk.validate(&root)?;
            Some(walk.cursor(&self.codec, self.descriptor.binding())?)
        };
        // Fresh root + ancestry after the physical request, including empty pages.
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

#[cfg(test)]
#[path = "tree/budget_tests.rs"]
mod budget_tests;
