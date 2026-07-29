//! Authorized point reads over published cloud Markdown metadata.

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use catdb_connectors::{CancellationToken, SourceId};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::{
    store::{CloudDocumentStore, CloudDocumentStoreError, CloudDocumentVersionPins},
    CloudCacheState, CloudDocumentError, CloudIoContext, CloudMarkdownDocument,
    MarkdownDocumentRevision, MarkdownDocumentSource, ProviderRevision, SourceRecordKey,
};

const READ_DEADLINE: Duration = Duration::from_secs(30);
const EDIT_AUTHORITY_LIFETIME_MS: i64 = 30 * 60 * 1_000;
const EDIT_AUTHORITY_VERSION: u8 = 1;
const MAC_BYTES: usize = 32;

type HmacSha256 = Hmac<Sha256>;

/// Revalidates the exact Library membership admitted for a point read.
#[async_trait::async_trait]
pub(crate) trait CloudDocumentReadAuthority: Send + Sync {
    async fn revalidate(&self) -> Result<(), CloudDocumentError>;
}

/// Authenticated, short-lived renderer authority for one immutable body anchor.
pub(crate) struct CloudEditAuthorityCodec {
    key: [u8; 32],
}

impl CloudEditAuthorityCodec {
    pub(crate) fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn issue(
        &self,
        issued_at_ms: i64,
        library_id: &[u8; 16],
        source_id: &SourceId,
        source_version_id: &[u8],
        record_key: &SourceRecordKey,
        revision: &ProviderRevision,
        body: &[u8],
    ) -> Result<String, CloudDocumentError> {
        let expires_at_ms = issued_at_ms
            .checked_add(EDIT_AUTHORITY_LIFETIME_MS)
            .ok_or(CloudDocumentError::ProtocolFailed)?;
        let mut payload = Vec::new();
        payload.push(EDIT_AUTHORITY_VERSION);
        payload.extend_from_slice(&issued_at_ms.to_be_bytes());
        payload.extend_from_slice(&expires_at_ms.to_be_bytes());
        payload.extend_from_slice(library_id);
        append_bounded(&mut payload, source_id.as_str().as_bytes())?;
        append_bounded(&mut payload, source_version_id)?;
        payload.extend_from_slice(blake3::hash(record_key.as_str().as_bytes()).as_bytes());
        append_bounded(&mut payload, revision.opaque.as_bytes())?;
        payload.extend_from_slice(blake3::hash(body).as_bytes());
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        mac.update(&payload);
        payload.extend_from_slice(&mac.finalize().into_bytes());
        Ok(URL_SAFE_NO_PAD.encode(payload))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn verify(
        &self,
        token: &str,
        now_ms: i64,
        library_id: &[u8; 16],
        source_id: &SourceId,
        source_version_id: &[u8],
        record_key: &SourceRecordKey,
        revision: &ProviderRevision,
        body: &[u8],
    ) -> Result<(), CloudDocumentError> {
        let decoded = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| CloudDocumentError::Unauthorized)?;
        let payload_bytes = decoded
            .len()
            .checked_sub(MAC_BYTES)
            .ok_or(CloudDocumentError::Unauthorized)?;
        let (payload, supplied_mac) = decoded.split_at(payload_bytes);
        let mut mac =
            HmacSha256::new_from_slice(&self.key).map_err(|_| CloudDocumentError::Unauthorized)?;
        mac.update(payload);
        let expected_mac = mac.finalize().into_bytes();
        if !bool::from(expected_mac.as_slice().ct_eq(supplied_mac)) {
            return Err(CloudDocumentError::Unauthorized);
        }

        let mut decoder = TokenDecoder::new(payload);
        if decoder.byte()? != EDIT_AUTHORITY_VERSION {
            return Err(CloudDocumentError::Unauthorized);
        }
        let issued_at_ms = decoder.i64()?;
        let expires_at_ms = decoder.i64()?;
        if issued_at_ms > now_ms
            || expires_at_ms != issued_at_ms.saturating_add(EDIT_AUTHORITY_LIFETIME_MS)
            || now_ms > expires_at_ms
            || decoder.fixed(16)? != library_id
            || decoder.bounded()? != source_id.as_str().as_bytes()
            || decoder.bounded()? != source_version_id
            || decoder.fixed(32)? != blake3::hash(record_key.as_str().as_bytes()).as_bytes()
            || decoder.bounded()? != revision.opaque.as_bytes()
            || decoder.fixed(32)? != blake3::hash(body).as_bytes()
            || !decoder.is_empty()
        {
            return Err(CloudDocumentError::Unauthorized);
        }
        Ok(())
    }
}

