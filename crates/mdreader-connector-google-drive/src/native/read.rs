//! Exact bounded native body read with shared operation accounting.
use super::*;
impl Client {
    pub(super) async fn read_document(
        &self,
        key: &RecordKey,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<DocumentBody> {
        let (file, version) = self.fetch_metadata(key, context, spent).await?;
        if file
            .size
            .as_deref()
            .is_some_and(|s| s.parse::<u64>().map_or(true, |n| n > MAX_BODY_BYTES as u64))
        {
            return Err(ProviderError::TooLarge);
        }
        let mut headers = self.headers(key)?;
        if let Some(token) = &version.conditional_write_token {
            if !validator::is_v2(token) {
                headers.insert("if-match".into(), token.as_str().into());
            }
        }
        let response = self
            .send(
                Method::Get,
                format!(
                    "https://www.googleapis.com/drive/v3/files/{}?alt=media&supportsAllDrives=true",
                    key.as_str()
                ),
                headers,
                vec![],
                context,
                None,
                spent,
            )
            .await?;
        if response.status() == 412 {
            return Err(ProviderError::Conflict);
        }
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let body = MarkdownBytes::new(response.into_body())?;
        if let Some(hash) = &version.content_hash {
            verify_content_hash(body.as_bytes(), hash)?;
        }
        // Confirm authority and revision after body transfer, including servers
        // that omitted a conditional HTTP validator. Never invent an ETag.
        let (_, after) = self.fetch_metadata(key, context, spent).await?;
        if after.provider_revision != version.provider_revision
            || after.content_hash != version.content_hash
        {
            return Err(ProviderError::Conflict);
        }
        Ok(DocumentBody {
            key: key.clone(),
            version: after,
            body,
            observed_at_ms: now()?,
        })
    }
}
