//! Explicit multipart create under the saved folder, never a file-ID update.
use super::*;
impl Client {
    pub(super) async fn create_document(
        &self,
        permit: creation::CreatePermit,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<Metadata> {
        permit.validate(self.descriptor.binding(), body)?;
        if !self.descriptor.capabilities().guarded_write
            || !matches!(self.descriptor.scope(), SelectedScope::DriveFolder { .. })
        {
            return Err(ProviderError::Unsupported);
        }
        if body.as_bytes().len() > MAX_BODY_BYTES.saturating_sub(16384) {
            return Err(ProviderError::TooLarge);
        }
        let mut spent = 0;
        let root = self.root_id(context, &mut spent).await?;
        let boundary = format!("mdreader_{}", permit.operation_id().simple());
        if body
            .as_bytes()
            .windows(boundary.len())
            .any(|v| v == boundary.as_bytes())
        {
            return Err(ProviderError::Invalid);
        }
        let json=serde_json::to_string(&serde_json::json!({"name":permit.filename(),"mimeType":"text/markdown","parents":[root.as_str()],"appProperties":{"mdreaderCreateOperation":permit.operation_id().to_string()}})).map_err(|_|ProviderError::Invalid)?;
        let mut multipart=format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{json}\r\n--{boundary}\r\nContent-Type: text/markdown; charset=UTF-8\r\n\r\n").into_bytes();
        multipart.extend_from_slice(body.as_bytes());
        multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut headers = BTreeMap::new();
        headers.insert(
            "content-type".into(),
            format!("multipart/related; boundary={boundary}"),
        );
        let unknown = || ProviderError::UnknownWrite {
            operation_id: permit.operation_id(),
        };
        let response=self.send(Method::Post,"https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&supportsAllDrives=true&fields=id,name,mimeType,version,md5Checksum,size,modifiedTime,trashed,parents".into(),headers,multipart,context,Some(permit.operation_id()),&mut spent).await?;
        if response.status() != 200 && response.status() != 201 {
            return Err(unknown());
        }
        let mut file: File = serde_json::from_slice(response.body()).map_err(|_| unknown())?;
        let key = RecordKey::new(file.id.clone()).map_err(|_| unknown())?;
        file.validate(&key).map_err(|_| unknown())?;
        if file.name != permit.filename()
            || file.size.as_deref().and_then(|v| v.parse::<u64>().ok())
                != Some(body.as_bytes().len() as u64)
            || file.parents != vec![root.as_str().to_owned()]
        {
            return Err(unknown());
        }
        file.membership_ancestors = Some(vec![root]);
        let accepted = file
            .version(response.headers().get("etag"))
            .map_err(|_| unknown())?;
        if let Some(hash) = &accepted.content_hash {
            verify_content_hash(body.as_bytes(), hash).map_err(|_| unknown())?;
        }
        // Fresh native membership/revision and strong validator are required for
        // the immediately editable exact receipt; failure after POST is unknown.
        self.known
            .lock()
            .map_err(|_| unknown())?
            .insert(key.clone(), file.membership_ancestors.clone());
        let (confirmed, version) = self
            .fetch_metadata(&key, context, &mut spent)
            .await
            .map_err(|_| unknown())?;
        if version.provider_revision != accepted.provider_revision
            || version.content_hash != accepted.content_hash
            || confirmed.name != permit.filename()
            || confirmed.size != file.size
            || version.conditional_write_token.is_none()
        {
            return Err(unknown());
        }
        let mut metadata = confirmed.metadata(version).map_err(|_| unknown())?;
        let selected = self.folder.as_ref().ok_or_else(unknown)?;
        metadata.display_path =
            DisplayText::new(format!("/{}/{}", selected.root.as_str(), key.as_str()))
                .map_err(|_| unknown())?;
        Ok(metadata)
    }
}
