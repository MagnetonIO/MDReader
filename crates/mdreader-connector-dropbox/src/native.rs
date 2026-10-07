//! CatDB-free Dropbox API v2 kernel. Exact files and explicitly selected
//! app-folder roots are supported. Broad imported authorization stays legacy.
use async_trait::async_trait;
use mdreader_provider::{
    transport::{self, Method, Request, Response, Transport},
    *,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct AccessGrant {
    token: Zeroizing<String>,
    expires_at: SystemTime,
    generation: u64,
}
impl AccessGrant {
    pub fn new(token: String, expires_at: SystemTime, generation: u64) -> Result<Self> {
        if token.is_empty()
            || token.len() > 8192
            || token.chars().any(char::is_control)
            || generation == 0
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            token: Zeroizing::new(token),
            expires_at,
            generation,
        })
    }
}
impl std::fmt::Debug for AccessGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccessGrant([redacted])")
    }
}
/// The platform owner exchanges/refreshes credentials in its own custody. These
/// methods may only return an app-folder grant for the exact admitted account.
#[async_trait]
pub trait CredentialResolver: Send + Sync {
    async fn access(&self, binding: &SourceBinding, context: &IoContext) -> Result<AccessGrant>;
    async fn refresh(
        &self,
        binding: &SourceBinding,
        previous_generation: u64,
        context: &IoContext,
    ) -> Result<AccessGrant>;
}
#[derive(Clone)]
pub struct SelectedFile {
    key: RecordKey,
    path: RecordKey,
}
impl SelectedFile {
    pub fn new(key: RecordKey, path: RecordKey) -> Result<Self> {
        if !key.as_str().starts_with("id:") || key.as_str().len() < 4 || !valid_path(path.as_str())
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self { key, path })
    }
}
#[derive(Clone)]
pub enum Selection {
    ExactFiles(Vec<SelectedFile>),
    AppFolderRoot { path: RecordKey, recursive: bool },
}
pub struct Client {
    descriptor: SourceDescriptor,
    selection: Selection,
    resolver: Arc<dyn CredentialResolver>,
    authority: Arc<dyn ProviderAuthority>,
    transport: Arc<dyn Transport>,
    codec: CursorCodec,
    credentials: tokio::sync::Mutex<Option<AccessGrant>>,
    membership: std::sync::Mutex<BTreeMap<RecordKey, RecordKey>>,
}
impl Client {
    pub fn new(
        descriptor: SourceDescriptor,
        selection: Selection,
        resolver: Arc<dyn CredentialResolver>,
        authority: Arc<dyn ProviderAuthority>,
        transport: Arc<dyn Transport>,
        cursor_key: [u8; 32],
    ) -> Result<Self> {
        let scope = match &selection {
            Selection::ExactFiles(files) => {
                SelectedScope::exact(files.iter().map(|f| f.key.clone()).collect())?
            }
            Selection::AppFolderRoot { path, recursive } => {
                if !valid_path(path.as_str()) {
                    return Err(ProviderError::Invalid);
                }
                SelectedScope::ScopedRoot {
                    root: path.clone(),
                    recursive: *recursive,
                }
            }
        };
        if descriptor.kind() != ProviderKind::Dropbox
            || descriptor.scope() != &scope
            || descriptor.capabilities().assets
            || !descriptor.capabilities().read
            || !descriptor.capabilities().list
            || matches!(selection, Selection::ExactFiles(_)) && descriptor.capabilities().changes
        {
            return Err(ProviderError::Invalid);
        }
        let membership = match &selection {
            Selection::ExactFiles(files) => files
                .iter()
                .map(|f| (f.path.clone(), f.key.clone()))
                .collect(),
            _ => BTreeMap::new(),
        };
        Ok(Self {
            descriptor,
            selection,
            resolver,
            authority,
            transport,
            codec: CursorCodec::new(cursor_key),
            credentials: tokio::sync::Mutex::new(None),
            membership: std::sync::Mutex::new(membership),
        })
    }
    /// Restore only owner-persisted locator membership for this exact current
    /// binding. Fresh metadata/downloads still verify the actual selected root.
    pub fn restore_membership(
        &self,
        binding: &SourceBinding,
        files: &[SelectedFile],
    ) -> Result<()> {
        if binding != self.descriptor.binding()
            || !matches!(self.selection, Selection::AppFolderRoot { .. })
            || files.len() > 16384
        {
            return Err(ProviderError::Unauthorized);
        }
        let mut membership = self
            .membership
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        let mut next = membership.clone();
        for file in files {
            if !self.allowed_path(file.path.as_str()) {
                return Err(ProviderError::Unauthorized);
            }
            if next
                .insert(file.path.clone(), file.key.clone())
                .is_some_and(|key| key != file.key)
            {
                return Err(ProviderError::IdentityMismatch);
            }
        }
        if next.len() > 16384 {
            return Err(ProviderError::TooLarge);
        }
        *membership = next;
        Ok(())
    }
    fn known(&self, key: &RecordKey) -> Result<()> {
        if !self
            .membership
            .lock()
            .map_err(|_| ProviderError::Unavailable)?
            .values()
            .any(|k| k == key)
        {
            return Err(ProviderError::Unauthorized);
        }
        Ok(())
    }
    fn allowed_path(&self, path: &str) -> bool {
        if !valid_path(path) {
            return false;
        }
        match &self.selection {
            Selection::ExactFiles(_) => true,
            Selection::AppFolderRoot {
                path: root,
                recursive,
            } => {
                let root = root.as_str().trim_end_matches('/');
                path.starts_with(&format!("{root}/"))
                    && (*recursive || !path[root.len() + 1..].contains('/'))
            }
        }
    }
    fn observe(&self, files: &[File]) -> Result<()> {
        let mut entries = self
            .membership
            .lock()
            .map_err(|_| ProviderError::Unavailable)?;
        let mut next = entries.clone();
        for file in files {
            if !self.allowed_path(&file.path_lower) {
                return Err(ProviderError::Unauthorized);
            }
            next.insert(
                RecordKey::new(file.path_lower.clone())?,
                RecordKey::new(file.id.clone())?,
            );
        }
        if next.len() > 16384 {
            return Err(ProviderError::TooLarge);
        }
        *entries = next;
        Ok(())
    }
    async fn grant(&self, context: &IoContext, refresh: Option<u64>) -> Result<AccessGrant> {
        context.check()?;
        let operation = async {
            let mut slot = self.credentials.lock().await;
            if let Some(grant) = slot.as_ref() {
                if grant.expires_at > SystemTime::now() && refresh != Some(grant.generation) {
                    return Ok(grant.clone());
                }
            }
            let next = if let Some(previous) = slot.as_ref() {
                self.resolver
                    .refresh(self.descriptor.binding(), previous.generation, context)
                    .await?
            } else {
                self.resolver
                    .access(self.descriptor.binding(), context)
                    .await?
            };
            if next.expires_at <= SystemTime::now()
                || slot
                    .as_ref()
                    .is_some_and(|previous| next.generation <= previous.generation)
            {
                return Err(ProviderError::ReconnectRequired);
            }
            *slot = Some(next.clone());
            Ok(next)
        };
        tokio::select! {biased;_=context.cancellation().cancelled()=>Err(ProviderError::Cancelled),result=tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),operation)=>result.map_err(|_|ProviderError::Timeout)?}
    }
    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        method: Method,
        url: String,
        extras: BTreeMap<String, String>,
        body: Vec<u8>,
        context: &IoContext,
        mutation: Option<uuid::Uuid>,
        spent: &mut usize,
    ) -> Result<Response> {
        revalidate_authority(&*self.authority, self.descriptor.binding(), context, None).await?;
        context.check()?;
        let mut grant = self.grant(context, None).await?;
        for attempt in 0..2 {
            // Credential resolution/refresh can suspend; revalidate admission
            // again immediately before the physical provider request.
            revalidate_authority(&*self.authority, self.descriptor.binding(), context, None)
                .await?;
            let mut headers = extras.clone();
            headers.insert(
                "authorization".into(),
                format!("Bearer {}", grant.token.as_str()),
            );
            let request = Request::new(method, url.clone(), headers, body.clone())?;
            let response = transport::send(&*self.transport, request, context, mutation).await?;
            *spent = spent.saturating_add(
                response.body().len()
                    + response
                        .headers()
                        .iter()
                        .map(|(k, v)| k.len() + v.len())
                        .sum::<usize>(),
            );
            if *spent > context.byte_limit() {
                return Err(mutation
                    .map(|operation_id| ProviderError::UnknownWrite { operation_id })
                    .unwrap_or(ProviderError::TooLarge));
            }
            revalidate_authority(
                &*self.authority,
                self.descriptor.binding(),
                context,
                mutation,
            )
            .await?;
            if response.status() == 401 && mutation.is_none() && attempt == 0 {
                grant = self.grant(context, Some(grant.generation)).await?;
                continue;
            }
            return Ok(response);
        }
        Err(ProviderError::ReconnectRequired)
    }

    async fn api(
        &self,
        path: &str,
        body: serde_json::Value,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<Response> {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".into(), "application/json".into());
        self.send(
            Method::Post,
            format!("https://api.dropboxapi.com/2/files/{path}"),
            headers,
            serde_json::to_vec(&body).map_err(|_| ProviderError::Invalid)?,
            context,
            None,
            spent,
        )
        .await
    }
    async fn fetch_metadata(
        &self,
        key: &RecordKey,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<File> {
        self.known(key)?;
        let response = self
            .api(
                "get_metadata",
                serde_json::json!({"path":key.as_str(),"include_deleted":false}),
                context,
                spent,
            )
            .await?;
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let file: File =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        file.validate()?;
        if file.id != key.as_str() {
            return Err(ProviderError::IdentityMismatch);
        }
        if !self.allowed_path(&file.path_lower) {
            return Err(ProviderError::Unauthorized);
        }
        Ok(file)
    }
    fn cursor(&self, kind: CursorKind, token: &str) -> Result<ProviderCursor> {
        self.codec.encode(
            ProviderKind::Dropbox,
            self.descriptor.binding(),
            kind,
            OpaqueValue::new(token)?,
        )
    }
    fn root_arguments(&self, limit: usize) -> Result<serde_json::Value> {
        match &self.selection {
            Selection::AppFolderRoot { path, recursive } => Ok(
                serde_json::json!({"path":if path.as_str()=="/"{""}else{path.as_str()},"recursive":recursive,"include_deleted":false,"include_non_downloadable_files":false,"limit":limit}),
            ),
            _ => Err(ProviderError::Unsupported),
        }
    }
}
#[derive(Clone, Deserialize)]
struct File {
    #[serde(rename = ".tag")]
    tag: Option<String>,
    id: String,
    name: String,
    path_lower: String,
    rev: String,
    content_hash: Option<String>,
    size: u64,
    server_modified: Option<String>,
}
impl File {
    fn validate(&self) -> Result<()> {
        if self.tag.as_deref().is_some_and(|s| s != "file")
            || !self.id.starts_with("id:")
            || self.id.len() < 4
            || !valid_path(&self.path_lower)
            || self.rev.len() < 9
            || self.rev.len() > 256
            || !self.rev.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ProviderError::Protocol);
        }
        RecordKey::new(self.id.clone())?;
        DisplayText::new(self.name.clone())?;
        Ok(())
    }
    fn version(&self) -> Result<DocumentVersion> {
        Ok(DocumentVersion {
            provider_revision: OpaqueValue::new(self.rev.clone())?,
            content_hash: self
                .content_hash
                .as_deref()
                .map(|s| ContentHash::from_hex(HashAlgorithm::DropboxContentHash, s))
                .transpose()?,
            conditional_write_token: Some(OpaqueValue::new(self.rev.clone())?),
        })
    }
    fn metadata(self) -> Result<Metadata> {
        let version = self.version()?;
        let modified_at_ms = self
            .server_modified
            .map(|s| {
                time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
                    .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
                    .map_err(|_| ProviderError::Protocol)
            })
            .transpose()?;
        Ok(Metadata {
            key: RecordKey::new(self.id)?,
            name: DisplayText::new(self.name)?,
            display_path: DisplayText::new(self.path_lower)?,
            media_type: Some(DisplayText::new("text/markdown")?),
            modified_at_ms,
            size_bytes: Some(self.size),
            version,
        })
    }
}
fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 4096
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && !path.split('/').any(|p| p == "." || p == "..")
        && (path == "/" || !path.ends_with('/'))
}
fn markdown(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}
fn now() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .map_err(|_| ProviderError::Protocol)
}
#[derive(Deserialize)]
struct Page {
    entries: Vec<serde_json::Value>,
    cursor: String,
    has_more: bool,
}
#[async_trait]
impl DocumentProvider for Client {
    fn descriptor(&self) -> &SourceDescriptor {
        &self.descriptor
    }
    async fn metadata(&self, key: &RecordKey, context: &IoContext) -> Result<Metadata> {
        let mut spent = 0;
        self.fetch_metadata(key, context, &mut spent)
            .await?
            .metadata()
    }