/// Reads one already-published record through its admitted provider adapter.
pub(crate) struct CloudDocumentService {
    store: Arc<CloudDocumentStore>,
    source: Arc<dyn MarkdownDocumentSource>,
    pins: CloudDocumentVersionPins,
    library_id: [u8; 16],
    cursor_key: [u8; 32],
    authority: Arc<dyn CloudDocumentReadAuthority>,
}

impl CloudDocumentService {
    pub(crate) fn new(
        store: Arc<CloudDocumentStore>,
        source: Arc<dyn MarkdownDocumentSource>,
        pins: CloudDocumentVersionPins,
        library_id: [u8; 16],
        cursor_key: [u8; 32],
        authority: Arc<dyn CloudDocumentReadAuthority>,
    ) -> Self {
        Self {
            store,
            source,
            pins,
            library_id,
            cursor_key,
            authority,
        }
    }

    pub(crate) async fn read_cloud_markdown(
        &self,
        source_id: &SourceId,
        record_key: &str,
    ) -> Result<CloudMarkdownDocument, CloudDocumentError> {
        if self.source.source_id() != source_id {
            return Err(CloudDocumentError::Unauthorized);
        }
        self.authority.revalidate().await?;
        let record_key = SourceRecordKey::new(record_key);
        let published = {
            let store = Arc::clone(&self.store);
            let source_id = source_id.clone();
            let record_key = record_key.clone();
            tauri::async_runtime::spawn_blocking(move || {
                store.published_for_keys(&source_id, &[record_key])
            })
            .await
            .map_err(|_| CloudDocumentError::ProtocolFailed)?
            .map_err(map_store_error)?
            .into_iter()
            .next()
            .ok_or(CloudDocumentError::NotFound)?
        };
        if published.deleted || !is_markdown_name(&published.name) {
            return Err(CloudDocumentError::NotFound);
        }
        if published
            .size_bytes
            .is_some_and(|size| size > CloudDocumentStore::MAX_BODY_BYTES as u64)
        {
            return Err(CloudDocumentError::TooLarge);
        }

        let accessed_at_ms = now_ms()?;
        let cached = {
            let store = Arc::clone(&self.store);
            let source_id = source_id.clone();
            let pins = self.pins.clone();
            let record_key = record_key.clone();
            let revision = published.revision.clone();
            tauri::async_runtime::spawn_blocking(move || {
                store.get_body(&source_id, &pins, &record_key, &revision, accessed_at_ms)
            })
            .await
            .map_err(|_| CloudDocumentError::ProtocolFailed)?
            .map_err(map_store_error)?
        };
        if let Some(body) = cached {
            self.authority.revalidate().await?;
            return self.document_from_body(&published, body, CloudCacheState::Cached);
        }

        let context = CloudIoContext {
            deadline: Instant::now() + READ_DEADLINE,
            page_rows: 1,
            byte_limit: CloudDocumentStore::MAX_BODY_BYTES,
            cancellation: CancellationToken::new(),
        };
        let document = self.source.read_document(&record_key, &context).await?;
        if document.key != record_key || document.revision != published.revision {
            return Err(CloudDocumentError::IdentityMismatch);
        }
        if document.body.len() > CloudDocumentStore::MAX_BODY_BYTES {
            return Err(CloudDocumentError::TooLarge);
        }
        std::str::from_utf8(&document.body).map_err(|_| CloudDocumentError::InvalidUtf8)?;
        self.authority.revalidate().await?;

        {
            let store = Arc::clone(&self.store);
            let source_id = source_id.clone();
            let pins = self.pins.clone();
            let document = MarkdownDocumentRevision {
                key: document.key.clone(),
                revision: document.revision.clone(),
                body: document.body.clone(),
                observed_at_ms: document.observed_at_ms,
            };
            tauri::async_runtime::spawn_blocking(move || {
                store.put_body(&source_id, &pins, &document)
            })
            .await
            .map_err(|_| CloudDocumentError::ProtocolFailed)?
            .map_err(map_store_error)?;
        }
        self.authority.revalidate().await?;
        self.document_from_body(&published, document.body, CloudCacheState::ProviderFresh)
    }

