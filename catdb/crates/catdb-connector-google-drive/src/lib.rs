//! Read-only Google Drive API v3 source for an MDReader Library.
//!
//! The connector uses Google's per-file `drive.file` OAuth scope and exposes
//! no write request at all. The scope is deliberately named exactly rather
//! than called a read-only permission: CatDB's connector contract, not an
//! overclaim about Google's scope, is what makes this source read-only.
//! Interactive OAuth credentials and cursor-authentication keys are resolved
//! at runtime and never enter the immutable source specification.

#![deny(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use catdb_connectors::{
    DataSource, ReadContext, SourceCapabilities, SourceCursor, SourceError, SourceFreshness,
    SourceId, SourceModel, SourcePage, SourcePlan, SourceRow,
};
use catdb_core::{BaseType, Schema};
use catdb_query::EntityQuery;
use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use time::format_description::well_known::Rfc3339;
use url::Url;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// Google's least-privilege per-file scope used for Picker-selected content.
///
/// Google does not describe this scope as read-only. Read-only behavior is
/// enforced by this connector exposing only GET/list/export/download calls.
pub const GOOGLE_DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

const API_BASE: &str = "https://www.googleapis.com";
const AUTHORIZATION_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const MAX_HTTP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONFIGURED_SELECTIONS: usize = 512;
const MAX_SELECTION_ID_BYTES: usize = 256;
const MAX_CHANGE_EVENTS: usize = 1_000;
const CURSOR_VERSION: u8 = 1;
const QUERY_CURSOR_DOMAIN: &[u8] = b"catdb-google-drive-query-cursor\0";
const CHANGE_CURSOR_DOMAIN: &[u8] = b"catdb-google-drive-change-cursor\0";

/// Runtime secret lookup key stored in the immutable source specification.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveSecretReference(String);

impl DriveSecretReference {
    /// Validate a non-empty bounded secret reference.
    pub fn new(reference: impl Into<String>) -> Result<Self, DriveConnectorError> {
        let reference = reference.into();
        if reference.is_empty() || reference.len() > 512 || reference.chars().any(char::is_control)
        {
            return Err(DriveConnectorError::InvalidConfiguration {
                field: "credential_reference",
            });
        }
        Ok(Self(reference))
    }
}

impl fmt::Debug for DriveSecretReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveSecretReference([redacted-reference])")
    }
}

/// One Drive item explicitly selected for an MDReader Library.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DriveSelection {
    /// A specific Picker-selected file.
    File {
        /// Stable Google Drive file id.
        id: String,
    },
    /// A Picker-selected folder whose visible direct children are listed.
    Folder {
        /// Stable Google Drive folder id.
        id: String,
    },
}

impl DriveSelection {
    /// Select one file.
    pub fn file(id: impl Into<String>) -> Self {
        Self::File { id: id.into() }
    }

    /// Select one folder.
    pub fn folder(id: impl Into<String>) -> Self {
        Self::Folder { id: id.into() }
    }

    fn id(&self) -> &str {
        match self {
            Self::File { id } | Self::Folder { id } => id,
        }
    }

    fn validate(&self) -> Result<(), DriveConnectorError> {
        validate_drive_id("selection", self.id())
    }
}

/// Immutable, non-secret Google Drive source configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveSourceConfig {
    source_id: String,
    catdb_schema: String,
    catdb_entity: String,
    selections: Vec<DriveSelection>,
    credential_reference: DriveSecretReference,
    shared_drive_id: Option<String>,
}

impl DriveSourceConfig {
    /// Configure a selected-file Google Drive source.
    pub fn new(
        source_id: impl Into<String>,
        catdb_schema: impl Into<String>,
        catdb_entity: impl Into<String>,
        selections: Vec<DriveSelection>,
        credential_reference: DriveSecretReference,
    ) -> Result<Self, DriveConnectorError> {
        let config = Self {
            source_id: source_id.into(),
            catdb_schema: catdb_schema.into(),
            catdb_entity: catdb_entity.into(),
            selections,
            credential_reference,
            shared_drive_id: None,
        };
        config.validate()?;
        Ok(config)
    }

    /// Restrict listing and change tracking to one shared Drive.
    pub fn with_shared_drive(
        mut self,
        drive_id: impl Into<String>,
    ) -> Result<Self, DriveConnectorError> {
        let drive_id = drive_id.into();
        validate_drive_id("shared_drive_id", &drive_id)?;
        self.shared_drive_id = Some(drive_id);
        Ok(self)
    }

    /// Exact OAuth scope required by the connector.
    pub fn oauth_scope(&self) -> &'static str {
        GOOGLE_DRIVE_FILE_SCOPE
    }

    fn validate(&self) -> Result<(), DriveConnectorError> {
        for (field, value) in [
            ("source_id", self.source_id.as_str()),
            ("catdb_schema", self.catdb_schema.as_str()),
            ("catdb_entity", self.catdb_entity.as_str()),
        ] {
            if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err(DriveConnectorError::InvalidConfiguration { field });
            }
        }
        if self.selections.is_empty() || self.selections.len() > MAX_CONFIGURED_SELECTIONS {
            return Err(DriveConnectorError::InvalidConfiguration {
                field: "selections",
            });
        }
        let mut unique = BTreeSet::new();
        for selection in &self.selections {
            selection.validate()?;
            if !unique.insert(selection.clone()) {
                return Err(DriveConnectorError::InvalidConfiguration {
                    field: "selections",
                });
            }
        }
        if let Some(drive_id) = &self.shared_drive_id {
            validate_drive_id("shared_drive_id", drive_id)?;
        }
        Ok(())
    }
}

/// Catalog-pinned Drive configuration and semantic schema identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedDriveSourceSpec {
    config: DriveSourceConfig,
    schema_version: String,
}

