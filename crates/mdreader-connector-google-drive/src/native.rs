//! Reader-owned Google Drive API v3 adapter. Native selection is exact Picker
//! file IDs, or explicitly consented account/folder scope with guarded writes.
#[path = "native/create.rs"]
mod create;
#[path = "native/read.rs"]
mod read;
#[path = "native/recover_create.rs"]
mod recover_create;
#[path = "native/validator.rs"]
mod validator;
#[path = "native/write_diagnostic.rs"]
mod write_diagnostic;
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
#[path = "native/changes.rs"]
mod changes;
#[path = "native/folder.rs"]
mod folder;
#[path = "native/membership.rs"]
mod membership;
#[path = "native/tree.rs"]
mod tree;
pub const SELECTED_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
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
/// methods return only the exact admitted account/selection permission contract.
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
    resource_key: Option<OpaqueValue>,
}
impl SelectedFile {
    pub fn new(key: RecordKey, resource_key: Option<OpaqueValue>) -> Result<Self> {
        let safe = |v: &str| {
            !v.is_empty()
                && v.len() <= 512
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        };
        if key.as_str().len() > 256
            || !safe(key.as_str())
            || resource_key.as_ref().is_some_and(|k| {
                k.as_str().len() > 512
                    || !k
                        .as_str()
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && !matches!(b, b',' | b'/'))
            })
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self { key, resource_key })
    }
}
impl std::fmt::Debug for SelectedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SelectedFile([redacted])")
    }
}
pub struct Client {
    descriptor: SourceDescriptor,
    selected: Vec<SelectedFile>,
    known: std::sync::Mutex<BTreeMap<RecordKey, Option<Vec<RecordKey>>>>,
    folder: Option<folder::FolderScope>,
    resolver: Arc<dyn CredentialResolver>,
    authority: Arc<dyn ProviderAuthority>,
    transport: Arc<dyn Transport>,
    codec: CursorCodec,
    credentials: tokio::sync::Mutex<Option<AccessGrant>>,
}
impl Client {
    pub fn new(
        descriptor: SourceDescriptor,
        selected: Vec<SelectedFile>,
        resolver: Arc<dyn CredentialResolver>,
        authority: Arc<dyn ProviderAuthority>,
        transport: Arc<dyn Transport>,
        cursor_key: [u8; 32],
    ) -> Result<Self> {
        let scope = SelectedScope::exact(selected.iter().map(|f| f.key.clone()).collect())?;
        if descriptor.kind() != ProviderKind::GoogleDrive
            || descriptor.scope() != &scope
            || descriptor.capabilities().assets
            || !descriptor.capabilities().read
            || !descriptor.capabilities().list
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            descriptor,
            selected,
            known: std::sync::Mutex::new(BTreeMap::new()),
            folder: None,
            resolver,
            authority,
            transport,
            codec: CursorCodec::new(cursor_key),
            credentials: tokio::sync::Mutex::new(None),
        })
    }
    fn selected(&self, key: &RecordKey) -> Result<&SelectedFile> {
        self.selected
            .iter()
            .find(|f| f.key == *key)
            .ok_or(ProviderError::Unauthorized)
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
    fn headers(&self, key: &RecordKey) -> Result<BTreeMap<String, String>> {
        let resource = if self.folder.is_some() {
            folder::valid_id(key)?;
            None
        } else {
            self.selected(key)?.resource_key.as_ref()
        };
        let mut h = BTreeMap::new();
        if let Some(resource) = resource {
            h.insert(
                "x-goog-drive-resource-keys".into(),
                format!("{}/{}", key.as_str(), resource.as_str()),
            );
        }
        Ok(h)
    }
    async fn fetch_metadata(
        &self,
        key: &RecordKey,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<(File, DocumentVersion)> {
        let url = format!(
            "https://www.googleapis.com/drive/v3/files/{}?supportsAllDrives=true&fields=id,name,mimeType,version,md5Checksum,size,modifiedTime,trashed,parents",
            key.as_str()
        );
        let response = self
            .send(
                Method::Get,
                url,
                self.headers(key)?,
                vec![],
                context,
                None,
                spent,
            )
            .await?;
        if response.status() != 200 {
            return Err(membership::metadata_failure(&response));
        }
        let mut file: File =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        file.validate(key)?;
        file.membership_ancestors = self.member_path(&file, context, spent).await?;
        let mut version = file.version(response.headers().get("etag"))?;
        if version.conditional_write_token.is_none()
            && self.descriptor.capabilities().guarded_write
            && matches!(self.descriptor.scope(), SelectedScope::DriveFolder { .. })
        {
            version.conditional_write_token = self.v2_validator(key, &file, context, spent).await?;
        }
        Ok((file, version))
    }
    fn cursor(&self, kind: CursorKind, token: &str) -> Result<ProviderCursor> {
        self.codec.encode(
            ProviderKind::GoogleDrive,
            self.descriptor.binding(),
            kind,
            OpaqueValue::new(token)?,
        )
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    #[serde(skip)]
    membership_ancestors: Option<Vec<RecordKey>>,
    id: String,
    name: String,
    mime_type: String,
    #[serde(default)]
    version: String,
    md5_checksum: Option<String>,
    size: Option<String>,
    modified_time: Option<String>,
    #[serde(default)]
    trashed: bool,
    #[serde(default)]
    parents: Vec<String>,
}
impl File {
    fn validate(&self, key: &RecordKey) -> Result<()> {
        if self.id != key.as_str() {
            return Err(ProviderError::IdentityMismatch);
        }
        if self.trashed {
            return Err(ProviderError::NotFound);
        }
        if !markdown(&self.name) || !downloadable(&self.mime_type) {
            return Err(ProviderError::Unsupported);
        }
        DisplayText::new(self.name.clone())?;
        if self.version.is_empty()
            || self.version.len() > 20
            || !self.version.bytes().all(|b| b.is_ascii_digit())
            || self.version.parse::<u64>().is_err()
        {
            return Err(ProviderError::Protocol);
        }
        Ok(())
    }
    fn version(&self, etag: Option<&String>) -> Result<DocumentVersion> {
        let token = etag
            .filter(|tag| tag.starts_with('"') && tag.ends_with('"') && !tag.starts_with("W/"))
            .map(|s| OpaqueValue::new(s.clone()))
            .transpose()?;
        Ok(DocumentVersion {
            provider_revision: OpaqueValue::new(self.version.clone())?,
            content_hash: self
                .md5_checksum
                .as_deref()
                .map(|s| ContentHash::from_hex(HashAlgorithm::Md5, s))
                .transpose()?,
            conditional_write_token: token,
        })
    }
    fn metadata(self, version: DocumentVersion) -> Result<Metadata> {
        let size_bytes = self
            .size
            .map(|s| s.parse::<u64>().map_err(|_| ProviderError::Protocol))
            .transpose()?;
        let modified_at_ms = self
            .modified_time
            .map(|s| {
                time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
                    .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
                    .map_err(|_| ProviderError::Protocol)
            })
            .transpose()?;
        Ok(Metadata {
            membership_ancestors: self.membership_ancestors,
            key: RecordKey::new(self.id)?,
            name: DisplayText::new(self.name.clone())?,
            display_path: DisplayText::new(self.name)?,
            media_type: Some(DisplayText::new(self.mime_type)?),
            modified_at_ms,
            size_bytes,
            version,
        })
    }
}
// Drive stores the uploader's MIME label; it is not a Markdown suffix index.
// Google-native documents and shortcuts require other APIs and are not bodies.
fn downloadable(mime: &str) -> bool {
    !mime.is_empty()
        && mime.len() <= 256
        && !mime.chars().any(char::is_control)
        && !mime.starts_with("application/vnd.google-apps.")
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
#[async_trait]
impl DocumentProvider for Client {
    fn descriptor(&self) -> &SourceDescriptor {
        &self.descriptor
    }
    fn retain_metadata_observation(&self, rows: &[Metadata], replaces: bool) -> Result<()> {
        self.remember_metadata(rows, replaces)
    }
    fn retain_metadata_changes(&self, rows: &[Metadata], deleted: &[RecordKey]) -> Result<()> {
        {
            let mut known = self.known.lock().map_err(|_| ProviderError::Persistence)?;
            for key in deleted {
                known.remove(key);
            }
        }
        self.remember_metadata(rows, false)
    }
    async fn metadata(&self, key: &RecordKey, context: &IoContext) -> Result<Metadata> {
        let mut spent = 0;
        let (file, version) = self.fetch_metadata(key, context, &mut spent).await?;
        let mut metadata = file.metadata(version)?;
        if let Some(folder) = &self.folder {
            metadata.display_path =
                DisplayText::new(format!("/{}/{}", folder.root.as_str(), key.as_str()))?;
        }
        Ok(metadata)
    }

    async fn list(
        &self,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<MetadataPage> {
        if self.folder.is_some() {
            return self.list_folder(cursor, context).await;
        }
        let offset = cursor
            .map(|cursor| {
                self.codec
                    .decode(
                        ProviderKind::GoogleDrive,
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
        if offset >= self.selected.len() {
            return Err(ProviderError::Invalid);
        }
        let end = offset
            .saturating_add(context.page_rows())
            .min(self.selected.len());
        let mut spent = 0;
        let mut rows = Vec::new();
        for selected in &self.selected[offset..end] {
            let (file, version) = self
                .fetch_metadata(&selected.key, context, &mut spent)
                .await?;
            rows.push(file.metadata(version)?);
        }
        let observed = now()?;
        let next = if end < self.selected.len() {
            Some(self.cursor(CursorKind::List, &end.to_string())?)
        } else {
            None
        };
        let coverage = if next.is_some() {
            Coverage::Partial {
                reason: DisplayText::new("Selected-file traversal continues")?,
            }
        } else {
            Coverage::MetadataCompleteSelected {
                observed_at_ms: observed,
            }
        };
        MetadataPage::new(rows, next, observed, coverage)
    }
    async fn folders(
        &self,
        folder: &RecordKey,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<FolderPage> {
        self.browse_folders(folder, cursor, context).await
    }
    async fn read(&self, key: &RecordKey, context: &IoContext) -> Result<DocumentBody> {
        self.read_document(key, context, &mut 0).await
    }
    async fn start_changes(&self, context: &IoContext) -> Result<ProviderCursor> {
        if !self.descriptor.capabilities().changes {
            return Err(ProviderError::Unsupported);
        }
        let mut spent = 0;
        let response = self
            .send(
                Method::Get,
                "https://www.googleapis.com/drive/v3/changes/startPageToken?supportsAllDrives=true"
                    .into(),
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
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Start {
            start_page_token: String,
        }
        let start: Start =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        self.cursor(CursorKind::Changes, &start.start_page_token)
    }
    async fn changes(&self, cursor: &ProviderCursor, context: &IoContext) -> Result<ChangeOutcome> {
        self.changed_metadata(cursor, context).await
    }
    async fn recover_create(
        &self,
        operation_id: uuid::Uuid,
        filename: &str,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<creation::RecoveryObservation> {
        self.observe_created(operation_id, filename, body, context)
            .await
    }
    async fn guarded_create(
        &self,
        permit: creation::CreatePermit,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<Metadata> {
        self.create_document(permit, body, context).await
    }
    async fn guarded_write(
        &self,
        permit: journal::DispatchPermit,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<WriteReceipt> {
        permit.validate(self.descriptor.binding(), body)?;
        self.headers(permit.key())?;
        if !self.descriptor.capabilities().guarded_write {
            return Err(ProviderError::Unsupported);
        }
        let expected = permit
            .expected()
            .conditional_write_token
            .as_ref()
            .ok_or(ProviderError::Unsupported)?;
        let v2 = validator::is_v2(expected);
        let mut spent = 0;
        let (_, current) = self
            .fetch_metadata(permit.key(), context, &mut spent)
            .await
            .inspect_err(|_| {
                if v2 {
                    validator::failure(
                        permit.operation_id(),
                        validator::WritePhase::PreflightRead,
                        None,
                    );
                }
            })?;
        if current.provider_revision != permit.expected().provider_revision
            || current.content_hash != permit.expected().content_hash
        {
            if v2 {
                validator::failure(
                    permit.operation_id(),
                    validator::WritePhase::PreflightRevision,
                    None,
                );
            }
            return Err(ProviderError::Conflict);
        }
        if current.conditional_write_token.as_ref() != Some(expected) {
            if v2 {
                validator::failure(
                    permit.operation_id(),
                    validator::WritePhase::PreflightValidator,
                    None,
                );
            }
            return Err(ProviderError::Conflict);
        }
        let unknown = || ProviderError::UnknownWrite {
            operation_id: permit.operation_id(),
        };
        let (file, version) = if v2 {
            self.v2_write(&permit, body, context, &mut spent).await?
        } else {
            let mut headers = self.headers(permit.key())?;
            headers.insert("if-match".into(), expected.as_str().into());
            headers.insert("content-type".into(), "text/markdown; charset=utf-8".into());
            let url = format!(
                "https://www.googleapis.com/upload/drive/v3/files/{}?uploadType=media&supportsAllDrives=true&fields=id,name,mimeType,version,md5Checksum,size,trashed",
                permit.key().as_str()
            );
            let response = self
                .send(
                    Method::Patch,
                    url,
                    headers,
                    body.as_bytes().to_vec(),
                    context,
                    Some(permit.operation_id()),
                    &mut spent,
                )
                .await?;
            if response.status() == 412 {
                return Err(ProviderError::Conflict);
            }
            if response.status() != 200 {
                return Err(unknown());
            }
            let file: File = serde_json::from_slice(response.body()).map_err(|_| unknown())?;
            file.validate(permit.key()).map_err(|_| unknown())?;
            let version = file
                .version(response.headers().get("etag"))
                .map_err(|_| unknown())?;
            (file, version)
        };
        if version.provider_revision == permit.expected().provider_revision
            || file.size.as_deref().and_then(|s| s.parse::<u64>().ok())
                != Some(body.as_bytes().len() as u64)
        {
            if v2 {
                validator::failure(
                    permit.operation_id(),
                    validator::WritePhase::ResponseRevisionSize,
                    None,
                );
            }
            return Err(unknown());
        }
        if let Some(hash) = &version.content_hash {
            verify_content_hash(body.as_bytes(), hash).map_err(|_| unknown())?;
        }
        let version = if matches!(self.descriptor.scope(), SelectedScope::DriveFolder { .. }) {
            // A file ETag cannot condition ancestor movement. Confirm the exact
            // accepted revision and fresh lineage before publishing a receipt;
            // any unconfirmed post-dispatch read remains an unknown outcome.
            let (confirmed, observed) = self
                .fetch_metadata(permit.key(), context, &mut spent)
                .await
                .map_err(|_| {
                    if v2 {
                        validator::failure(
                            permit.operation_id(),
                            validator::WritePhase::ConfirmationRead,
                            None,
                        );
                    }
                    unknown()
                })?;
            if observed.provider_revision != version.provider_revision
                || observed.content_hash != version.content_hash
                || confirmed
                    .size
                    .as_deref()
                    .and_then(|size| size.parse::<u64>().ok())
                    != Some(body.as_bytes().len() as u64)
            {
                if v2 {
                    validator::failure(
                        permit.operation_id(),
                        validator::WritePhase::ConfirmationMismatch,
                        None,
                    );
                }
                return Err(unknown());
            }
            observed
        } else {
            version
        };
        Ok(WriteReceipt {
            operation_id: permit.operation_id(),
            key: permit.key().clone(),
            version,
            body_hash: permit.body_hash(),
            size_bytes: body.as_bytes().len() as u64,
        })
    }
}
