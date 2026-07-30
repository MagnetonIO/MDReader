//! Authorized point reads over published cloud Markdown metadata.

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use catdb_connectors::{CancellationToken, SourceId};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::{
    store::{
        CloudDocumentStore, CloudDocumentStoreError, CloudDocumentVersionPins, CloudWriteRepair,
        PersistedCloudWriteRepair,
    },
    CloudCacheState, CloudDocumentError, CloudIoContext, CloudMarkdownDocument,
    CloudMarkdownSaveOutcome, CloudWriteRepairExecutor, MarkdownDocumentRevision,
    MarkdownDocumentSource, MarkdownWriteReceipt, ProviderRevision, SourceRecordKey,
};

const READ_DEADLINE: Duration = Duration::from_secs(30);
const WRITE_DEADLINE: Duration = Duration::from_secs(30);
const EDIT_AUTHORITY_LIFETIME_MS: i64 = 30 * 60 * 1_000;
const EDIT_AUTHORITY_VERSION: u8 = 1;
const MAC_BYTES: usize = 32;

type HmacSha256 = Hmac<Sha256>;

struct DecodedEditAuthority {
    library_id: [u8; 16],
    source_id: Vec<u8>,
    source_version_id: Vec<u8>,
    record_key_digest: [u8; 32],
    revision: Vec<u8>,
    body_hash: [u8; 32],
}

#[derive(Deserialize, Serialize)]
struct CloudWriteReceiptPayload {
    version: u8,
    expected_revision: String,
    opened_body_hash: [u8; 32],
    content: String,
    modified_at_ms: Option<i64>,
    size_bytes: u64,
}

/// Local-only executor for provider receipts that already crossed the durable
/// journal boundary. It has no provider handle, so restart repair cannot
/// repeat a remote mutation.
pub(crate) struct CloudDocumentLocalPublisher {
    store: Arc<CloudDocumentStore>,
    pins: CloudDocumentVersionPins,
}

impl CloudDocumentLocalPublisher {
    pub(crate) fn new(store: Arc<CloudDocumentStore>, pins: CloudDocumentVersionPins) -> Self {
        Self { store, pins }
    }

    async fn publish(&self, repair: &PersistedCloudWriteRepair) -> Result<(), CloudDocumentError> {
        let payload: CloudWriteReceiptPayload = serde_json::from_slice(repair.receipt_json())
            .map_err(|_| CloudDocumentError::LocalPublicationPending)?;
        if payload.version != 1
            || payload.content.len() > CloudDocumentStore::MAX_BODY_BYTES
            || payload.size_bytes != payload.content.len() as u64
            || repair.body_hash() != blake3::hash(payload.content.as_bytes()).as_bytes()
        {
            return Err(CloudDocumentError::LocalPublicationPending);
        }
        let source_id = String::from_utf8(repair.source_id_bytes().to_vec())
            .map(SourceId::new)
            .map_err(|_| CloudDocumentError::LocalPublicationPending)?;
        let expected_revision = ProviderRevision {
            opaque: payload.expected_revision,
            content_hash: None,
        };
        let acceptance = repair
            .provider_acceptance()
            .map_err(|_| CloudDocumentError::LocalPublicationPending)?;
        let store = Arc::clone(&self.store);
        let pins = self.pins.clone();
        let body = payload.content.into_bytes();
        let modified_at_ms = payload.modified_at_ms;
        let observed_at_ms = repair.created_at_ms();
        tauri::async_runtime::spawn_blocking(move || {
            store.publish_provider_acceptance(
                &source_id,
                &pins,
                &acceptance,
                &expected_revision,
                &body,
                modified_at_ms,
                observed_at_ms,
            )
        })
        .await
        .map_err(|_| CloudDocumentError::LocalPublicationPending)?
        .map_err(|_| CloudDocumentError::LocalPublicationPending)
    }
}