impl PinnedDriveSourceSpec {
    /// Pin non-secret source configuration to an immutable schema version.
    pub fn new(
        config: DriveSourceConfig,
        schema_version: impl Into<String>,
    ) -> Result<Self, DriveConnectorError> {
        config.validate()?;
        let schema_version = schema_version.into();
        if schema_version.is_empty()
            || schema_version.len() > 256
            || schema_version.chars().any(char::is_control)
        {
            return Err(DriveConnectorError::InvalidConfiguration {
                field: "schema_version",
            });
        }
        Ok(Self {
            config,
            schema_version,
        })
    }
}

/// OAuth access token held only in runtime memory.
pub struct DriveAccessToken(Zeroizing<String>);

impl DriveAccessToken {
    /// Construct a non-empty access token.
    pub fn new(token: impl Into<String>) -> Result<Self, DriveConnectorError> {
        let token = token.into();
        if token.is_empty() || token.len() > 16 * 1024 || token.chars().any(char::is_control) {
            return Err(DriveConnectorError::InvalidSecret);
        }
        Ok(Self(Zeroizing::new(token)))
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for DriveAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveAccessToken([redacted])")
    }
}

/// Stable 256-bit key authenticating opaque Drive cursors.
pub struct DriveCursorKey([u8; 32]);

impl DriveCursorKey {
    /// Construct a cursor key.
    pub fn new(key: [u8; 32]) -> Self {
        Self(key)
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for DriveCursorKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for DriveCursorKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveCursorKey([redacted])")
    }
}

/// Runtime-only Drive token and cursor key.
pub struct DriveRuntimeSecrets {
    token: DriveAccessToken,
    cursor_key: DriveCursorKey,
}

impl DriveRuntimeSecrets {
    /// Bind OAuth credentials to a stable cursor key.
    pub fn new(token: DriveAccessToken, cursor_key: DriveCursorKey) -> Self {
        Self { token, cursor_key }
    }

    fn into_parts(self) -> (DriveAccessToken, DriveCursorKey) {
        (self.token, self.cursor_key)
    }
}

impl fmt::Debug for DriveRuntimeSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveRuntimeSecrets([redacted])")
    }
}

/// Redacted evidence that runtime-only credentials are installed.
#[derive(Clone, Copy)]
pub struct DriveRuntimeSecretEvidence;

impl fmt::Debug for DriveRuntimeSecretEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveRuntimeSecretEvidence([redacted])")
    }
}

/// Secret resolver used by application-owned OAuth storage.
#[async_trait]
pub trait DriveSecretResolver: Send + Sync {
    /// Resolve token material without serializing it into the source spec.
    async fn resolve(
        &self,
        reference: &DriveSecretReference,
    ) -> Result<DriveRuntimeSecrets, DriveConnectorError>;
}

/// OAuth Authorization Code with PKCE state owned by one native login attempt.
pub struct DriveOAuthPkce {
    verifier: Zeroizing<String>,
    state: String,
    redirect_uri: String,
    challenge: String,
}

impl DriveOAuthPkce {
    /// Validate RFC 7636 verifier/state/redirect data and derive S256.
    pub fn new(
        verifier: impl Into<String>,
        state: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Result<Self, DriveConnectorError> {
        let verifier = verifier.into();
        let state = state.into();
        let redirect_uri = redirect_uri.into();
        if !(43..=128).contains(&verifier.len())
            || !verifier.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
            })
        {
            return Err(DriveConnectorError::InvalidPkce);
        }
        if state.is_empty() || state.len() > 512 || state.chars().any(char::is_control) {
            return Err(DriveConnectorError::InvalidPkce);
        }
        let redirect = Url::parse(&redirect_uri).map_err(|_| DriveConnectorError::InvalidPkce)?;
        let valid_redirect = redirect.scheme() == "https"
            || (redirect.scheme() == "http"
                && matches!(redirect.host_str(), Some("127.0.0.1" | "::1" | "localhost")));
        if !valid_redirect {
            return Err(DriveConnectorError::InvalidPkce);
        }
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Ok(Self {
            verifier: Zeroizing::new(verifier),
            state,
            redirect_uri,
            challenge,
        })
    }

    /// Derived URL-safe S256 code challenge.
    pub fn code_challenge(&self) -> &str {
        &self.challenge
    }

    /// Build the native-app authorization URL without a client secret.
    pub fn authorization_url(&self, client_id: &str) -> Result<String, DriveConnectorError> {
        if client_id.is_empty() || client_id.len() > 512 || client_id.chars().any(char::is_control)
        {
            return Err(DriveConnectorError::InvalidPkce);
        }
        let mut url =
            Url::parse(AUTHORIZATION_ENDPOINT).map_err(|_| DriveConnectorError::InvalidPkce)?;
        url.query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", GOOGLE_DRIVE_FILE_SCOPE)
            .append_pair("code_challenge", &self.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &self.state)
            .append_pair("access_type", "offline");
        Ok(url.into())
    }

    /// Verify the callback state before a caller exchanges the authorization code.
    pub fn verify_state(&self, callback_state: &str) -> bool {
        constant_time_equal(self.state.as_bytes(), callback_state.as_bytes())
    }

    /// Runtime-only verifier used by the application-owned token exchange.
    pub fn verifier(&self) -> &str {
        self.verifier.as_str()
    }
}

impl fmt::Debug for DriveOAuthPkce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveOAuthPkce")
            .field("verifier", &"[redacted]")
            .field("state", &"[redacted]")
            .field("redirect_uri", &self.redirect_uri)
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// HTTP method emitted by this read-only connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveHttpMethod {
    /// Read/list/export/download.
    Get,
}

