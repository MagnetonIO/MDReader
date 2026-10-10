//! Account change feed filtered to the exact retained selection. Structural
//! work is carried by a signed continuation, never by a process-local queue.
use super::*;
use serde::{Deserialize, Serialize};
#[path = "changes/known.rs"]
mod known;

const MAX_CHANGE_ROWS: usize = 32;
const MAX_CURSOR: usize = 32 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    v: u8,
    token: String,
    resume: Option<Resume>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resume {
    page_digest: String,
    index: usize,
    subtree: Option<Subtree>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Subtree {
    root: RecordKey,
    after: Option<RecordKey>,
    walking: bool,
    rescan: bool,
    walk: Option<serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    file_id: String,
    #[serde(default)]
    removed: bool,
    file: Option<ChangedFile>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangedFile {
    id: String,
    name: String,
    mime_type: String,
    #[serde(default)]
    trashed: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    changes: Vec<Item>,
    next_page_token: Option<String>,
    new_start_page_token: Option<String>,
}
impl Client {
    fn change_cursor(&self, state: &Cursor) -> Result<ProviderCursor> {
        let result = self.codec.encode_traversal(
            ProviderKind::GoogleDrive,
            self.descriptor.binding(),
            CursorKind::Changes,
            serde_json::to_string(state).map_err(|_| ProviderError::Protocol)?,
        )?;
        if result.as_bytes().len() > MAX_CURSOR {
            return Err(ProviderError::TooLarge);
        }
        Ok(result)
    }
    fn change_state(&self, cursor: &ProviderCursor) -> Result<Cursor> {
        if cursor.as_bytes().len() > MAX_CURSOR {
            return Err(ProviderError::TooLarge);
        }
        let raw = self.codec.decode_traversal(
            ProviderKind::GoogleDrive,
            self.descriptor.binding(),
            CursorKind::Changes,
            cursor,
        )?;
        let state = if raw.starts_with('{') {
            serde_json::from_str::<Cursor>(&raw).map_err(|_| ProviderError::Invalid)?
        } else {
            Cursor {
                v: 1,
                token: raw,
                resume: None,
            }
        };
        if state.v != 1
            || state.token.is_empty()
            || state.token.len() > 2048
            || state.token.chars().any(char::is_control)
            || state.resume.as_ref().is_some_and(|r| {
                r.index > MAX_CHANGE_ROWS
                    || r.page_digest.len() != 64
                    || r.subtree.as_ref().is_some_and(|s| {
                        s.after
                            .as_ref()
                            .is_some_and(|key| folder::valid_id(key).is_err())
                    })
            })
        {
            return Err(ProviderError::Invalid);
        }
        Ok(state)
    }
    async fn event_for(&self, key: RecordKey, context: &IoContext) -> Result<Option<Change>> {
        match self.metadata(&key, context).await {
            Ok(_) => Ok(Some(Change::Upsert { key })),
            Err(
                ProviderError::NotFound | ProviderError::Unsupported | ProviderError::Unauthorized,
            ) => {
                // Loss of the source grant is not a file tombstone.
                self.authority.revalidate(self.descriptor.binding()).await?;
                Ok(Some(Change::Deleted {
                    key,
                    expected_path: None,
                }))
            }
            Err(error) => Err(error),
        }
    }
    async fn subtree_page(&self, state: &mut Cursor, context: &IoContext) -> Result<ChangeOutcome> {
        let subtree = state
            .resume
            .as_mut()
            .and_then(|r| r.subtree.as_mut())
            .ok_or(ProviderError::Protocol)?;
        let mut events = Vec::new();
        if !subtree.walking {
            let (keys, more) = self.affected_keys(
                &subtree.root,
                subtree.after.as_ref(),
                context.page_rows().min(8),
            )?;
            for key in keys {
                subtree.after = Some(key.clone());
                if let Some(event) = self.event_for(key, context).await? {
                    events.push(event);
                }
            }
            subtree.walking = !more;
            if subtree.walking && !subtree.rescan {
                state
                    .resume
                    .as_mut()
                    .ok_or(ProviderError::Protocol)?
                    .subtree = None;
            }
        } else {
            let cursor = subtree
                .walk
                .as_ref()
                .map(|walk| {
                    self.codec.encode_traversal(
                        ProviderKind::GoogleDrive,
                        self.descriptor.binding(),
                        CursorKind::List,
                        serde_json::to_string(walk).map_err(|_| ProviderError::Protocol)?,
                    )
                })
                .transpose()?;
            let page = self
                .list_tree_at(cursor.as_ref(), context, Some(&subtree.root))
                .await?;
            events.extend(page.rows().iter().map(|row| Change::Upsert {
                key: row.key.clone(),
            }));
            if let Some(next) = page.next_cursor() {
                let walk = self.codec.decode_traversal(
                    ProviderKind::GoogleDrive,
                    self.descriptor.binding(),
                    CursorKind::List,
                    next,
                )?;
                subtree.walk =
                    Some(serde_json::from_str(&walk).map_err(|_| ProviderError::Protocol)?);
            } else {
                state
                    .resume
                    .as_mut()
                    .ok_or(ProviderError::Protocol)?
                    .subtree = None;
            }
        }
        ChangeOutcome::page(events, Some(self.change_cursor(state)?), None)
    }
    pub(super) async fn changed_metadata(
        &self,
        cursor: &ProviderCursor,
        context: &IoContext,
    ) -> Result<ChangeOutcome> {
        if !self.descriptor.capabilities().changes {
            return Err(ProviderError::Unsupported);
        }
        let mut state = self.change_state(cursor)?;
        if self.folder.is_some() {
            let mut spent = 0;
            self.root_id(context, &mut spent).await?;
        }
        if state.resume.as_ref().is_some_and(|r| r.subtree.is_some()) {
            return self.subtree_page(&mut state, context).await;
        }
        let mut url = url::Url::parse("https://www.googleapis.com/drive/v3/changes")
            .map_err(|_| ProviderError::Invalid)?;
        url.query_pairs_mut().append_pair("pageToken", &state.token)
            .append_pair("pageSize", &context.page_rows().min(MAX_CHANGE_ROWS).to_string())
            .append_pair("supportsAllDrives", "true").append_pair("includeItemsFromAllDrives", "true")
            .append_pair("includeRemoved", "true").append_pair("includeCorpusRemovals", "true")
            .append_pair("spaces", "drive")
            .append_pair("fields", "changes(fileId,removed,file(id,name,mimeType,trashed)),nextPageToken,newStartPageToken");
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
        if response.status() == 410 {
            return Ok(ChangeOutcome::RebuildRequired);
        }
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let page: Page =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        if page.changes.len() > context.page_rows().min(MAX_CHANGE_ROWS) {
            return Err(ProviderError::TooLarge);
        }
        let fence = serde_json::json!({"keys": page.changes.iter().map(|item| (&item.file_id, item.removed, item.file.as_ref().is_some_and(|f| f.mime_type == "application/vnd.google-apps.folder"))).collect::<Vec<_>>(), "next": page.next_page_token, "checkpoint": page.new_start_page_token});
        let fence = serde_json::to_vec(&fence).map_err(|_| ProviderError::Protocol)?;
        let digest = body_hash(&fence)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let start = match &state.resume {
            Some(resume) if resume.page_digest == digest && resume.index <= page.changes.len() => {
                resume.index
            }
            Some(_) => return Ok(ChangeOutcome::RebuildRequired),
            None => 0,
        };
        let mut events = Vec::new();
        for (index, item) in page.changes.iter().enumerate().skip(start) {
            let key = RecordKey::new(item.file_id.clone())?;
            folder::valid_id(&key)?;
            if item.file.as_ref().is_some_and(|f| f.id != key.as_str()) {
                return Err(ProviderError::IdentityMismatch);
            }
            if !item.removed && item.file.is_none() {
                return Err(ProviderError::Protocol);
            }
            if self.folder.is_none() {
                if self.selected(&key).is_err() {
                    continue;
                }
                events.push(
                    if item.removed
                        || item.file.as_ref().is_some_and(|f| {
                            f.trashed || !markdown(&f.name) || !downloadable(&f.mime_type)
                        })
                    {
                        Change::Deleted {
                            key,
                            expected_path: None,
                        }
                    } else {
                        Change::Upsert { key }
                    },
                );
                continue;
            }
            let scope = self.folder.as_ref().ok_or(ProviderError::Protocol)?;
            let is_folder = item
                .file
                .as_ref()
                .is_some_and(|f| f.mime_type == "application/vnd.google-apps.folder");
            let (had_descendants, missing_ancestry) = self.known_structure(&key)?;
            if !scope.account_wide
                && (is_folder || had_descendants || item.removed && missing_ancestry)
            {
                if missing_ancestry {
                    return Ok(ChangeOutcome::RebuildRequired);
                }
                let mut spent = 0;
                let root = self.root_id(context, &mut spent).await?;
                if key == root {
                    continue;
                }
                let rescan = if is_folder
                    && !item.removed
                    && !item.file.as_ref().is_some_and(|f| f.trashed)
                    && scope.recursive
                {
                    match self
                        .ancestry(vec![key.clone()], &root, context, &mut spent)
                        .await
                    {
                        Ok(()) => true,
                        Err(ProviderError::Unauthorized | ProviderError::NotFound) => {
                            self.authority.revalidate(self.descriptor.binding()).await?;
                            false
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    false
                };
                if !had_descendants && !rescan {
                    continue;
                }
                state.resume = Some(Resume {
                    page_digest: digest.clone(),
                    index: index + 1,
                    subtree: Some(Subtree {
                        root: key,
                        after: None,
                        walking: false,
                        rescan,
                        walk: None,
                    }),
                });
                return ChangeOutcome::page(events, Some(self.change_cursor(&state)?), None);
            }
            if item.removed
                || item
                    .file
                    .as_ref()
                    .is_some_and(|f| f.trashed || !markdown(&f.name) || !downloadable(&f.mime_type))
            {
                events.push(Change::Deleted {
                    key,
                    expected_path: None,
                });
            } else if let Some(event) = self.event_for(key, context).await? {
                events.push(event);
            }
        }
        let next = page
            .next_page_token
            .map(|s| self.cursor(CursorKind::Changes, &s))
            .transpose()?;
        let checkpoint = page
            .new_start_page_token
            .map(|s| self.cursor(CursorKind::Changes, &s))
            .transpose()?;
        ChangeOutcome::page(events, next, checkpoint)
    }
}