#[async_trait::async_trait]
impl CloudWriteRepairExecutor for CloudDocumentLocalPublisher {
    async fn resume_local_publication(
        &self,
        repair: &PersistedCloudWriteRepair,
    ) -> Result<(), CloudDocumentError> {
        self.publish(repair).await
    }
}

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
        let decoded = self.decode_authenticated(token, now_ms)?;
        if decoded.library_id != *library_id
            || decoded.source_id != source_id.as_str().as_bytes()
            || decoded.source_version_id != source_version_id
            || decoded.record_key_digest != *blake3::hash(record_key.as_str().as_bytes()).as_bytes()
            || decoded.revision != revision.opaque.as_bytes()
            || decoded.body_hash != *blake3::hash(body).as_bytes()
        {
            return Err(CloudDocumentError::Unauthorized);
        }
        Ok(())
    }

    fn decode_authenticated(
        &self,
        token: &str,
        now_ms: i64,
    ) -> Result<DecodedEditAuthority, CloudDocumentError> {
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
        {
            return Err(CloudDocumentError::Unauthorized);
        }
        let library_id = decoder
            .fixed(16)?
            .try_into()
            .map_err(|_| CloudDocumentError::Unauthorized)?;
        let source_id = decoder.bounded()?.to_vec();
        let source_version_id = decoder.bounded()?.to_vec();
        let record_key_digest = decoder
            .fixed(32)?
            .try_into()
            .map_err(|_| CloudDocumentError::Unauthorized)?;
        let revision = decoder.bounded()?.to_vec();
        let body_hash = decoder
            .fixed(32)?
            .try_into()
            .map_err(|_| CloudDocumentError::Unauthorized)?;
        if !decoder.is_empty() {
            return Err(CloudDocumentError::Unauthorized);
        }
        Ok(DecodedEditAuthority {
            library_id,
            source_id,
            source_version_id,
            record_key_digest,
            revision,
            body_hash,
        })
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
            query_id: uuid::Uuid::new_v4(),
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

    pub(crate) async fn save_cloud_markdown(
        &self,
        edit_token: &str,
        content: &str,
    ) -> Result<CloudMarkdownSaveOutcome, CloudDocumentError> {
        if content.len() > CloudDocumentStore::MAX_BODY_BYTES {
            return Err(CloudDocumentError::TooLarge);
        }
        self.authority.revalidate().await?;
        let source_id = self.source.source_id().clone();
        let body_hash = *blake3::hash(content.as_bytes()).as_bytes();
        let mut operation = blake3::Hasher::new();
        operation.update(b"mdreader-cloud-write-v1\0");
        operation.update(edit_token.as_bytes());
        operation.update(&body_hash);
        let operation_id = operation.finalize();
        let existing_repair = {
            let store = Arc::clone(&self.store);
            let source_id = source_id.clone();
            let operation_id = operation_id.as_bytes().to_vec();
            tauri::async_runtime::spawn_blocking(move || {
                store.load_write_repair(&source_id, &operation_id)
            })
            .await
            .map_err(|_| CloudDocumentError::ProtocolFailed)?
            .map_err(map_store_error)?
        };
        if let Some(repair) = existing_repair {
            let claims = CloudEditAuthorityCodec::new(self.cursor_key)
                .decode_authenticated(edit_token, now_ms()?)?;
            let payload: CloudWriteReceiptPayload =
                serde_json::from_slice(repair.receipt_json())
                    .map_err(|_| CloudDocumentError::ProtocolFailed)?;
            if claims.library_id != self.library_id
                || claims.source_id != source_id.as_str().as_bytes()
                || claims.source_version_id != self.pins.source_version_id()
                || claims.record_key_digest
                    != *blake3::hash(repair.record_key().as_str().as_bytes()).as_bytes()
                || claims.revision != payload.expected_revision.as_bytes()
                || claims.body_hash != payload.opened_body_hash
                || repair.body_hash() != &body_hash
                || payload.size_bytes != content.len() as u64
                || payload.content != content
            {
                return Err(CloudDocumentError::Unauthorized);
            }
            if repair.is_provider_accepted() {
                let publisher =
                    CloudDocumentLocalPublisher::new(Arc::clone(&self.store), self.pins.clone());
                publisher.publish(&repair).await?;
                let store = Arc::clone(&self.store);
                let source_id = source_id.clone();
                let operation_id = repair.operation_id().to_vec();
                tauri::async_runtime::spawn_blocking(move || {
                    store.mark_write_published(&source_id, &operation_id)
                })
                .await
                .map_err(|_| CloudDocumentError::LocalPublicationPending)?
                .map_err(|_| CloudDocumentError::LocalPublicationPending)?;
            }
            self.authority.revalidate().await?;
            let edit_token = CloudEditAuthorityCodec::new(self.cursor_key).issue(
                now_ms()?,
                &self.library_id,
                &source_id,
                self.pins.source_version_id(),
                repair.record_key(),
                repair.accepted_revision(),
                content.as_bytes(),
            )?;
            return Ok(CloudMarkdownSaveOutcome::Saved {
                edit_token,
                modified_at_ms: payload.modified_at_ms,
            });
        }
        let published = {
            let store = Arc::clone(&self.store);
            let source_id = source_id.clone();
            tauri::async_runtime::spawn_blocking(move || store.list_published(&source_id))
                .await
                .map_err(|_| CloudDocumentError::ProtocolFailed)?
                .map_err(map_store_error)?
        };
        let mut opened = None;
        let accessed_at_ms = now_ms()?;
        for candidate in published
            .into_iter()
            .filter(|candidate| !candidate.deleted && is_markdown_name(&candidate.name))
        {
            if candidate.pins != self.pins {
                continue;
            }
            let store = Arc::clone(&self.store);
            let source_id_for_store = source_id.clone();
            let pins = self.pins.clone();
            let key = candidate.key.clone();
            let revision = candidate.revision.clone();
            let body = tauri::async_runtime::spawn_blocking(move || {
                store.get_body(&source_id_for_store, &pins, &key, &revision, accessed_at_ms)
            })
            .await
            .map_err(|_| CloudDocumentError::ProtocolFailed)?
            .map_err(map_store_error)?;
            if let Some(body) = body {
                if CloudEditAuthorityCodec::new(self.cursor_key)
                    .verify(
                        edit_token,
                        now_ms()?,
                        &self.library_id,
                        &source_id,
                        self.pins.source_version_id(),
                        &candidate.key,
                        &candidate.revision,
                        &body,
                    )
                    .is_ok()
                {
                    opened = Some((candidate, body));
                    break;
                }
            }
        }
        let Some((opened, opened_body)) = opened else {
            return Err(CloudDocumentError::Unauthorized);
        };

        let context = CloudIoContext {
            query_id: uuid::Uuid::new_v4(),
            deadline: Instant::now() + WRITE_DEADLINE,
            page_rows: 1,
            byte_limit: CloudDocumentStore::MAX_BODY_BYTES,
            cancellation: CancellationToken::new(),
        };
        let current = self.source.read_document(&opened.key, &context).await?;
        if current.key != opened.key {
            return Err(CloudDocumentError::IdentityMismatch);
        }
        self.authority.revalidate().await?;
        let receipt = if current.revision != opened.revision {
            if current.body != content.as_bytes() {
                return Ok(CloudMarkdownSaveOutcome::Conflict {});
            }
            MarkdownWriteReceipt {
                key: current.key,
                revision: current.revision,
                modified_at_ms: None,
                size_bytes: current.body.len() as u64,
            }
        } else {
            if current.body != opened_body {
                return Ok(CloudMarkdownSaveOutcome::Conflict {});
            }
            match self
                .source
                .write_document(&opened.key, &opened.revision, content.as_bytes(), &context)
                .await
            {
                Ok(receipt) => receipt,
                Err(CloudDocumentError::Conflict) => {
                    return Ok(CloudMarkdownSaveOutcome::Conflict {});
                }
                Err(error) => return Err(error),
            }
        };
        if receipt.key != opened.key
            || receipt.revision == opened.revision
            || receipt.size_bytes != content.len() as u64
        {
            return Err(CloudDocumentError::WriteOutcomeUnknown);
        }
        let receipt_json = serde_json::to_vec(&CloudWriteReceiptPayload {
            version: 1,
            expected_revision: opened.revision.opaque.clone(),
            opened_body_hash: *blake3::hash(&opened_body).as_bytes(),
            content: content.to_string(),
            modified_at_ms: receipt.modified_at_ms,
            size_bytes: receipt.size_bytes,
        })
        .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let repair = CloudWriteRepair::new(
            operation_id.as_bytes(),
            &source_id,
            opened.key.clone(),
            receipt.revision.clone(),
            body_hash,
            receipt_json,
            now_ms()?,
        )
        .map_err(map_store_error)?;
        {
            let store = Arc::clone(&self.store);
            let source_id = source_id.clone();
            let operation_id = operation_id.as_bytes().to_vec();
            let pins = self.pins.clone();
            let expected_revision = opened.revision.clone();
            let content = content.as_bytes().to_vec();
            let modified_at_ms = receipt.modified_at_ms;
            let observed_at_ms = now_ms()?;
            tauri::async_runtime::spawn_blocking(move || {
                store.record_provider_acceptance(&repair)?;
                store.publish_provider_acceptance(
                    &source_id,
                    &pins,
                    &repair,
                    &expected_revision,
                    &content,
                    modified_at_ms,
                    observed_at_ms,
                )?;
                store.mark_write_published(&source_id, &operation_id)
            })
            .await
            .map_err(|_| CloudDocumentError::LocalPublicationPending)?
            .map_err(|_| CloudDocumentError::LocalPublicationPending)?;
        }
        let edit_token = CloudEditAuthorityCodec::new(self.cursor_key).issue(
            now_ms()?,
            &self.library_id,
            &source_id,
            self.pins.source_version_id(),
            &opened.key,
            &receipt.revision,
            content.as_bytes(),
        )?;
        Ok(CloudMarkdownSaveOutcome::Saved {
            edit_token,
            modified_at_ms: receipt.modified_at_ms,
        })
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