/// Bounded authenticated Drive API request.
#[derive(Clone)]
pub struct DriveHttpRequest {
    method: DriveHttpMethod,
    path: String,
    bearer_token: Arc<str>,
}

impl DriveHttpRequest {
    /// Request method. Always GET for V1.
    pub fn method(&self) -> DriveHttpMethod {
        self.method
    }

    /// API-relative path and encoded query.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Debug for DriveHttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveHttpRequest")
            .field("method", &self.method)
            .field("path", &redacted_path(&self.path))
            .field("bearer_token", &"[redacted]")
            .finish()
    }
}

/// Bounded Drive HTTP response.
#[derive(Clone, PartialEq, Eq)]
pub struct DriveHttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl DriveHttpResponse {
    /// Validate status, headers, and the fixed response-body bound.
    pub fn new(
        status: u16,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<Self, DriveTransportError> {
        if !(100..=599).contains(&status) {
            return Err(DriveTransportError::InvalidResponse);
        }
        if body.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(DriveTransportError::ResponseTooLarge);
        }
        Ok(Self {
            status,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value))
                .collect(),
            body,
        })
    }
}

impl fmt::Debug for DriveHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveHttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Failure beneath the semantic connector boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DriveTransportError {
    /// Network or TLS failed.
    #[error("Drive transport failed")]
    Failed,
    /// Response exceeded the fixed connector bound.
    #[error("Drive response exceeded the fixed body bound")]
    ResponseTooLarge,
    /// Response metadata was invalid.
    #[error("Drive response metadata was invalid")]
    InvalidResponse,
}

/// Injected transport used by protocol fixtures and the production HTTPS client.
#[async_trait]
pub trait DriveTransport: Send + Sync {
    /// Send one bounded request.
    async fn send(
        &self,
        request: DriveHttpRequest,
    ) -> Result<DriveHttpResponse, DriveTransportError>;
}

/// Rustls-backed Google Drive transport.
pub struct ReqwestDriveTransport {
    client: reqwest::Client,
}

impl ReqwestDriveTransport {
    /// Build a transport with redirects disabled.
    pub fn new() -> Result<Self, DriveConnectorError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| DriveConnectorError::Transport)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl DriveTransport for ReqwestDriveTransport {
    async fn send(
        &self,
        request: DriveHttpRequest,
    ) -> Result<DriveHttpResponse, DriveTransportError> {
        let url = Url::parse(API_BASE)
            .and_then(|base| base.join(&request.path))
            .map_err(|_| DriveTransportError::InvalidResponse)?;
        if url.scheme() != "https" || url.host_str() != Some("www.googleapis.com") {
            return Err(DriveTransportError::InvalidResponse);
        }
        let response = self
            .client
            .get(url)
            .bearer_auth(request.bearer_token.as_ref())
            .send()
            .await
            .map_err(|_| DriveTransportError::Failed)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_string(), value.to_string()))
            })
            .collect();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| DriveTransportError::Failed)?;
            let next = body
                .len()
                .checked_add(chunk.len())
                .ok_or(DriveTransportError::ResponseTooLarge)?;
            if next > MAX_HTTP_RESPONSE_BYTES {
                return Err(DriveTransportError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        DriveHttpResponse::new(status, headers, body)
    }
}

/// Evidence level for this implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveAvailability {
    /// Protocol fixtures passed; no live Google account has been claimed.
    FixtureValidated,
}

/// Opaque authenticated Drive changes position.
#[derive(Clone, PartialEq, Eq)]
pub struct DriveChangeCursor(Vec<u8>);

impl fmt::Debug for DriveChangeCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveChangeCursor([opaque])")
    }
}

/// Revision material used to distinguish unchanged content from an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveKnownRevision {
    modified_time: String,
    checksum: Option<String>,
}

impl DriveKnownRevision {
    /// Validate a known remote revision.
    pub fn new(
        modified_time: impl Into<String>,
        checksum: Option<impl Into<String>>,
    ) -> Result<Self, DriveConnectorError> {
        let modified_time = modified_time.into();
        parse_timestamp_ms(&modified_time).map_err(|_| DriveConnectorError::InvalidRevision)?;
        let checksum = checksum.map(Into::into);
        if checksum
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 512)
        {
            return Err(DriveConnectorError::InvalidRevision);
        }
        Ok(Self {
            modified_time,
            checksum,
        })
    }

    fn matches(&self, file: &DriveFile) -> bool {
        self.modified_time == file.modified_time
            && self.checksum.as_deref() == file.md5_checksum.as_deref()
    }
}

/// Provider change classified against the Library's known revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveChangeEvent {
    /// A tracked file was removed or trashed.
    Deleted {
        /// Stable Drive file id.
        file_id: String,
    },
    /// Provider revision is byte-equivalent to the known materialization.
    Unchanged {
        /// Stable Drive file id.
        file_id: String,
    },
    /// A tracked file is new or changed.
    Upserted {
        /// Stable Drive file id.
        file_id: String,
        /// Provider modified time.
        modified_time: String,
        /// Provider checksum when Drive supplies one.
        checksum: Option<String>,
    },
}

/// One explicit incremental-refresh outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveChangeOutcome {
    /// A complete provider page and its continuation/checkpoint.
    Page {
        /// Classified changes for tracked content.
        events: Vec<DriveChangeEvent>,
        /// Next provider page, if the same refresh has more work.
        next_cursor: Option<DriveChangeCursor>,
        /// New durable start token, present only after the refresh is exhausted.
        checkpoint: Option<DriveChangeCursor>,
    },
    /// Provider expired the start token; callers must rebuild, never emit empty.
    RebuildRequired,
}

