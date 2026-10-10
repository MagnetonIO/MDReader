//! Explicit create-only upload under the existing saved folder grant.
use super::*;
pub(super) fn header_json(value: &serde_json::Value) -> Result<String> {
    let json = serde_json::to_string(value).map_err(|_| ProviderError::Invalid)?;
    let mut encoded = String::with_capacity(json.len());
    for ch in json.chars() {
        if ch >= '\u{007f}' {
            let mut units = [0; 2];
            for unit in ch.encode_utf16(&mut units) {
                encoded.push_str(&format!("\\u{unit:04x}"));
            }
        } else {
            encoded.push(ch);
        }
    }
    Ok(encoded)
}
impl Client {
    pub(super) async fn create_document(
        &self,
        permit: creation::CreatePermit,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<Metadata> {
        permit.validate(self.descriptor.binding(), body)?;
        if !self.descriptor.capabilities().guarded_write {
            return Err(ProviderError::Unsupported);
        }
        let Selection::AppFolderRoot { path, .. } = &self.selection else {
            return Err(ProviderError::Unsupported);
        };
        let destination = format!(
            "{}/{}",
            path.as_str().trim_end_matches('/'),
            permit.filename()
        );
        if !valid_path(&destination) || !self.allowed_path(&destination) {
            return Err(ProviderError::Unauthorized);
        }
        let mut headers = BTreeMap::new();
        headers.insert("dropbox-api-arg".into(),header_json(&serde_json::json!({"path":destination,"mode":"add","autorename":false,"strict_conflict":true,"mute":false})).map_err(|_|ProviderError::Invalid)?);
        headers.insert("content-type".into(), "application/octet-stream".into());
        let unknown = || ProviderError::UnknownWrite {
            operation_id: permit.operation_id(),
        };
        let response = self
            .send(
                Method::Post,
                "https://content.dropboxapi.com/2/files/upload".into(),
                headers,
                body.as_bytes().to_vec(),
                context,
                Some(permit.operation_id()),
                &mut 0,
            )
            .await?;
        if response.status() == 409 {
            let error: serde_json::Value =
                serde_json::from_slice(response.body()).map_err(|_| unknown())?;
            if error.pointer("/error/.tag").and_then(|v| v.as_str()) == Some("path")
                && error.pointer("/error/path/.tag").and_then(|v| v.as_str()) == Some("conflict")
            {
                return Err(ProviderError::Conflict);
            }
        }
        if response.status() != 200 {
            return Err(unknown());
        }
        let file: File = serde_json::from_slice(response.body()).map_err(|_| unknown())?;
        file.validate().map_err(|_| unknown())?;
        if file.path_lower != destination.to_lowercase()
            || file.name != permit.filename()
            || file.size != body.as_bytes().len() as u64
        {
            return Err(unknown());
        }
        let version = file.version().map_err(|_| unknown())?;
        if let Some(hash) = &version.content_hash {
            verify_content_hash(body.as_bytes(), hash).map_err(|_| unknown())?;
        }
        self.observe(std::slice::from_ref(&file))
            .map_err(|_| unknown())?;
        file.metadata().map_err(|_| unknown())
    }
}