    fn document_from_body(
        &self,
        published: &super::store::PublishedCloudDocument,
        body: Vec<u8>,
        cache_state: CloudCacheState,
    ) -> Result<CloudMarkdownDocument, CloudDocumentError> {
        let content = String::from_utf8(body).map_err(|_| CloudDocumentError::InvalidUtf8)?;
        let mut document_key = blake3::Hasher::new();
        document_key.update(b"mdreader-cloud-document-key-v1\0");
        document_key.update(&self.library_id);
        document_key.update(self.source.source_id().as_str().as_bytes());
        document_key.update(&[0]);
        document_key.update(published.key.as_str().as_bytes());
        let document_key = document_key.finalize().to_hex().to_string();
        let edit_token = CloudEditAuthorityCodec::new(self.cursor_key).issue(
            now_ms()?,
            &self.library_id,
            self.source.source_id(),
            self.pins.source_version_id(),
            &published.key,
            &published.revision,
            content.as_bytes(),
        )?;
        Ok(CloudMarkdownDocument {
            document_key,
            title: published.name.clone(),
            content,
            edit_token: Some(edit_token),
            cache_state,
            modified_at_ms: published.modified_at_ms,
        })
    }
}

fn append_bounded(payload: &mut Vec<u8>, value: &[u8]) -> Result<(), CloudDocumentError> {
    let length = u16::try_from(value.len()).map_err(|_| CloudDocumentError::ProtocolFailed)?;
    payload.extend_from_slice(&length.to_be_bytes());
    payload.extend_from_slice(value);
    Ok(())
}

struct TokenDecoder<'a> {
    remaining: &'a [u8],
}

impl<'a> TokenDecoder<'a> {
    fn new(payload: &'a [u8]) -> Self {
        Self { remaining: payload }
    }

    fn byte(&mut self) -> Result<u8, CloudDocumentError> {
        Ok(self.fixed(1)?[0])
    }

    fn i64(&mut self) -> Result<i64, CloudDocumentError> {
        let bytes: [u8; 8] = self
            .fixed(8)?
            .try_into()
            .map_err(|_| CloudDocumentError::Unauthorized)?;
        Ok(i64::from_be_bytes(bytes))
    }

    fn bounded(&mut self) -> Result<&'a [u8], CloudDocumentError> {
        let length: [u8; 2] = self
            .fixed(2)?
            .try_into()
            .map_err(|_| CloudDocumentError::Unauthorized)?;
        self.fixed(usize::from(u16::from_be_bytes(length)))
    }

    fn fixed(&mut self, length: usize) -> Result<&'a [u8], CloudDocumentError> {
        if self.remaining.len() < length {
            return Err(CloudDocumentError::Unauthorized);
        }
        let (value, remaining) = self.remaining.split_at(length);
        self.remaining = remaining;
        Ok(value)
    }

    fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

fn is_markdown_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

fn now_ms() -> Result<i64, CloudDocumentError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CloudDocumentError::ProtocolFailed)?
        .as_millis();
    i64::try_from(millis).map_err(|_| CloudDocumentError::ProtocolFailed)
}

fn map_store_error(error: CloudDocumentStoreError) -> CloudDocumentError {
    match error {
        CloudDocumentStoreError::BodyTooLarge => CloudDocumentError::TooLarge,
        CloudDocumentStoreError::ForeignSource => CloudDocumentError::Unauthorized,
        _ => CloudDocumentError::ProtocolFailed,
    }
}