#[derive(Serialize, Deserialize)]
struct QueryCursorPayload {
    version: u8,
    connector: String,
    source_spec_digest: String,
    schema_version: String,
    plan_digest: String,
    query_id: Uuid,
    page_token: String,
}

#[derive(Serialize, Deserialize)]
struct ChangeCursorPayload {
    version: u8,
    connector: String,
    source_spec_digest: String,
    schema_version: String,
    page_token: String,
}

#[derive(Serialize, Deserialize)]
struct CursorEnvelope<T> {
    payload: T,
    authentication_tag: Vec<u8>,
}

/// Read-only Drive source over selected items.
pub struct DriveSource {
    spec: PinnedDriveSourceSpec,
    model: SourceModel,
    source_spec: serde_json::Value,
    source_spec_digest: String,
    token: DriveAccessToken,
    cursor_key: DriveCursorKey,
    transport: Arc<dyn DriveTransport>,
}

impl fmt::Debug for DriveSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveSource")
            .field("source_id", self.model.id())
            .field("schema_version", &self.spec.schema_version)
            .field("runtime_secrets", &"[redacted]")
            .finish()
    }
}

impl DriveSource {
    /// Resolve credentials and open with the production HTTPS transport.
    pub async fn open_pinned(
        spec: PinnedDriveSourceSpec,
        resolver: &dyn DriveSecretResolver,
    ) -> Result<Self, DriveConnectorError> {
        spec.config.validate()?;
        let secrets = resolver.resolve(&spec.config.credential_reference).await?;
        Self::open_with_transport(spec, secrets, Arc::new(ReqwestDriveTransport::new()?))
    }

    /// Open a pinned source with injected runtime dependencies.
    pub fn open_with_transport(
        spec: PinnedDriveSourceSpec,
        secrets: DriveRuntimeSecrets,
        transport: Arc<dyn DriveTransport>,
    ) -> Result<Self, DriveConnectorError> {
        spec.config.validate()?;
        let model = SourceModel::new(
            SourceId::new(spec.config.source_id.clone()),
            &spec.config.catdb_schema,
            &spec.config.catdb_entity,
            vec![
                catdb_connectors::SourceAttr::text("id"),
                catdb_connectors::SourceAttr::text("name"),
                catdb_connectors::SourceAttr::text("mimeType"),
                catdb_connectors::SourceAttr::parsed("modifiedAt", BaseType::Timestamp),
                catdb_connectors::SourceAttr::parsed("size", BaseType::Int),
                catdb_connectors::SourceAttr::text("checksum"),
                catdb_connectors::SourceAttr::text("body"),
            ],
        );
        let source_spec =
            serde_json::to_value(&spec).map_err(|_| DriveConnectorError::InvalidConfiguration {
                field: "source_spec",
            })?;
        let source_spec_digest = digest(&spec)?;
        let (token, cursor_key) = secrets.into_parts();
        Ok(Self {
            spec,
            model,
            source_spec,
            source_spec_digest,
            token,
            cursor_key,
            transport,
        })
    }

    /// Fixture-only evidence; never a live-provider availability claim.
    pub fn availability_evidence(&self) -> DriveAvailability {
        DriveAvailability::FixtureValidated
    }

    /// Redacted evidence that runtime secrets are attached.
    pub fn runtime_secret_evidence(&self) -> DriveRuntimeSecretEvidence {
        DriveRuntimeSecretEvidence
    }

    /// Obtain the provider's current incremental-change start token.
    pub async fn start_change_tracking(
        &self,
        context: &ReadContext,
    ) -> Result<DriveChangeCursor, SourceError> {
        context.check()?;
        let mut physical_bytes = 0usize;
        let response = self
            .send(
                self.request(self.change_start_path()),
                context,
                &mut physical_bytes,
            )
            .await?;
        if response.status != 200 {
            return Err(self.map_http_failure(&response));
        }
        let body: DriveStartPageTokenResponse =
            serde_json::from_slice(&response.body).map_err(|_| self.malformed("startPageToken"))?;
        validate_provider_token(&body.start_page_token)?;
        self.encode_change_cursor(&body.start_page_token)
    }