    async fn list(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<MetadataPage> {
        let mut spent = 0;
        if let Selection::ExactFiles(files) = &self.selection {
            let offset = cursor
                .map(|cursor| {
                    self.codec
                        .decode(
                            ProviderKind::Dropbox,
                            self.descriptor.binding(),
                            CursorKind::List,
                            cursor,
                        )
                        .and_then(|v| {
                            v.as_str()
                                .parse::<usize>()
                                .map_err(|_| ProviderError::Invalid)
                        })
                })
                .transpose()?
                .unwrap_or(0);
            if offset >= files.len() {
                return Err(ProviderError::Invalid);
            }
            let end = offset.saturating_add(context.page_rows()).min(files.len());
            let mut rows = Vec::new();
            for selected in &files[offset..end] {
                let file = self
                    .fetch_metadata(&selected.key, context, &mut spent)
                    .await?;
                if !markdown(&file.name) {
                    return Err(ProviderError::Unsupported);
                }
                rows.push(file.metadata()?);
            }
            let next = if end < files.len() {
                Some(self.cursor(CursorKind::List, &end.to_string())?)
            } else {
                None
            };
            let observed = now()?;
            let coverage = if next.is_none() {
                Coverage::MetadataCompleteSelected {
                    observed_at_ms: observed,
                }
            } else {
                Coverage::Partial {
                    reason: DisplayText::new("Selected-file traversal continues")?,
                }
            };
            return MetadataPage::new(rows, next, observed, coverage);
        }
        let response = if let Some(cursor) = cursor {
            let token = self.codec.decode(
                ProviderKind::Dropbox,
                self.descriptor.binding(),
                CursorKind::List,
                cursor,
            )?;
            self.api(
                "list_folder/continue",
                serde_json::json!({"cursor":token.as_str()}),
                context,
                &mut spent,
            )
            .await?
        } else {
            self.api(
                "list_folder",
                self.root_arguments(context.page_rows())?,
                context,
                &mut spent,
            )
            .await?
        };
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let page: Page =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        if page.entries.len() > context.page_rows() {
            return Err(ProviderError::TooLarge);
        }
        let mut files = Vec::new();
        for entry in page.entries {
            match entry.get(".tag").and_then(|v| v.as_str()) {
                Some("file") => {
                    let file: File =
                        serde_json::from_value(entry).map_err(|_| ProviderError::Protocol)?;
                    file.validate()?;
                    if !self.allowed_path(&file.path_lower) {
                        return Err(ProviderError::Unauthorized);
                    }
                    if markdown(&file.name) {
                        files.push(file);
                    }
                }
                Some("folder") => {}
                _ => return Err(ProviderError::Protocol),
            }
        }
        self.observe(&files)?;
        let rows = files
            .into_iter()
            .map(File::metadata)
            .collect::<Result<Vec<_>>>()?;
        let observed = now()?;
        let next = if page.has_more {
            Some(self.cursor(CursorKind::List, &page.cursor)?)
        } else {
            None
        };
        let coverage = if next.is_some() {
            Coverage::Partial {
                reason: DisplayText::new("Selected-root traversal continues")?,
            }
        } else {
            Coverage::MetadataCompleteSelected {
                observed_at_ms: observed,
            }
        };
        MetadataPage::new(rows, next, observed, coverage)
    }
    async fn read(&self, key: &RecordKey, context: &IoContext) -> Result<DocumentBody> {
        self.known(key)?;
        let mut spent = 0;
        let mut headers = BTreeMap::new();
        headers.insert(
            "dropbox-api-arg".into(),
            serde_json::to_string(&serde_json::json!({"path":key.as_str()}))
                .map_err(|_| ProviderError::Invalid)?,
        );
        let response = self
            .send(
                Method::Post,
                "https://content.dropboxapi.com/2/files/download".into(),
                headers,
                vec![],
                context,
                None,
                &mut spent,
            )
            .await?;
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let file: File = serde_json::from_str(
            response
                .headers()
                .get("dropbox-api-result")
                .ok_or(ProviderError::Protocol)?,
        )
        .map_err(|_| ProviderError::Protocol)?;
        file.validate()?;
        if file.id != key.as_str() {
            return Err(ProviderError::IdentityMismatch);
        }
        if !self.allowed_path(&file.path_lower) {
            return Err(ProviderError::Unauthorized);
        }
        if !markdown(&file.name) {
            return Err(ProviderError::Unsupported);
        }
        if file.size != response.body().len() as u64 {
            return Err(ProviderError::Protocol);
        }
        let version = file.version()?;
        let body = MarkdownBytes::new(response.into_body())?;
        if let Some(hash) = &version.content_hash {
            verify_content_hash(body.as_bytes(), hash)?;
        }
        Ok(DocumentBody {
            key: key.clone(),
            version,
            body,
            observed_at_ms: now()?,
        })
    }
    async fn start_changes(&self, context: &IoContext) -> Result<ProviderCursor> {
        if !self.descriptor.capabilities().changes {
            return Err(ProviderError::Unsupported);
        }
        let mut arguments = self.root_arguments(context.page_rows())?;
        arguments
            .as_object_mut()
            .ok_or(ProviderError::Invalid)?
            .remove("limit");
        let mut spent = 0;
        let response = self
            .api(
                "list_folder/get_latest_cursor",
                arguments,
                context,
                &mut spent,
            )
            .await?;
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        #[derive(Deserialize)]
        struct Start {
            cursor: String,
        }
        let start: Start =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        self.cursor(CursorKind::Changes, &start.cursor)
    }
    async fn changes(&self, cursor: &ProviderCursor, context: &IoContext) -> Result<ChangeOutcome> {
        if !self.descriptor.capabilities().changes {
            return Err(ProviderError::Unsupported);
        }
        let token = self.codec.decode(
            ProviderKind::Dropbox,
            self.descriptor.binding(),
            CursorKind::Changes,
            cursor,
        )?;
        let mut spent = 0;
        let response = self
            .api(
                "list_folder/continue",
                serde_json::json!({"cursor":token.as_str()}),
                context,
                &mut spent,
            )
            .await?;
        if response.status() == 409 {
            let value: serde_json::Value =
                serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
            if value
                .get("error")
                .and_then(|v| v.get(".tag"))
                .and_then(|v| v.as_str())
                == Some("reset")
            {
                return Ok(ChangeOutcome::RebuildRequired);
            }
        }
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let page: Page =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        if page.entries.len() > context.page_rows()
            || page.has_more && page.cursor == token.as_str()
        {
            return Err(ProviderError::Protocol);
        }
        let mut events = Vec::new();
        let mut files = Vec::new();
        for entry in page.entries {
            match entry.get(".tag").and_then(|v| v.as_str()) {
                Some("file") => {
                    let file: File =
                        serde_json::from_value(entry).map_err(|_| ProviderError::Protocol)?;
                    file.validate()?;
                    if !self.allowed_path(&file.path_lower) {
                        return Err(ProviderError::Unauthorized);
                    }
                    let key = RecordKey::new(file.id.clone())?;
                    events.push(if markdown(&file.name) {
                        Change::Upsert { key }
                    } else {
                        Change::Deleted {
                            key,
                            expected_path: None,
                        }
                    });
                    files.push(file);
                }
                Some("deleted") => {
                    let path = entry
                        .get("path_lower")
                        .and_then(|v| v.as_str())
                        .ok_or(ProviderError::Protocol)?;
                    if !self.allowed_path(path) {
                        return Err(ProviderError::Unauthorized);
                    }
                    let path = RecordKey::new(path)?;
                    if let Some(key) = self
                        .membership
                        .lock()
                        .map_err(|_| ProviderError::Unavailable)?
                        .get(&path)
                        .cloned()
                    {
                        events.push(Change::Deleted {
                            key,
                            expected_path: Some(path.clone()),
                        });
                    }
                }
                Some("folder") => {}
                _ => return Err(ProviderError::Protocol),
            }
        }
        self.observe(&files)?;
        let next = self.cursor(CursorKind::Changes, &page.cursor)?;
        if page.has_more {
            ChangeOutcome::page(events, Some(next), None)
        } else {
            ChangeOutcome::page(events, None, Some(next))
        }
    }
    async fn guarded_write(
        &self,
        permit: journal::DispatchPermit,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<WriteReceipt> {
        permit.validate(self.descriptor.binding(), body)?;
        self.known(permit.key())?;
        if !self.descriptor.capabilities().guarded_write {
            return Err(ProviderError::Unsupported);
        }
        let mut spent = 0;
        let current = self
            .fetch_metadata(permit.key(), context, &mut spent)
            .await?
            .version()?;
        if current.provider_revision != permit.expected().provider_revision
            || current.content_hash != permit.expected().content_hash
            || current.conditional_write_token != permit.expected().conditional_write_token
        {
            return Err(ProviderError::Conflict);
        }
        let expected = permit
            .expected()
            .conditional_write_token
            .as_ref()
            .ok_or(ProviderError::Unsupported)?;
        if expected != &permit.expected().provider_revision {
            return Err(ProviderError::Invalid);
        }
        let args = serde_json::json!({"path":permit.key().as_str(),"mode":{".tag":"update","update":expected.as_str()},"autorename":false,"strict_conflict":true,"mute":false});
        let mut headers = BTreeMap::new();
        headers.insert(
            "dropbox-api-arg".into(),
            serde_json::to_string(&args).map_err(|_| ProviderError::Invalid)?,
        );
        headers.insert("content-type".into(), "application/octet-stream".into());
        let response = self
            .send(
                Method::Post,
                "https://content.dropboxapi.com/2/files/upload".into(),
                headers,
                body.as_bytes().to_vec(),
                context,
                Some(permit.operation_id()),
                &mut spent,
            )
            .await?;
        if response.status() == 409 {
            let error: serde_json::Value =
                serde_json::from_slice(response.body()).map_err(|_| {
                    ProviderError::UnknownWrite {
                        operation_id: permit.operation_id(),
                    }
                })?;
            if error
                .get("error")
                .and_then(|v| v.get(".tag"))
                .and_then(|v| v.as_str())
                == Some("path")
                && error
                    .get("error")
                    .and_then(|v| v.get("path"))
                    .and_then(|v| v.get(".tag"))
                    .and_then(|v| v.as_str())
                    == Some("conflict")
            {
                return Err(ProviderError::Conflict);
            }
        }
        let unknown = || ProviderError::UnknownWrite {
            operation_id: permit.operation_id(),
        };
        if response.status() != 200 {
            return Err(unknown());
        }
        let file: File = serde_json::from_slice(response.body()).map_err(|_| unknown())?;
        file.validate().map_err(|_| unknown())?;
        if file.id != permit.key().as_str()
            || !self.allowed_path(&file.path_lower)
            || !markdown(&file.name)
            || file.size != body.as_bytes().len() as u64
        {
            return Err(unknown());
        }
        let version = file.version().map_err(|_| unknown())?;
        if let Some(hash) = &version.content_hash {
            verify_content_hash(body.as_bytes(), hash).map_err(|_| unknown())?;
        }
        Ok(WriteReceipt {
            operation_id: permit.operation_id(),
            key: permit.key().clone(),
            version,
            body_hash: permit.body_hash(),
            size_bytes: body.as_bytes().len() as u64,
        })
    }
}
