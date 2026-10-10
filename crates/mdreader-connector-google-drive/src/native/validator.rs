//! Drive v2 exposes the file ETag omitted by v3. Its validator belongs only to
//! v2 conditional updates; v3 remains the source of selected membership/index
//! revisions. Never infer a validator from a revision/hash or use If-Match: *.
//! The v2 File resource documents its output-only ETag. Chromium's first-party
//! Drive adapter uses that ETag for PUT multipart updates on this same v2 API:
//! https://chromium.googlesource.com/chromium/src/+/138.0.7204.157/google_apis/drive/drive_api_requests.cc
//! https://developers.google.com/workspace/drive/api/reference/rest/v2/files/update
use super::*;
const PREFIX: &str = "drive-v2-file-etag:";
const FIELDS: &str =
    "id,title,mimeType,version,md5Checksum,fileSize,modifiedDate,labels(trashed),parents(id),etag";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V2File {
    id: String,
    title: String,
    mime_type: String,
    version: String,
    md5_checksum: Option<String>,
    file_size: Option<String>,
    modified_date: Option<String>,
    labels: Labels,
    parents: Vec<Parent>,
    etag: Option<String>,
}
#[derive(Deserialize)]
struct Labels {
    trashed: bool,
}
#[derive(Deserialize)]
struct Parent {
    id: String,
}
fn strong(value: &str) -> bool {
    value.len() >= 3
        && value.len() <= 4096
        && value.starts_with('"')
        && value.ends_with('"')
        && value[1..value.len() - 1]
            .bytes()
            .all(|v| v >= 0x21 && v != b'"' && v != 0x7f)
}
pub(super) fn is_v2(token: &OpaqueValue) -> bool {
    token.as_str().starts_with(PREFIX)
}
pub(super) fn raw(token: &OpaqueValue) -> Result<&str> {
    let value = token
        .as_str()
        .strip_prefix(PREFIX)
        .ok_or(ProviderError::Invalid)?;
    if !strong(value) {
        return Err(ProviderError::Invalid);
    }
    Ok(value)
}
fn decode(response: &Response, key: &RecordKey) -> Result<(File, DocumentVersion)> {
    let v: V2File = serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
    let file = File {
        membership_ancestors: None,
        id: v.id,
        name: v.title,
        mime_type: v.mime_type,
        version: v.version,
        md5_checksum: v.md5_checksum,
        size: v.file_size,
        modified_time: v.modified_date,
        trashed: v.labels.trashed,
        parents: v.parents.into_iter().map(|p| p.id).collect(),
    };
    file.validate(key)?;
    membership::parents(&file.parents)?;
    let mut version = file.version(None)?;
    if let Some(etag) = v.etag.filter(|v| strong(v)) {
        if response
            .headers()
            .get("etag")
            .is_some_and(|header| header != &etag)
        {
            return Err(ProviderError::Conflict);
        }
        version.conditional_write_token = Some(OpaqueValue::new(format!("{PREFIX}{etag}"))?);
    }
    Ok((file, version))
}
impl Client {
    pub(super) async fn v2_validator(
        &self,
        key: &RecordKey,
        file: &File,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<Option<OpaqueValue>> {
        let response=self.send(Method::Get,format!("https://www.googleapis.com/drive/v2/files/{}?supportsAllDrives=true&fields={FIELDS}",key.as_str()),self.headers(key)?,vec![],context,None,spent).await?;
        // v2 may not be available for a retained read. Refusal never grants
        // editing and never turns the underlying known document into a deletion.
        if matches!(response.status(), 400 | 403 | 404) {
            return Ok(None);
        }
        if response.status() != 200 {
            return Err(membership::metadata_failure(&response));
        }
        let (observed, version) = decode(&response, key)?;
        let original = file.version(None)?;
        if observed.name != file.name
            || observed.mime_type != file.mime_type
            || observed.size != file.size
            || membership::parents(&observed.parents)? != membership::parents(&file.parents)?
            || version.provider_revision != original.provider_revision
            || version.content_hash != original.content_hash
        {
            return Err(ProviderError::Conflict);
        }
        // Blob checksum is required to bind this separate representation to the
        // exact v3 content observation; no hash-less validator refinement.
        if original.content_hash.is_none() {
            return Ok(None);
        }
        Ok(version.conditional_write_token)
    }
    pub(super) async fn v2_write(
        &self,
        permit: &journal::DispatchPermit,
        body: &MarkdownBytes,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<(File, DocumentVersion)> {
        let token = permit
            .expected()
            .conditional_write_token
            .as_ref()
            .ok_or(ProviderError::Unsupported)?;
        let etag = raw(token)?;
        // First-party Chromium Drive v2 multipart updates likewise issue PUT
        // with the file ETag. Empty metadata preserves names and parent folders.
        let boundary = format!("mdreader-{}", permit.operation_id());
        // The operation identity is already visible to the editing client.
        // Reject a delimiter collision before any PUT, preserving literal text.
        if body
            .as_bytes()
            .windows(boundary.len())
            .any(|v| v == boundary.as_bytes())
        {
            return Err(ProviderError::Invalid);
        }
        let prefix=format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{{}}\r\n--{boundary}\r\nContent-Type: text/markdown; charset=UTF-8\r\n\r\n");
        let suffix = format!("\r\n--{boundary}--\r\n");
        let size = prefix
            .len()
            .checked_add(body.as_bytes().len())
            .and_then(|v| v.checked_add(suffix.len()))
            .filter(|v| *v <= MAX_BODY_BYTES)
            .ok_or(ProviderError::TooLarge)?;
        let mut payload = Vec::with_capacity(size);
        payload.extend_from_slice(prefix.as_bytes());
        payload.extend_from_slice(body.as_bytes());
        payload.extend_from_slice(suffix.as_bytes());
        let mut headers = self.headers(permit.key())?;
        headers.insert("if-match".into(), etag.into());
        headers.insert(
            "content-type".into(),
            format!("multipart/related; boundary={boundary}"),
        );
        let response=self.send(Method::Put,format!("https://www.googleapis.com/upload/drive/v2/files/{}?uploadType=multipart&supportsAllDrives=true&fields={FIELDS}",permit.key().as_str()),headers,payload,context,Some(permit.operation_id()),spent).await?;
        if response.status() == 412 {
            return Err(ProviderError::Conflict);
        }
        let unknown = || ProviderError::UnknownWrite {
            operation_id: permit.operation_id(),
        };
        if response.status() != 200 {
            return Err(unknown());
        }
        let (file, version) = decode(&response, permit.key()).map_err(|_| unknown())?;
        // This multipart path must prove the accepted literal bytes, not infer
        // their content from size or a revision change after remote mutation.
        let hash = version.content_hash.as_ref().ok_or_else(unknown)?;
        verify_content_hash(body.as_bytes(), hash).map_err(|_| unknown())?;
        Ok((file, version))
    }
}