    /// Read one provider change page without collapsing reset or deletion.
    pub async fn read_changes(
        &self,
        cursor: &DriveChangeCursor,
        known: &BTreeMap<String, DriveKnownRevision>,
        context: &ReadContext,
    ) -> Result<DriveChangeOutcome, SourceError> {
        context.check()?;
        if known.len() > MAX_CHANGE_EVENTS {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "known revisions",
            });
        }
        let token = self.decode_change_cursor(cursor)?;
        let mut physical_bytes = 0usize;
        let response = self
            .send(
                self.request(self.change_list_path(&token, context.page_size())),
                context,
                &mut physical_bytes,
            )
            .await?;
        if response.status == 410 {
            return Ok(DriveChangeOutcome::RebuildRequired);
        }
        if response.status != 200 {
            return Err(self.map_http_failure(&response));
        }
        let body: DriveChangesResponse =
            serde_json::from_slice(&response.body).map_err(|_| self.malformed("changes"))?;
        if body.changes.len() > context.page_size() || body.changes.len() > MAX_CHANGE_EVENTS {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "change page",
            });
        }
        let mut events = Vec::new();
        for change in body.changes {
            context.check()?;
            if change.change_type != "file" {
                return Err(self.malformed("changeType"));
            }
            if change.removed {
                if known.contains_key(&change.file_id) {
                    events.push(DriveChangeEvent::Deleted {
                        file_id: change.file_id,
                    });
                }
                continue;
            }
            let file = change.file.ok_or_else(|| self.malformed("change.file"))?;
            file.validate(self.model.id())?;
            if file.id != change.file_id {
                return Err(self.malformed("change.fileId"));
            }
            if file.trashed {
                if known.contains_key(&file.id) {
                    events.push(DriveChangeEvent::Deleted { file_id: file.id });
                }
            } else if self.is_tracked(&file, known.contains_key(&file.id)) {
                if known
                    .get(&file.id)
                    .is_some_and(|revision| revision.matches(&file))
                {
                    events.push(DriveChangeEvent::Unchanged { file_id: file.id });
                } else {
                    events.push(DriveChangeEvent::Upserted {
                        file_id: file.id,
                        modified_time: file.modified_time,
                        checksum: file.md5_checksum,
                    });
                }
            }
        }
        let next_cursor = body
            .next_page_token
            .as_deref()
            .map(|token| {
                validate_provider_token(token)?;
                self.encode_change_cursor(token)
            })
            .transpose()?;
        let checkpoint = body
            .new_start_page_token
            .as_deref()
            .map(|token| {
                validate_provider_token(token)?;
                self.encode_change_cursor(token)
            })
            .transpose()?;
        if next_cursor.is_some() == checkpoint.is_some() {
            return Err(self.malformed("change continuation"));
        }
        Ok(DriveChangeOutcome::Page {
            events,
            next_cursor,
            checkpoint,
        })
    }

    fn request(&self, path: String) -> DriveHttpRequest {
        DriveHttpRequest {
            method: DriveHttpMethod::Get,
            path,
            bearer_token: Arc::from(self.token.as_str()),
        }
    }

    async fn send(
        &self,
        request: DriveHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
    ) -> Result<DriveHttpResponse, SourceError> {
        context.check()?;
        let deadline = tokio::time::Instant::from_std(context.deadline());
        let result = tokio::select! {
            biased;
            _ = context.cancellation().cancelled() => return Err(SourceError::Cancelled),
            result = tokio::time::timeout_at(deadline, self.transport.send(request)) => result,
        };
        let response = result
            .map_err(|_| SourceError::DeadlineExceeded)?
            .map_err(|error| match error {
                DriveTransportError::ResponseTooLarge => SourceError::ByteBudgetExceeded,
                DriveTransportError::Failed | DriveTransportError::InvalidResponse => {
                    SourceError::Transport {
                        source_id: self.model.id().clone(),
                    }
                }
            })?;
        *physical_bytes = physical_bytes
            .checked_add(response.body.len())
            .ok_or(SourceError::ByteBudgetExceeded)?;
        if *physical_bytes > context.remaining_bytes() {
            return Err(SourceError::ByteBudgetExceeded);
        }
        context.check()?;
        Ok(response)
    }

    fn map_http_failure(&self, response: &DriveHttpResponse) -> SourceError {
        match response.status {
            401 | 403 => SourceError::Authentication {
                source_id: self.model.id().clone(),
            },
            408 | 504 => SourceError::StatementTimeout {
                source_id: self.model.id().clone(),
            },
            429 => SourceError::RateLimited {
                source_id: self.model.id().clone(),
                retry_after_ms: retry_after_ms(response),
            },
            _ => SourceError::Transport {
                source_id: self.model.id().clone(),
            },
        }
    }

    fn list_path(&self, page_token: Option<&str>, page_size: usize) -> String {
        let mut params = vec![
            (
                "fields",
                "files(id,name,mimeType,modifiedTime,size,md5Checksum,parents,trashed),nextPageToken,incompleteSearch".to_string(),
            ),
            ("pageSize", page_size.to_string()),
            ("q", self.selection_query()),
            ("spaces", "drive".to_string()),
            ("supportsAllDrives", "true".to_string()),
        ];
        if let Some(token) = page_token {
            params.push(("pageToken", token.to_string()));
        }
        if let Some(drive_id) = &self.spec.config.shared_drive_id {
            params.push(("corpora", "drive".to_string()));
            params.push(("driveId", drive_id.clone()));
            params.push(("includeItemsFromAllDrives", "true".to_string()));
        }
        encoded_path("/drive/v3/files", params)
    }

    fn selection_query(&self) -> String {
        let mut clauses = Vec::new();
        for selection in &self.spec.config.selections {
            match selection {
                DriveSelection::File { id } => {
                    clauses.push(format!("id = '{}'", escape_query_literal(id)));
                }
                DriveSelection::Folder { id } => {
                    clauses.push(format!("'{}' in parents", escape_query_literal(id)));
                }
            }
        }
        format!("trashed = false and ({})", clauses.join(" or "))
    }

    fn body_path(&self, file: &DriveFile) -> Option<String> {
        let encoded_id = encode_path_segment(&file.id);
        match file.mime_type.as_str() {
            "application/vnd.google-apps.document" => Some(encoded_path(
                &format!("/drive/v3/files/{encoded_id}/export"),
                vec![
                    ("mimeType", "text/plain".to_string()),
                    ("supportsAllDrives", "true".to_string()),
                ],
            )),
            "text/markdown" | "text/plain" => Some(encoded_path(
                &format!("/drive/v3/files/{encoded_id}"),
                vec![
                    ("alt", "media".to_string()),
                    ("supportsAllDrives", "true".to_string()),
                ],
            )),
            _ => None,
        }
    }

    fn change_start_path(&self) -> String {
        let mut params = vec![("supportsAllDrives", "true".to_string())];
        if let Some(drive_id) = &self.spec.config.shared_drive_id {
            params.push(("driveId", drive_id.clone()));
        }
        encoded_path("/drive/v3/changes/startPageToken", params)
    }

    fn change_list_path(&self, page_token: &str, page_size: usize) -> String {
        let mut params = vec![
            ("pageToken", page_token.to_string()),
            ("pageSize", page_size.to_string()),
            (
                "fields",
                "changes(changeType,removed,fileId,file(id,name,mimeType,modifiedTime,size,md5Checksum,parents,trashed)),nextPageToken,newStartPageToken".to_string(),
            ),
            ("spaces", "drive".to_string()),
            ("supportsAllDrives", "true".to_string()),
            ("includeItemsFromAllDrives", "true".to_string()),
        ];
        if let Some(drive_id) = &self.spec.config.shared_drive_id {
            params.push(("driveId", drive_id.clone()));
        }
        encoded_path("/drive/v3/changes", params)
    }

    fn is_tracked(&self, file: &DriveFile, already_known: bool) -> bool {
        already_known
            || self
                .spec
                .config
                .selections
                .iter()
                .any(|selection| match selection {
                    DriveSelection::File { id } => id == &file.id,
                    DriveSelection::Folder { id } => file.parents.iter().any(|parent| parent == id),
                })
    }

    fn validate_plan(&self, plan: &SourcePlan) -> Result<(), SourceError> {
        if plan.source != *self.model.id() || !plan.pushed.filters.is_empty() {
            return Err(SourceError::InvalidPlan {
                source_id: self.model.id().clone(),
            });
        }
        for field in plan
            .projected
            .iter()
            .map(String::as_str)
            .chain(plan.residual_attrs())
        {
            if !self.model.capabilities().observes(field) {
                return Err(SourceError::InvalidPlan {
                    source_id: self.model.id().clone(),
                });
            }
        }
        Ok(())
    }

    fn encode_query_cursor(
        &self,
        plan: &SourcePlan,
        query_id: Uuid,
        page_token: String,
    ) -> Result<SourceCursor, SourceError> {
        validate_provider_token(&page_token)?;
        let payload = QueryCursorPayload {
            version: CURSOR_VERSION,
            connector: "catdb-google-drive-source/v1".to_string(),
            source_spec_digest: self.source_spec_digest.clone(),
            schema_version: self.spec.schema_version.clone(),
            plan_digest: plan_digest(plan)?,
            query_id,
            page_token,
        };
        let authentication_tag = self.authenticate(QUERY_CURSOR_DOMAIN, &payload)?;
        let bytes = serde_json::to_vec(&CursorEnvelope {
            payload,
            authentication_tag,
        })
        .map_err(|_| SourceError::InvalidCursor)?;
        SourceCursor::new(bytes)
    }

    fn decode_query_cursor(
        &self,
        plan: &SourcePlan,
        cursor: &SourceCursor,
        query_id: Uuid,
    ) -> Result<String, SourceError> {
        let envelope: CursorEnvelope<QueryCursorPayload> =
            serde_json::from_slice(cursor.as_bytes()).map_err(|_| SourceError::InvalidCursor)?;
        if !self.verify(
            QUERY_CURSOR_DOMAIN,
            &envelope.payload,
            &envelope.authentication_tag,
        )? || envelope.payload.version != CURSOR_VERSION
            || envelope.payload.connector != "catdb-google-drive-source/v1"
            || envelope.payload.source_spec_digest != self.source_spec_digest
            || envelope.payload.schema_version != self.spec.schema_version
            || envelope.payload.plan_digest != plan_digest(plan)?
            || envelope.payload.query_id != query_id
        {
            return Err(SourceError::InvalidCursor);
        }
        validate_provider_token(&envelope.payload.page_token)?;
        Ok(envelope.payload.page_token)
    }

    fn encode_change_cursor(&self, page_token: &str) -> Result<DriveChangeCursor, SourceError> {
        let payload = ChangeCursorPayload {
            version: CURSOR_VERSION,
            connector: "catdb-google-drive-changes/v1".to_string(),
            source_spec_digest: self.source_spec_digest.clone(),
            schema_version: self.spec.schema_version.clone(),
            page_token: page_token.to_string(),
        };
        let authentication_tag = self.authenticate(CHANGE_CURSOR_DOMAIN, &payload)?;
        let bytes = serde_json::to_vec(&CursorEnvelope {
            payload,
            authentication_tag,
        })
        .map_err(|_| SourceError::InvalidCursor)?;
        if bytes.len() > catdb_connectors::MAX_SOURCE_CURSOR_BYTES {
            return Err(SourceError::CursorTooLarge {
                actual: bytes.len(),
                maximum: catdb_connectors::MAX_SOURCE_CURSOR_BYTES,
            });
        }
        Ok(DriveChangeCursor(bytes))
    }

    fn decode_change_cursor(&self, cursor: &DriveChangeCursor) -> Result<String, SourceError> {
        let envelope: CursorEnvelope<ChangeCursorPayload> =
            serde_json::from_slice(&cursor.0).map_err(|_| SourceError::InvalidCursor)?;
        if !self.verify(
            CHANGE_CURSOR_DOMAIN,
            &envelope.payload,
            &envelope.authentication_tag,
        )? || envelope.payload.version != CURSOR_VERSION
            || envelope.payload.connector != "catdb-google-drive-changes/v1"
            || envelope.payload.source_spec_digest != self.source_spec_digest
            || envelope.payload.schema_version != self.spec.schema_version
        {
            return Err(SourceError::InvalidCursor);
        }
        validate_provider_token(&envelope.payload.page_token)?;
        Ok(envelope.payload.page_token)
    }

    fn authenticate<T: Serialize>(
        &self,
        domain: &[u8],
        payload: &T,
    ) -> Result<Vec<u8>, SourceError> {
        let bytes = serde_json::to_vec(payload).map_err(|_| SourceError::InvalidCursor)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.cursor_key.as_bytes())
            .map_err(|_| SourceError::InvalidCursor)?;
        mac.update(domain);
        mac.update(&bytes);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    fn verify<T: Serialize>(
        &self,
        domain: &[u8],
        payload: &T,
        tag: &[u8],
    ) -> Result<bool, SourceError> {
        let bytes = serde_json::to_vec(payload).map_err(|_| SourceError::InvalidCursor)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.cursor_key.as_bytes())
            .map_err(|_| SourceError::InvalidCursor)?;
        mac.update(domain);
        mac.update(&bytes);
        Ok(mac.verify_slice(tag).is_ok())
    }

    fn malformed(&self, attr: &'static str) -> SourceError {
        SourceError::Malformed {
            source_id: self.model.id().clone(),
            path: "google-drive-response".to_string(),
            attr: attr.to_string(),
            detail: "provider response did not match the pinned Drive API v3 contract".to_string(),
        }
    }
}

