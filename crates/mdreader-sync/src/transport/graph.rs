use super::*;

/// Bounded untrusted ciphertext; not a prepared owner publication or plaintext.
pub struct DownloadedResource {
    pub blob: String,
    pub envelope_utf8: Vec<u8>,
}
impl Transport {
    /// Exact authenticated body requests, with the same scope/generation barrier
    /// and rejection hook as foreground transport. No owner lock spans HTTP.
    pub async fn download_resources<O: Owner, A: Authority>(
        &self,
        owner: &O,
        authority: &A,
        blobs: &[String],
        limit: usize,
    ) -> TransportResult<Vec<DownloadedResource>> {
        if blobs.len() > crate::codec::MAX_RESOURCES || limit == 0 || limit > 32 * 1024 * 1024 {
            return Err(TransportError::Quota);
        }
        let mut unique = HashSet::new();
        for blob in blobs {
            crypto::validate_digest(blob)?;
            if !unique.insert(blob) {
                return Err(TransportError::Protocol);
            }
        }
        let scope = authority.scope()?;
        let generation = authority.generation()?;
        self.check(authority, &scope, generation)?;
        owner.with_store(&scope, |_| Ok(()))?;
        {
            let mut running = self.running.lock().map_err(|_| TransportError::Busy)?;
            if !running.insert(scope.clone()) {
                return Err(TransportError::Busy);
            }
        }
        let _guard = RunGuard {
            running: self.running,
            scope: scope.clone(),
        };
        tokio::time::timeout(Duration::from_secs(60), async {
            let mut session = Session {
                authority,
                scope,
                generation,
                remaining: limit,
            };
            let mut result = Vec::with_capacity(blobs.len());
            for blob in blobs {
                let (_, body) = self
                    .request(
                        owner,
                        &mut session,
                        Method::GET,
                        self.url(&format!("/v1/blobs/{blob}"), &[])?,
                        Vec::new(),
                        512 * 1024,
                    )
                    .await?;
                if crate::blob_digest(&body) != *blob {
                    return Err(TransportError::Protocol);
                }
                result.push(DownloadedResource {
                    blob: blob.clone(),
                    envelope_utf8: body,
                });
            }
            Ok(result)
        })
        .await
        .map_err(|_| TransportError::Retryable)?
    }
}