#[async_trait]
impl DataSource for DriveSource {
    fn id(&self) -> &SourceId {
        self.model.id()
    }

    fn schema(&self) -> &Schema {
        self.model.schema()
    }

    fn capabilities(&self) -> &SourceCapabilities {
        self.model.capabilities()
    }

    fn source_spec(&self) -> serde_json::Value {
        self.source_spec.clone()
    }

    async fn freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
        context.check()?;
        Ok(SourceFreshness {
            observed_at_ms: now_ms(),
            freshness_ms: 0,
        })
    }

    fn compile(
        &self,
        query: &EntityQuery,
        authorized_fields: &[String],
    ) -> Result<SourcePlan, SourceError> {
        self.model.compile(query, authorized_fields)
    }

    async fn execute_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        context.check()?;
        self.validate_plan(plan)?;
        let page_size = context.page_size().min(context.remaining_rows());
        if page_size == 0 {
            return Err(SourceError::RowBudgetExceeded);
        }
        let page_token = cursor
            .map(|cursor| self.decode_query_cursor(plan, cursor, context.query_id()))
            .transpose()?;
        let mut physical_bytes = 0usize;
        let response = self
            .send(
                self.request(self.list_path(page_token.as_deref(), page_size)),
                context,
                &mut physical_bytes,
            )
            .await?;
        if response.status == 400 && cursor.is_some() {
            return Err(SourceError::InvalidCursor);
        }
        if response.status != 200 {
            return Err(self.map_http_failure(&response));
        }
        let list: DriveListResponse =
            serde_json::from_slice(&response.body).map_err(|_| self.malformed("files"))?;
        if list.incomplete_search {
            return Err(SourceError::IncompleteSearch {
                source_id: self.model.id().clone(),
            });
        }
        if list.files.len() > page_size {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Drive page rows",
            });
        }
        let scanned = list.files.len();
        let needs_body = plan
            .projected
            .iter()
            .map(String::as_str)
            .chain(plan.residual_attrs())
            .any(|field| field == "body");
        let mut rows = Vec::with_capacity(scanned);
        for file in list.files {
            context.check()?;
            file.validate(self.model.id())?;
            if file.trashed {
                continue;
            }
            let mut value = file.to_value(self.model.id())?;
            if needs_body {
                if let Some(path) = self.body_path(&file) {
                    let body = self
                        .send(self.request(path), context, &mut physical_bytes)
                        .await?;
                    if body.status != 200 {
                        return Err(self.map_http_failure(&body));
                    }
                    let text = String::from_utf8(body.body)
                        .map_err(|_| self.malformed("body encoding"))?;
                    value
                        .as_object_mut()
                        .ok_or_else(|| self.malformed("file"))?
                        .insert("body".to_string(), serde_json::Value::String(text));
                }
            }
            if !plan.residual.iter().all(|residual| residual.passes(&value)) {
                continue;
            }
            let mut projected = value
                .as_object()
                .cloned()
                .ok_or_else(|| self.malformed("file"))?;
            projected.retain(|field, _| plan.projected.contains(field));
            rows.push(SourceRow {
                source: self.model.id().clone(),
                entity: self.model.entity().to_string(),
                key: file.id,
                value: serde_json::Value::Object(projected),
            });
        }
        let next_cursor = list
            .next_page_token
            .map(|token| self.encode_query_cursor(plan, context.query_id(), token))
            .transpose()?;
        if next_cursor.is_some() && scanned == 0 {
            return Err(SourceError::InvalidCursor);
        }
        Ok(SourcePage {
            source: self.model.id().clone(),
            scanned,
            rows,
            next_cursor,
            observed_at_ms: now_ms(),
            freshness_ms: 0,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveListResponse {
    #[serde(default)]
    files: Vec<DriveFile>,
    next_page_token: Option<String>,
    #[serde(default)]
    incomplete_search: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: String,
    name: String,
    mime_type: String,
    modified_time: String,
    size: Option<String>,
    md5_checksum: Option<String>,
    #[serde(default)]
    parents: Vec<String>,
    #[serde(default)]
    trashed: bool,
}

impl DriveFile {
    fn validate(&self, source_id: &SourceId) -> Result<(), SourceError> {
        validate_drive_id_source(source_id, "id", &self.id)?;
        if self.name.is_empty()
            || self.name.len() > 32 * 1024
            || self.name.chars().any(|character| character == '\0')
        {
            return Err(malformed_source(source_id, "name"));
        }
        if self.mime_type.is_empty() || self.mime_type.len() > 512 {
            return Err(malformed_source(source_id, "mimeType"));
        }
        parse_timestamp_ms(&self.modified_time)
            .map_err(|_| malformed_source(source_id, "modifiedTime"))?;
        if self.parents.len() > MAX_CONFIGURED_SELECTIONS {
            return Err(SourceError::ResourceLimit {
                source_id: source_id.clone(),
                resource: "Drive parents",
            });
        }
        for parent in &self.parents {
            validate_drive_id_source(source_id, "parent", parent)?;
        }
        if let Some(size) = &self.size {
            size.parse::<i64>()
                .ok()
                .filter(|size| *size >= 0)
                .ok_or_else(|| malformed_source(source_id, "size"))?;
        }
        if self
            .md5_checksum
            .as_ref()
            .is_some_and(|checksum| checksum.is_empty() || checksum.len() > 512)
        {
            return Err(malformed_source(source_id, "md5Checksum"));
        }
        Ok(())
    }

    fn to_value(&self, source_id: &SourceId) -> Result<serde_json::Value, SourceError> {
        let mut value = serde_json::Map::new();
        value.insert("id".to_string(), self.id.clone().into());
        value.insert("name".to_string(), self.name.clone().into());
        value.insert("mimeType".to_string(), self.mime_type.clone().into());
        value.insert(
            "modifiedAt".to_string(),
            parse_timestamp_ms(&self.modified_time)
                .map_err(|_| malformed_source(source_id, "modifiedTime"))?
                .into(),
        );
        if let Some(size) = &self.size {
            value.insert(
                "size".to_string(),
                size.parse::<i64>()
                    .map_err(|_| malformed_source(source_id, "size"))?
                    .into(),
            );
        }
        if let Some(checksum) = &self.md5_checksum {
            value.insert("checksum".to_string(), checksum.clone().into());
        }
        Ok(serde_json::Value::Object(value))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveStartPageTokenResponse {
    start_page_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveChangesResponse {
    #[serde(default)]
    changes: Vec<DriveChange>,
    next_page_token: Option<String>,
    new_start_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveChange {
    change_type: String,
    #[serde(default)]
    removed: bool,
    file_id: String,
    file: Option<DriveFile>,
}

/// Connector configuration or construction failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DriveConnectorError {
    /// One immutable configuration field is invalid.
    #[error("invalid Google Drive configuration field `{field}`")]
    InvalidConfiguration {
        /// Invalid field.
        field: &'static str,
    },
    /// Runtime token/key material is invalid.
    #[error("invalid Google Drive runtime secret")]
    InvalidSecret,
    /// Native OAuth PKCE material is invalid.
    #[error("invalid Google Drive PKCE request")]
    InvalidPkce,
    /// Known revision material is malformed.
    #[error("invalid Google Drive revision")]
    InvalidRevision,
    /// Production HTTPS transport could not be constructed.
    #[error("Google Drive transport construction failed")]
    Transport,
}

fn validate_drive_id(field: &'static str, id: &str) -> Result<(), DriveConnectorError> {
    if id.is_empty()
        || id.len() > MAX_SELECTION_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DriveConnectorError::InvalidConfiguration { field });
    }
    Ok(())
}

fn validate_drive_id_source(
    source_id: &SourceId,
    attr: &'static str,
    id: &str,
) -> Result<(), SourceError> {
    if id.is_empty()
        || id.len() > MAX_SELECTION_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(malformed_source(source_id, attr));
    }
    Ok(())
}

fn validate_provider_token(token: &str) -> Result<(), SourceError> {
    if token.is_empty() || token.len() > 1_024 || token.chars().any(char::is_control) {
        return Err(SourceError::InvalidCursor);
    }
    Ok(())
}

fn digest<T: Serialize>(value: &T) -> Result<String, DriveConnectorError> {
    serde_json::to_vec(value)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .map_err(|_| DriveConnectorError::InvalidConfiguration {
            field: "source_spec",
        })
}

fn plan_digest(plan: &SourcePlan) -> Result<String, SourceError> {
    serde_json::to_vec(plan)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .map_err(|_| SourceError::InvalidCursor)
}

fn parse_timestamp_ms(value: &str) -> Result<i64, ()> {
    time::OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| ())?
        .unix_timestamp_nanos()
        .checked_div(1_000_000)
        .and_then(|millis| i64::try_from(millis).ok())
        .ok_or(())
}

fn encoded_path(path: &str, params: Vec<(&str, String)>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in params {
        serializer.append_pair(name, &value);
    }
    format!("{path}?{}", serializer.finish())
}

fn encode_path_segment(segment: &str) -> String {
    url::form_urlencoded::byte_serialize(segment.as_bytes()).collect()
}

fn escape_query_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn retry_after_ms(response: &DriveHttpResponse) -> Option<u64> {
    let value = response.headers.get("retry-after")?;
    if let Ok(seconds) = value.parse::<u64>() {
        return seconds.checked_mul(1_000);
    }
    let when = httpdate::parse_http_date(value).ok()?;
    let delay = when.duration_since(SystemTime::now()).unwrap_or_default();
    u64::try_from(delay.as_millis()).ok()
}

fn redacted_path(path: &str) -> String {
    Url::parse(&format!("{API_BASE}{path}"))
        .ok()
        .map(|mut url| {
            let pairs = url
                .query_pairs()
                .map(|(name, value)| {
                    if name == "pageToken" {
                        (name.into_owned(), "[redacted]".to_string())
                    } else {
                        (name.into_owned(), value.into_owned())
                    }
                })
                .collect::<Vec<_>>();
            url.set_query(None);
            url.query_pairs_mut().extend_pairs(pairs);
            format!(
                "{}{}",
                url.path(),
                url.query()
                    .map(|query| format!("?{query}"))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "[invalid-path]".to_string())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn malformed_source(source_id: &SourceId, attr: &'static str) -> SourceError {
    SourceError::Malformed {
        source_id: source_id.clone(),
        path: "google-drive-response".to_string(),
        attr: attr.to_string(),
        detail: "provider response did not match the pinned Drive API v3 contract".to_string(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}
