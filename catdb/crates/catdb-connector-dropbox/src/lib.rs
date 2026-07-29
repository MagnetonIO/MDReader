//! Dropbox API v2 source for a CatDB-backed MDReader Library.
//!
//! Dropbox remains authoritative for its files. This connector lists metadata,
//! reads Markdown or plain-text content, and exposes authenticated incremental
//! cursors. Its semantic [`DataSource`] surface remains read-only; provider-local
//! document operations are separately typed. OAuth material is resolved at
//! runtime and never becomes part of the immutable source specification.

#![deny(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use catdb_connectors::{
    DataSource, ReadContext, SourceCapabilities, SourceCursor, SourceError, SourceFreshness,
    SourceId, SourceModel, SourcePage, SourcePlan, SourceRow,
};
use catdb_core::{BaseType, Schema};
use catdb_query::EntityQuery;
use catdb_storage::{ObjectFilterOp, ObjectValue};
use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use time::format_description::well_known::Rfc3339;
use url::Url;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// Dropbox metadata permission required by V1.
pub const DROPBOX_METADATA_READ_SCOPE: &str = "files.metadata.read";
/// Dropbox content permission required by V1.
pub const DROPBOX_CONTENT_READ_SCOPE: &str = "files.content.read";
/// Dropbox content permission required for guarded Markdown updates.
pub const DROPBOX_CONTENT_WRITE_SCOPE: &str = "files.content.write";

const AUTHORIZATION_ENDPOINT: &str = "https://www.dropbox.com/oauth2/authorize";
const API_ORIGIN: &str = "https://api.dropboxapi.com/";
const CONTENT_ORIGIN: &str = "https://content.dropboxapi.com/";
const MAX_HTTP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MARKDOWN_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_HEADERS: usize = 128;
const MAX_PROVIDER_CURSOR_BYTES: usize = 4 * 1024;
const MAX_CHANGE_EVENTS: usize = 1_000;
const MAX_EMPTY_PROVIDER_PAGES: usize = 32;
const MAX_ACTIVE_QUERY_IDS: usize = 128;
const MAX_USED_QUERY_CURSORS: usize = 1_024;
const ACCESS_TOKEN_EXPIRY_SKEW: Duration = Duration::from_secs(30);
const CURSOR_VERSION: u8 = 1;
const QUERY_CURSOR_DOMAIN: &[u8] = b"catdb-dropbox-query-cursor\0";
const CHANGE_CURSOR_DOMAIN: &[u8] = b"catdb-dropbox-change-cursor\0";
const WRITE_RECONCILIATION_DOMAIN: &[u8] = b"catdb-dropbox-write-reconciliation\0";

/// Runtime secret lookup key stored in a non-secret source specification.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropboxSecretReference(String);

impl DropboxSecretReference {
    /// Validate a bounded secret reference.
    pub fn new(reference: impl Into<String>) -> Result<Self, DropboxConnectorError> {
        let reference = reference.into();
        if reference.is_empty() || reference.len() > 512 || contains_control(&reference) {
            return Err(DropboxConnectorError::InvalidConfiguration {
                field: "credential_reference",
            });
        }
        Ok(Self(reference))
    }
}

impl fmt::Debug for DropboxSecretReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DropboxSecretReference([redacted-reference])")
    }
}

/// Dropbox application authorization boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropboxAuthorizationMode {
    /// Application may read the account, with CatDB applying a selected-path filter.
    FullDropbox,
    /// Application is restricted by Dropbox to its application folder.
    AppFolder,
}

/// The physical records exposed by one immutable Dropbox source.
///
/// The general connector keeps its existing all-metadata contract by default.
/// Reader applications may pin a narrower readable-text or Markdown-only
/// universe as part of the versioned source configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DropboxContentScope {
    /// Expose every file as metadata while downloading only readable text.
    #[default]
    AllMetadata,
    /// Expose Markdown and plain-text files.
    ReadableText,
    /// Expose Markdown files only.
    Markdown,
}

/// The fixed OAuth capability requested by a native Dropbox authorization flow.
///
/// The default remains read-only so existing applications cannot accidentally
/// widen a consent request. Markdown editing must request the explicit V2
/// profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DropboxAuthorizationProfile {
    /// Read metadata and file content without mutation authority.
    #[default]
    ReadOnly,
    /// Read and guarded-update Markdown content.
    MarkdownReadWrite,
}

impl DropboxAuthorizationProfile {
    fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::ReadOnly => &[DROPBOX_METADATA_READ_SCOPE, DROPBOX_CONTENT_READ_SCOPE],
            Self::MarkdownReadWrite => &[
                DROPBOX_METADATA_READ_SCOPE,
                DROPBOX_CONTENT_READ_SCOPE,
                DROPBOX_CONTENT_WRITE_SCOPE,
            ],
        }
    }
}

/// Immutable, non-secret Dropbox source configuration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropboxSourceConfig {
    source_id: String,
    catdb_schema: String,
    catdb_entity: String,
    selected_path: String,
    authorization_mode: DropboxAuthorizationMode,
    credential_reference: DropboxSecretReference,
    #[serde(default)]
    content_scope: DropboxContentScope,
}

impl DropboxSourceConfig {
    /// Configure one Dropbox-backed semantic source.
    pub fn new(
        source_id: impl Into<String>,
        catdb_schema: impl Into<String>,
        catdb_entity: impl Into<String>,
        selected_path: impl Into<String>,
        authorization_mode: DropboxAuthorizationMode,
        credential_reference: DropboxSecretReference,
    ) -> Result<Self, DropboxConnectorError> {
        let config = Self {
            source_id: source_id.into(),
            catdb_schema: catdb_schema.into(),
            catdb_entity: catdb_entity.into(),
            selected_path: normalize_selected_path(selected_path.into())?,
            authorization_mode,
            credential_reference,
            content_scope: DropboxContentScope::default(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Pin the physical file universe exposed by this source.
    pub fn with_content_scope(mut self, scope: DropboxContentScope) -> Self {
        self.content_scope = scope;
        self
    }

    /// Exact OAuth scopes required by both Dropbox authorization modes.
    pub fn oauth_scopes(&self) -> [&'static str; 2] {
        [DROPBOX_METADATA_READ_SCOPE, DROPBOX_CONTENT_READ_SCOPE]
    }

    fn validate(&self) -> Result<(), DropboxConnectorError> {
        for (field, value) in [
            ("source_id", self.source_id.as_str()),
            ("catdb_schema", self.catdb_schema.as_str()),
            ("catdb_entity", self.catdb_entity.as_str()),
        ] {
            if value.is_empty() || value.len() > 256 || contains_control(value) {
                return Err(DropboxConnectorError::InvalidConfiguration { field });
            }
        }
        validate_selected_path(&self.selected_path)
    }
}

impl fmt::Debug for DropboxSourceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxSourceConfig")
            .field("source_id", &self.source_id)
            .field("catdb_schema", &self.catdb_schema)
            .field("catdb_entity", &self.catdb_entity)
            .field("selected_path", &self.selected_path)
            .field("authorization_mode", &self.authorization_mode)
            .field("credential_reference", &self.credential_reference)
            .field("content_scope", &self.content_scope)
            .finish()
    }
}

/// Source configuration pinned to one CatDB schema version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinnedDropboxSourceSpec {
    config: DropboxSourceConfig,
    schema_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPinnedDropboxSourceSpec {
    config: DropboxSourceConfig,
    schema_version: String,
}

impl<'de> Deserialize<'de> for PinnedDropboxSourceSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawPinnedDropboxSourceSpec::deserialize(deserializer)?;
        Self::new(raw.config, raw.schema_version).map_err(serde::de::Error::custom)
    }
}

impl PinnedDropboxSourceSpec {
    /// Bind a source configuration to an immutable semantic schema identity.
    pub fn new(
        config: DropboxSourceConfig,
        schema_version: impl Into<String>,
    ) -> Result<Self, DropboxConnectorError> {
        let schema_version = schema_version.into();
        if schema_version.is_empty()
            || schema_version.len() > 512
            || contains_control(&schema_version)
        {
            return Err(DropboxConnectorError::InvalidConfiguration {
                field: "schema_version",
            });
        }
        config.validate()?;
        Ok(Self {
            config,
            schema_version,
        })
    }
}

/// Connector construction or credential failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DropboxConnectorError {
    /// Immutable configuration was invalid.
    #[error("invalid Dropbox configuration field `{field}`")]
    InvalidConfiguration {
        /// Field that failed validation.
        field: &'static str,
    },
    /// Runtime secret material was absent or invalid.
    #[error("Dropbox runtime secret is invalid")]
    InvalidSecret,
    /// OAuth PKCE state was invalid.
    #[error("Dropbox PKCE state is invalid")]
    InvalidPkce,
    /// Known revision material was invalid.
    #[error("Dropbox revision is invalid")]
    InvalidRevision,
    /// Provider or TLS transport failed.
    #[error("Dropbox transport failed")]
    Transport,
    /// Provider rejected credential refresh due to rate limiting.
    #[error("Dropbox credential refresh was rate limited")]
    RateLimited {
        /// Provider-requested delay when supplied.
        retry_after_ms: Option<u64>,
    },
}

/// Runtime-only OAuth access token.
pub struct DropboxAccessToken(Zeroizing<String>);

impl DropboxAccessToken {
    /// Validate a non-empty bounded access token.
    pub fn new(token: impl Into<String>) -> Result<Self, DropboxConnectorError> {
        let token = token.into();
        if token.is_empty() || token.len() > 16 * 1024 || contains_control(&token) {
            return Err(DropboxConnectorError::InvalidSecret);
        }
        Ok(Self(Zeroizing::new(token)))
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for DropboxAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DropboxAccessToken([redacted])")
    }
}

/// Runtime access token plus its provider expiry.
pub struct DropboxAccessGrant {
    token: DropboxAccessToken,
    expires_at: Option<SystemTime>,
}

impl DropboxAccessGrant {
    /// Construct an access grant.
    pub fn new(
        token: DropboxAccessToken,
        expires_at: Option<SystemTime>,
    ) -> Result<Self, DropboxConnectorError> {
        if expires_at.is_some_and(|expiry| expiry <= UNIX_EPOCH) {
            return Err(DropboxConnectorError::InvalidSecret);
        }
        Ok(Self { token, expires_at })
    }

    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expiry| SystemTime::now() + ACCESS_TOKEN_EXPIRY_SKEW >= expiry)
    }
}

impl fmt::Debug for DropboxAccessGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DropboxAccessGrant([redacted])")
    }
}

/// Runtime-only OAuth refresh credential.
pub struct DropboxRefreshCredential(Zeroizing<String>);

impl DropboxRefreshCredential {
    /// Validate a non-empty bounded refresh credential.
    pub fn new(credential: impl Into<String>) -> Result<Self, DropboxConnectorError> {
        let credential = credential.into();
        if credential.is_empty() || credential.len() > 16 * 1024 || contains_control(&credential) {
            return Err(DropboxConnectorError::InvalidSecret);
        }
        Ok(Self(Zeroizing::new(credential)))
    }

    /// Borrow the credential only inside a resolver's HTTPS token exchange.
    ///
    /// Callers must not serialize, persist, or log this value.
    pub fn expose_for_exchange(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for DropboxRefreshCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DropboxRefreshCredential([redacted])")
    }
}

/// Stable 256-bit key authenticating opaque connector cursors.
pub struct DropboxCursorKey([u8; 32]);

impl DropboxCursorKey {
    /// Construct a cursor-authentication key.
    pub fn new(key: [u8; 32]) -> Self {
        Self(key)
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for DropboxCursorKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for DropboxCursorKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DropboxCursorKey([redacted])")
    }
}

/// Runtime-only Dropbox credentials and cursor key.
pub struct DropboxRuntimeSecrets {
    grant: DropboxAccessGrant,
    refresh_credential: Option<DropboxRefreshCredential>,
    cursor_key: DropboxCursorKey,
}

impl DropboxRuntimeSecrets {
    /// Bind a non-expiring access token to a cursor key.
    pub fn new(token: DropboxAccessToken, cursor_key: DropboxCursorKey) -> Self {
        Self {
            grant: DropboxAccessGrant {
                token,
                expires_at: None,
            },
            refresh_credential: None,
            cursor_key,
        }
    }

    /// Bind refreshable OAuth credentials to a cursor key.
    pub fn refreshable(
        grant: DropboxAccessGrant,
        refresh_credential: DropboxRefreshCredential,
        cursor_key: DropboxCursorKey,
    ) -> Self {
        Self {
            grant,
            refresh_credential: Some(refresh_credential),
            cursor_key,
        }
    }

    fn into_parts(
        self,
    ) -> (
        DropboxAccessGrant,
        Option<DropboxRefreshCredential>,
        DropboxCursorKey,
    ) {
        (self.grant, self.refresh_credential, self.cursor_key)
    }
}

impl fmt::Debug for DropboxRuntimeSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DropboxRuntimeSecrets([redacted])")
    }
}

/// Application-owned runtime secret resolver.
#[async_trait]
pub trait DropboxSecretResolver: Send + Sync {
    /// Resolve runtime-only access and cursor material.
    async fn resolve(
        &self,
        reference: &DropboxSecretReference,
    ) -> Result<DropboxRuntimeSecrets, DropboxConnectorError>;

    /// Refresh an expiring access grant.
    async fn refresh(
        &self,
        _reference: &DropboxSecretReference,
        _refresh: &DropboxRefreshCredential,
    ) -> Result<DropboxAccessGrant, DropboxConnectorError> {
        Err(DropboxConnectorError::InvalidSecret)
    }
}

/// OAuth Authorization Code with PKCE state for one native login attempt.
pub struct DropboxOAuthPkce {
    verifier: Zeroizing<String>,
    state: String,
    redirect_uri: String,
    challenge: String,
}

impl DropboxOAuthPkce {
    /// Validate RFC 7636 inputs and derive the S256 challenge.
    pub fn new(
        verifier: impl Into<String>,
        state: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Result<Self, DropboxConnectorError> {
        let verifier = verifier.into();
        let state = state.into();
        let redirect_uri = redirect_uri.into();
        if !(43..=128).contains(&verifier.len())
            || !verifier.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
            })
            || state.is_empty()
            || state.len() > 512
            || contains_control(&state)
        {
            return Err(DropboxConnectorError::InvalidPkce);
        }
        let redirect = Url::parse(&redirect_uri).map_err(|_| DropboxConnectorError::InvalidPkce)?;
        let valid_redirect = redirect.scheme() == "https"
            || (redirect.scheme() == "http"
                && matches!(redirect.host_str(), Some("127.0.0.1" | "::1" | "localhost")));
        if !valid_redirect {
            return Err(DropboxConnectorError::InvalidPkce);
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

    /// Derived URL-safe S256 challenge.
    pub fn code_challenge(&self) -> &str {
        &self.challenge
    }

    /// Construct the Dropbox authorization URL without a client secret.
    pub fn authorization_url(&self, client_id: &str) -> Result<String, DropboxConnectorError> {
        self.authorization_url_for_profile(client_id, DropboxAuthorizationProfile::ReadOnly)
    }

    /// Construct the Dropbox authorization URL for one fixed capability profile.
    pub fn authorization_url_for_profile(
        &self,
        client_id: &str,
        profile: DropboxAuthorizationProfile,
    ) -> Result<String, DropboxConnectorError> {
        if client_id.is_empty() || client_id.len() > 512 || contains_control(client_id) {
            return Err(DropboxConnectorError::InvalidPkce);
        }
        let mut url =
            Url::parse(AUTHORIZATION_ENDPOINT).map_err(|_| DropboxConnectorError::InvalidPkce)?;
        url.query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &profile.scopes().join(" "))
            .append_pair("code_challenge", &self.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("token_access_type", "offline")
            .append_pair("state", &self.state);
        Ok(url.into())
    }

    /// Verify a callback state in constant time.
    pub fn verify_state(&self, callback_state: &str) -> bool {
        constant_time_equal(self.state.as_bytes(), callback_state.as_bytes())
    }

    /// Runtime-only verifier for the application-owned token exchange.
    pub fn verifier(&self) -> &str {
        self.verifier.as_str()
    }
}

impl fmt::Debug for DropboxOAuthPkce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxOAuthPkce")
            .field("verifier", &"[redacted]")
            .field("state", &"[redacted]")
            .field("redirect_uri", &self.redirect_uri)
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// Dropbox API host selected for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropboxEndpoint {
    /// Metadata API host.
    Api,
    /// File-content API host.
    Content,
}

/// HTTP method emitted by this connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropboxHttpMethod {
    /// Dropbox API v2 RPC request.
    Post,
}

/// Bounded authenticated Dropbox request.
#[derive(Clone)]
pub struct DropboxHttpRequest {
    endpoint: DropboxEndpoint,
    method: DropboxHttpMethod,
    path: String,
    json_body: serde_json::Value,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    bearer_token: Arc<str>,
    credential_generation: u64,
}

impl DropboxHttpRequest {
    /// Request host family.
    pub fn endpoint(&self) -> DropboxEndpoint {
        self.endpoint
    }

    /// Request method, always POST in V1.
    pub fn method(&self) -> DropboxHttpMethod {
        self.method
    }

    /// Provider-relative RPC path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Structured provider request body.
    pub fn json_body(&self) -> &serde_json::Value {
        &self.json_body
    }

    /// Read a non-secret provider header case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Bounded document bytes supplied to the injected transport.
    pub fn body(&self) -> &[u8] {
        self.body.as_slice()
    }

    /// Credential generation used for bounded refresh diagnostics.
    pub fn credential_generation(&self) -> u64 {
        self.credential_generation
    }
}

impl fmt::Debug for DropboxHttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxHttpRequest")
            .field("endpoint", &self.endpoint)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("body", &"[redacted-provider-arguments]")
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("body_bytes", &self.body.len())
            .field("bearer_token", &"[redacted]")
            .field("credential_generation", &self.credential_generation)
            .finish()
    }
}

/// Bounded Dropbox HTTP response.
#[derive(Clone, PartialEq, Eq)]
pub struct DropboxHttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl DropboxHttpResponse {
    /// Validate provider response metadata and the fixed body bound.
    pub fn new(
        status: u16,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<Self, DropboxTransportError> {
        if !(100..=599).contains(&status) {
            return Err(DropboxTransportError::InvalidResponse);
        }
        if body.len() > MAX_HTTP_RESPONSE_BYTES {
            return Err(DropboxTransportError::ResponseTooLarge);
        }
        if headers.len() > MAX_HTTP_HEADERS {
            return Err(DropboxTransportError::InvalidResponse);
        }
        let mut header_bytes = 0usize;
        for (name, value) in &headers {
            if name.is_empty()
                || name.len() > 256
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || contains_control(value)
            {
                return Err(DropboxTransportError::InvalidResponse);
            }
            header_bytes = header_bytes
                .checked_add(name.len())
                .and_then(|bytes| bytes.checked_add(value.len()))
                .ok_or(DropboxTransportError::InvalidResponse)?;
            if header_bytes > MAX_HTTP_HEADER_BYTES {
                return Err(DropboxTransportError::InvalidResponse);
            }
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

impl fmt::Debug for DropboxHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxHttpResponse")
            .field("status", &self.status)
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Failure beneath the semantic source boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DropboxTransportError {
    /// Network or TLS failed.
    #[error("Dropbox transport failed")]
    Failed,
    /// Response exceeded the fixed connector bound.
    #[error("Dropbox response exceeded the fixed body bound")]
    ResponseTooLarge,
    /// Response metadata was invalid.
    #[error("Dropbox response metadata was invalid")]
    InvalidResponse,
}

/// Injected transport for protocol fixtures and the production HTTPS client.
#[async_trait]
pub trait DropboxTransport: Send + Sync {
    /// Send one bounded read request.
    async fn send(
        &self,
        request: DropboxHttpRequest,
    ) -> Result<DropboxHttpResponse, DropboxTransportError>;
}

/// Rustls-backed Dropbox transport with redirects disabled.
pub struct ReqwestDropboxTransport {
    client: reqwest::Client,
    api_origin: Url,
    content_origin: Url,
}

impl ReqwestDropboxTransport {
    /// Build a transport pinned to the two Dropbox HTTPS origins.
    pub fn new() -> Result<Self, DropboxConnectorError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| DropboxConnectorError::Transport)?;
        Ok(Self {
            client,
            api_origin: Url::parse(API_ORIGIN).map_err(|_| DropboxConnectorError::Transport)?,
            content_origin: Url::parse(CONTENT_ORIGIN)
                .map_err(|_| DropboxConnectorError::Transport)?,
        })
    }

    /// Exact metadata API origin.
    pub fn api_origin(&self) -> &Url {
        &self.api_origin
    }

    /// Exact content API origin.
    pub fn content_origin(&self) -> &Url {
        &self.content_origin
    }

    /// Redirects are never followed across an authorization boundary.
    pub fn follows_redirects(&self) -> bool {
        false
    }

    fn build_http_request(
        &self,
        request: &DropboxHttpRequest,
    ) -> Result<reqwest::Request, DropboxTransportError> {
        let base = match request.endpoint {
            DropboxEndpoint::Api => &self.api_origin,
            DropboxEndpoint::Content => &self.content_origin,
        };
        if !allowed_request(request.endpoint, &request.path) {
            return Err(DropboxTransportError::InvalidResponse);
        }
        let url = base
            .join(request.path.trim_start_matches('/'))
            .map_err(|_| DropboxTransportError::InvalidResponse)?;
        if url.scheme() != "https" || url.origin() != base.origin() {
            return Err(DropboxTransportError::InvalidResponse);
        }
        let mut builder = self
            .client
            .post(url)
            .bearer_auth(request.bearer_token.as_ref());
        match request.endpoint {
            DropboxEndpoint::Api => {
                builder = builder.header("content-type", "application/json").body(
                    serde_json::to_vec(&request.json_body)
                        .map_err(|_| DropboxTransportError::InvalidResponse)?,
                );
            }
            DropboxEndpoint::Content => {
                for (name, value) in &request.headers {
                    builder = builder.header(name, value);
                }
                if request.path == "/2/files/upload" {
                    builder = builder.header("content-type", "application/octet-stream");
                }
                builder = builder.body(request.body.clone());
            }
        }
        builder
            .build()
            .map_err(|_| DropboxTransportError::InvalidResponse)
    }
}

#[async_trait]
impl DropboxTransport for ReqwestDropboxTransport {
    async fn send(
        &self,
        request: DropboxHttpRequest,
    ) -> Result<DropboxHttpResponse, DropboxTransportError> {
        let response = self
            .client
            .execute(self.build_http_request(&request)?)
            .await
            .map_err(|_| DropboxTransportError::Failed)?;
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
            let chunk = chunk.map_err(|_| DropboxTransportError::Failed)?;
            let next = body
                .len()
                .checked_add(chunk.len())
                .ok_or(DropboxTransportError::ResponseTooLarge)?;
            if next > MAX_HTTP_RESPONSE_BYTES {
                return Err(DropboxTransportError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        DropboxHttpResponse::new(status, headers, body)
    }
}

/// Evidence level for this connector implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropboxAvailability {
    /// Protocol fixtures passed; no live Dropbox account is claimed.
    FixtureValidated,
}

/// Opaque authenticated Dropbox changes position.
#[derive(Clone, PartialEq, Eq)]
pub struct DropboxChangeCursor(Vec<u8>);

impl DropboxChangeCursor {
    /// Restore bounded opaque change state from application-managed storage.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, SourceError> {
        if bytes.len() > catdb_connectors::MAX_SOURCE_CURSOR_BYTES {
            return Err(SourceError::CursorTooLarge {
                actual: bytes.len(),
                maximum: catdb_connectors::MAX_SOURCE_CURSOR_BYTES,
            });
        }
        Ok(Self(bytes))
    }

    /// Opaque authenticated bytes for application-managed persistence.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for DropboxChangeCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DropboxChangeCursor([opaque])")
    }
}

/// Provider revision material used to classify an incremental change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropboxKnownRevision {
    rev: String,
    content_hash: String,
}

impl DropboxKnownRevision {
    /// Validate a known Dropbox file revision.
    pub fn new(
        rev: impl Into<String>,
        content_hash: impl Into<String>,
    ) -> Result<Self, DropboxConnectorError> {
        let rev = rev.into();
        let content_hash = content_hash.into();
        if !valid_provider_atom(&rev, 512) || !valid_provider_atom(&content_hash, 512) {
            return Err(DropboxConnectorError::InvalidRevision);
        }
        Ok(Self { rev, content_hash })
    }

    fn matches(&self, file: &DropboxFile) -> bool {
        self.rev == file.rev && file.content_hash.as_deref() == Some(self.content_hash.as_str())
    }
}

/// Opaque provider revision used to guard a single Dropbox document operation.
#[derive(Clone, PartialEq, Eq)]
pub struct DropboxRevision(String);

impl DropboxRevision {
    /// Validate provider revision authority before it is used for a document operation.
    pub fn new(revision: impl Into<String>) -> Result<Self, DropboxConnectorError> {
        let revision = revision.into();
        if !valid_provider_atom(&revision, 512) {
            return Err(DropboxConnectorError::InvalidRevision);
        }
        Ok(Self(revision))
    }

    /// Return the opaque provider revision for an immediate guarded update.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for DropboxRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxRevision")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// Safe identity for reconciling a Dropbox mutation whose provider outcome is unknown.
#[derive(Clone, PartialEq, Eq)]
pub struct DropboxWriteReconciliation {
    source_id: SourceId,
    record_fingerprint: String,
}

impl DropboxWriteReconciliation {
    fn new(source_id: SourceId, record_fingerprint: String) -> Self {
        Self {
            source_id,
            record_fingerprint,
        }
    }

    /// Source that owns the uncertain mutation.
    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    /// Stable non-secret fingerprint of the provider record to reconcile.
    pub fn record_fingerprint(&self) -> &str {
        self.record_fingerprint.as_str()
    }
}

impl fmt::Debug for DropboxWriteReconciliation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxWriteReconciliation")
            .field("source_id", &self.source_id)
            .field("record_fingerprint_bytes", &self.record_fingerprint.len())
            .finish()
    }
}

/// A provider-local document operation could not complete.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum DropboxDocumentError {
    /// The requested provider record no longer exists.
    #[error("Dropbox document was not found")]
    NotFound {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider rejected a guarded update because its revision changed.
    #[error("Dropbox document changed before the guarded update for source `{source_id}`")]
    Conflict {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The document exceeded a fixed document-operation bound.
    #[error("Dropbox document exceeded a fixed bound")]
    TooLarge {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider returned document bytes that were not valid UTF-8.
    #[error("Dropbox document body was not valid UTF-8")]
    InvalidUtf8 {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider returned a record other than the requested record.
    #[error("Dropbox document identity did not match the request")]
    IdentityMismatch {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider response did not meet the pinned document protocol.
    #[error("Dropbox document response did not meet the pinned protocol")]
    Protocol {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider rejected the runtime credential.
    #[error("Dropbox document authentication failed")]
    Authentication {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The document operation exceeded its deadline.
    #[error("Dropbox document operation timed out")]
    Timeout,
    /// The document operation was cooperatively cancelled.
    #[error("Dropbox document operation was cancelled")]
    Cancelled,
    /// The provider rate-limited the document operation.
    #[error("Dropbox document operation was rate limited")]
    RateLimited {
        /// Non-secret source identity.
        source_id: SourceId,
        /// Server-requested wait, when supplied.
        retry_after_ms: Option<u64>,
    },
    /// The guarded mutation may have been accepted, but no usable outcome was received.
    #[error("Dropbox write outcome is unknown; reconcile the returned identity")]
    WriteOutcomeUnknown {
        /// Non-secret identity for a provider read/reconciliation pass.
        reconciliation: DropboxWriteReconciliation,
    },
    /// A source failure that has no more-specific document-operation outcome.
    #[error("Dropbox document operation failed")]
    Source {
        /// Typed source failure preserved without exposing it through `Debug`.
        #[source]
        error: SourceError,
    },
}

impl From<SourceError> for DropboxDocumentError {
    fn from(error: SourceError) -> Self {
        match error {
            SourceError::Cancelled | SourceError::SourceCancelled { .. } => Self::Cancelled,
            SourceError::DeadlineExceeded | SourceError::StatementTimeout { .. } => Self::Timeout,
            SourceError::Authentication { source_id } => Self::Authentication { source_id },
            SourceError::RateLimited {
                source_id,
                retry_after_ms,
            } => Self::RateLimited {
                source_id,
                retry_after_ms,
            },
            SourceError::ResourceLimit { source_id, .. } => Self::TooLarge { source_id },
            SourceError::Malformed { source_id, .. } => Self::Protocol { source_id },
            error => Self::Source { error },
        }
    }
}

impl fmt::Debug for DropboxDocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { source_id } => formatter
                .debug_struct("DropboxDocumentError::NotFound")
                .field("source_id", source_id)
                .finish(),
            Self::Conflict { source_id } => formatter
                .debug_struct("DropboxDocumentError::Conflict")
                .field("source_id", source_id)
                .finish(),
            Self::TooLarge { source_id } => formatter
                .debug_struct("DropboxDocumentError::TooLarge")
                .field("source_id", source_id)
                .finish(),
            Self::InvalidUtf8 { source_id } => formatter
                .debug_struct("DropboxDocumentError::InvalidUtf8")
                .field("source_id", source_id)
                .finish(),
            Self::IdentityMismatch { source_id } => formatter
                .debug_struct("DropboxDocumentError::IdentityMismatch")
                .field("source_id", source_id)
                .finish(),
            Self::Protocol { source_id } => formatter
                .debug_struct("DropboxDocumentError::Protocol")
                .field("source_id", source_id)
                .finish(),
            Self::Authentication { source_id } => formatter
                .debug_struct("DropboxDocumentError::Authentication")
                .field("source_id", source_id)
                .finish(),
            Self::Timeout => formatter.write_str("DropboxDocumentError::Timeout"),
            Self::Cancelled => formatter.write_str("DropboxDocumentError::Cancelled"),
            Self::RateLimited {
                source_id,
                retry_after_ms,
            } => formatter
                .debug_struct("DropboxDocumentError::RateLimited")
                .field("source_id", source_id)
                .field("retry_after_ms", retry_after_ms)
                .finish(),
            Self::WriteOutcomeUnknown { reconciliation } => formatter
                .debug_struct("DropboxDocumentError::WriteOutcomeUnknown")
                .field("reconciliation", reconciliation)
                .finish(),
            Self::Source { .. } => formatter.write_str("DropboxDocumentError::Source([redacted])"),
        }
    }
}

/// One bounded Markdown document downloaded from Dropbox.
#[derive(Clone, PartialEq, Eq)]
pub struct DropboxDownloadedDocument {
    record_key: String,
    revision: DropboxRevision,
    body: Vec<u8>,
}

impl DropboxDownloadedDocument {
    /// Stable provider record key requested for this document.
    pub fn record_key(&self) -> &str {
        self.record_key.as_str()
    }

    /// Provider revision authority captured with the downloaded body.
    pub fn revision(&self) -> &DropboxRevision {
        &self.revision
    }

    /// Validated UTF-8 Markdown bytes.
    pub fn body(&self) -> &[u8] {
        self.body.as_slice()
    }
}

impl fmt::Debug for DropboxDownloadedDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxDownloadedDocument")
            .field("record_key_bytes", &self.record_key.len())
            .field("revision_bytes", &self.revision.0.len())
            .field("body_bytes", &self.body.len())
            .field("body_encoding", &"utf-8")
            .finish()
    }
}

/// Receipt for one guarded Dropbox Markdown update.
#[derive(Clone, PartialEq, Eq)]
pub struct DropboxWriteReceipt {
    record_key: String,
    revision: DropboxRevision,
}

impl DropboxWriteReceipt {
    /// Stable provider record key updated by this operation.
    pub fn record_key(&self) -> &str {
        self.record_key.as_str()
    }

    /// Provider revision authority returned by the update.
    pub fn revision(&self) -> &DropboxRevision {
        &self.revision
    }
}

impl fmt::Debug for DropboxWriteReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxWriteReceipt")
            .field("record_key_bytes", &self.record_key.len())
            .field("revision_bytes", &self.revision.0.len())
            .field("provider_response", &"file metadata")
            .finish()
    }
}

/// One Dropbox incremental-change classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropboxChangeEvent {
    /// Provider revision is unchanged.
    Unchanged {
        /// Provider-normalized path.
        path_lower: String,
    },
    /// File is new or changed.
    Upserted {
        /// Provider-normalized path.
        path_lower: String,
        /// Dropbox revision.
        rev: String,
        /// Dropbox content hash.
        content_hash: String,
    },
    /// Known file was removed.
    Deleted {
        /// Provider-normalized path.
        path_lower: String,
    },
}

/// Explicit incremental-refresh outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropboxChangeOutcome {
    /// One complete provider page.
    Page {
        /// Classified changes.
        events: Vec<DropboxChangeEvent>,
        /// Continuation while the same refresh has more pages.
        next_cursor: Option<DropboxChangeCursor>,
        /// Durable checkpoint present only after provider exhaustion.
        checkpoint: Option<DropboxChangeCursor>,
    },
    /// Provider expired the cursor; callers must rebuild managed state.
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
    provider_cursor: String,
}

#[derive(Serialize, Deserialize)]
struct ChangeCursorPayload {
    version: u8,
    connector: String,
    source_spec_digest: String,
    schema_version: String,
    provider_cursor: String,
}

#[derive(Serialize, Deserialize)]
struct CursorEnvelope<T> {
    payload: T,
    authentication_tag: Vec<u8>,
}

struct StoredAccessGrant {
    grant: DropboxAccessGrant,
    generation: u64,
}

struct DropboxCredentialState {
    access: RwLock<StoredAccessGrant>,
    refresh_credential: Option<DropboxRefreshCredential>,
    resolver: Option<Arc<dyn DropboxSecretResolver>>,
    refresh_lock: tokio::sync::Mutex<()>,
}

struct QueryReplayState {
    expires_at: Instant,
    used: BTreeSet<[u8; 32]>,
}

/// Read-only Dropbox semantic source.
pub struct DropboxSource {
    spec: PinnedDropboxSourceSpec,
    model: SourceModel,
    source_spec: serde_json::Value,
    source_spec_digest: String,
    credentials: DropboxCredentialState,
    cursor_key: DropboxCursorKey,
    transport: Arc<dyn DropboxTransport>,
    query_replays: Mutex<BTreeMap<Uuid, QueryReplayState>>,
}

impl fmt::Debug for DropboxSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DropboxSource")
            .field("source_id", self.model.id())
            .field("schema_version", &self.spec.schema_version)
            .field("authorization_mode", &self.spec.config.authorization_mode)
            .field("runtime_secrets", &"[redacted]")
            .finish()
    }
}

impl DropboxSource {
    fn attributes() -> Vec<catdb_connectors::SourceAttr> {
        vec![
            catdb_connectors::SourceAttr::text("id"),
            catdb_connectors::SourceAttr::text("name"),
            catdb_connectors::SourceAttr::text("path").pushes([ObjectFilterOp::Eq]),
            catdb_connectors::SourceAttr::parsed("modifiedAt", BaseType::Timestamp),
            catdb_connectors::SourceAttr::parsed("size", BaseType::Int),
            catdb_connectors::SourceAttr::text("rev"),
            catdb_connectors::SourceAttr::text("contentHash"),
            catdb_connectors::SourceAttr::text("body"),
        ]
    }

    /// Return the connector-owned schema declaration without resolving or
    /// exposing runtime credentials.
    pub fn declared_model(
        spec: &PinnedDropboxSourceSpec,
    ) -> Result<SourceModel, DropboxConnectorError> {
        spec.config.validate()?;
        Ok(SourceModel::new(
            SourceId::new(spec.config.source_id.clone()),
            &spec.config.catdb_schema,
            &spec.config.catdb_entity,
            Self::attributes(),
        ))
    }

    /// Resolve runtime credentials and open with the production HTTPS transport.
    pub async fn open_pinned(
        spec: PinnedDropboxSourceSpec,
        resolver: Arc<dyn DropboxSecretResolver>,
    ) -> Result<Self, DropboxConnectorError> {
        let secrets = resolver.resolve(&spec.config.credential_reference).await?;
        Self::construct(
            spec,
            secrets,
            Some(resolver),
            Arc::new(ReqwestDropboxTransport::new()?),
        )
    }

    /// Open with explicit runtime secrets and an injected transport.
    pub fn open_with_transport(
        spec: PinnedDropboxSourceSpec,
        secrets: DropboxRuntimeSecrets,
        transport: Arc<dyn DropboxTransport>,
    ) -> Result<Self, DropboxConnectorError> {
        Self::construct(spec, secrets, None, transport)
    }

    /// Open with explicit initial secrets, a refresh resolver, and an injected transport.
    pub fn open_with_resolver_and_transport(
        spec: PinnedDropboxSourceSpec,
        secrets: DropboxRuntimeSecrets,
        resolver: Arc<dyn DropboxSecretResolver>,
        transport: Arc<dyn DropboxTransport>,
    ) -> Result<Self, DropboxConnectorError> {
        Self::construct(spec, secrets, Some(resolver), transport)
    }

    fn construct(
        spec: PinnedDropboxSourceSpec,
        secrets: DropboxRuntimeSecrets,
        resolver: Option<Arc<dyn DropboxSecretResolver>>,
        transport: Arc<dyn DropboxTransport>,
    ) -> Result<Self, DropboxConnectorError> {
        let model = Self::declared_model(&spec)?;
        let source_spec = serde_json::to_value(&spec).map_err(|_| {
            DropboxConnectorError::InvalidConfiguration {
                field: "source_spec",
            }
        })?;
        let source_spec_digest = digest(&spec)?;
        let (grant, refresh_credential, cursor_key) = secrets.into_parts();
        Ok(Self {
            spec,
            model,
            source_spec,
            source_spec_digest,
            credentials: DropboxCredentialState {
                access: RwLock::new(StoredAccessGrant {
                    grant,
                    generation: 0,
                }),
                refresh_credential,
                resolver,
                refresh_lock: tokio::sync::Mutex::new(()),
            },
            cursor_key,
            transport,
            query_replays: Mutex::new(BTreeMap::new()),
        })
    }

    /// Fixture-only evidence; never a live-provider claim.
    pub fn availability_evidence(&self) -> DropboxAvailability {
        DropboxAvailability::FixtureValidated
    }

    /// Download one source-authorized Markdown document and its revision authority.
    pub async fn download_markdown(
        &self,
        record_key: &str,
        context: &ReadContext,
    ) -> Result<DropboxDownloadedDocument, DropboxDocumentError> {
        context.check()?;
        let mut physical_bytes = 0usize;
        let response = self
            .send(
                self.content_request_record(record_key)?,
                context,
                &mut physical_bytes,
            )
            .await?;
        if response.status != 200 {
            return Err(self.map_document_http_failure(&response));
        }
        let header = response
            .headers
            .get("dropbox-api-result")
            .ok_or_else(|| self.malformed("Dropbox-API-Result"))?;
        let file: DropboxFile =
            serde_json::from_str(header).map_err(|_| self.malformed("Dropbox-API-Result"))?;
        self.validate_file_scope(&file)?;
        if file.id != record_key {
            return Err(DropboxDocumentError::IdentityMismatch {
                source_id: self.model.id().clone(),
            });
        }
        if !is_exposed_path(DropboxContentScope::Markdown, &file.name) {
            return Err(self.malformed("document name").into());
        }
        let revision = DropboxRevision::new(file.rev.clone())
            .map_err(|_| self.malformed("document revision"))?;
        if response.body.len() > MAX_MARKDOWN_BODY_BYTES {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Dropbox Markdown document",
            }
            .into());
        }
        if std::str::from_utf8(&response.body).is_err() {
            return Err(DropboxDocumentError::InvalidUtf8 {
                source_id: self.model.id().clone(),
            });
        }
        Ok(DropboxDownloadedDocument {
            record_key: record_key.to_string(),
            revision,
            body: response.body,
        })
    }

    /// Update one Markdown document only when the provider revision still matches.
    pub async fn update_markdown(
        &self,
        record_key: &str,
        revision: &DropboxRevision,
        body: &[u8],
        context: &ReadContext,
    ) -> Result<DropboxWriteReceipt, DropboxDocumentError> {
        context.check()?;
        if body.len() > MAX_MARKDOWN_BODY_BYTES {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Dropbox Markdown document",
            }
            .into());
        }
        if std::str::from_utf8(body).is_err() {
            return Err(DropboxDocumentError::InvalidUtf8 {
                source_id: self.model.id().clone(),
            });
        }
        let mut physical_bytes = 0usize;
        let metadata = self
            .send(
                self.metadata_request_record(record_key)?,
                context,
                &mut physical_bytes,
            )
            .await?;
        if metadata.status != 200 {
            return Err(self.map_document_http_failure(&metadata));
        }
        let file: DropboxFile = serde_json::from_slice(&metadata.body)
            .map_err(|_| self.malformed("metadata result"))?;
        self.validate_file_scope(&file)?;
        if file.id != record_key {
            return Err(DropboxDocumentError::IdentityMismatch {
                source_id: self.model.id().clone(),
            });
        }
        if !is_exposed_path(DropboxContentScope::Markdown, &file.name) {
            return Err(self.malformed("document name").into());
        }
        let reconciliation = self.write_reconciliation(record_key)?;
        let response = self
            .send_guarded_document_update(
                self.content_update_request(&file.path_lower, revision, body)?,
                context,
                &mut physical_bytes,
                reconciliation.clone(),
            )
            .await?;
        if response.status == 409 {
            return Err(DropboxDocumentError::Conflict {
                source_id: self.model.id().clone(),
            });
        }
        if response.status != 200 {
            return Err(self.map_document_write_http_failure(&response, reconciliation));
        }
        let file: DropboxFile = serde_json::from_slice(&response.body)
            .map_err(|_| self.write_outcome_unknown(reconciliation.clone()))?;
        self.validate_file_scope(&file)
            .map_err(|_| self.write_outcome_unknown(reconciliation.clone()))?;
        if file.id != record_key {
            return Err(self.write_outcome_unknown(reconciliation));
        }
        if !is_exposed_path(DropboxContentScope::Markdown, &file.name) {
            return Err(self.write_outcome_unknown(reconciliation));
        }
        let revision = DropboxRevision::new(file.rev)
            .map_err(|_| self.write_outcome_unknown(reconciliation))?;
        Ok(DropboxWriteReceipt {
            record_key: record_key.to_string(),
            revision,
        })
    }

    /// Obtain the provider's current incremental-change cursor.
    pub async fn start_change_tracking(
        &self,
        context: &ReadContext,
    ) -> Result<DropboxChangeCursor, SourceError> {
        context.check()?;
        let mut physical_bytes = 0usize;
        let response = self
            .send(
                self.api_request(
                    "/2/files/list_folder/get_latest_cursor",
                    list_arguments(&self.spec.config.selected_path, context.page_size(), true),
                )?,
                context,
                &mut physical_bytes,
            )
            .await?;
        if response.status != 200 {
            return Err(self.map_http_failure(&response));
        }
        let start: DropboxStartCursor =
            serde_json::from_slice(&response.body).map_err(|_| self.malformed("cursor"))?;
        validate_provider_cursor(&start.cursor)?;
        self.encode_change_cursor(&start.cursor)
    }

    /// Read one incremental provider page without collapsing reset or deletion.
    pub async fn read_changes(
        &self,
        cursor: &DropboxChangeCursor,
        known: &BTreeMap<String, DropboxKnownRevision>,
        context: &ReadContext,
    ) -> Result<DropboxChangeOutcome, SourceError> {
        context.check()?;
        if known.len() > MAX_CHANGE_EVENTS {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Dropbox known revisions",
            });
        }
        let provider_cursor = self.decode_change_cursor(cursor)?;
        let mut physical_bytes = 0usize;
        let response = self
            .send(
                self.api_request(
                    "/2/files/list_folder/continue",
                    serde_json::json!({ "cursor": provider_cursor }),
                )?,
                context,
                &mut physical_bytes,
            )
            .await?;
        if response.status == 409 && is_reset_response(&response.body) {
            return Ok(DropboxChangeOutcome::RebuildRequired);
        }
        if response.status != 200 {
            return Err(self.map_http_failure(&response));
        }
        let page: DropboxListResponse =
            serde_json::from_slice(&response.body).map_err(|_| self.malformed("entries"))?;
        page.validate(self.model.id())?;
        if page.has_more && page.cursor == provider_cursor {
            return Err(SourceError::InvalidCursor);
        }
        let change_limit = context
            .page_size()
            .min(context.remaining_rows())
            .min(MAX_CHANGE_EVENTS);
        if page.entries.len() > change_limit {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Dropbox change events",
            });
        }
        let mut events = Vec::new();
        for entry in page.entries {
            match parse_entry(entry, self.model.id())? {
                DropboxEntry::File(file) => {
                    self.validate_file_scope(&file)?;
                    if !is_exposed_path(self.spec.config.content_scope, &file.name) {
                        if known.contains_key(&file.path_lower) {
                            events.push(DropboxChangeEvent::Deleted {
                                path_lower: file.path_lower,
                            });
                        }
                        continue;
                    }
                    let content_hash = file
                        .content_hash
                        .clone()
                        .ok_or_else(|| self.malformed("file content hash"))?;
                    let event = match known.get(&file.path_lower) {
                        Some(revision) if revision.matches(&file) => {
                            DropboxChangeEvent::Unchanged {
                                path_lower: file.path_lower,
                            }
                        }
                        _ => DropboxChangeEvent::Upserted {
                            path_lower: file.path_lower,
                            rev: file.rev,
                            content_hash,
                        },
                    };
                    events.push(event);
                }
                DropboxEntry::Deleted(deleted) => {
                    validate_deleted(&deleted, self.model.id())?;
                    self.validate_path_scope(&deleted.path_lower)?;
                    if known.contains_key(&deleted.path_lower) {
                        events.push(DropboxChangeEvent::Deleted {
                            path_lower: deleted.path_lower,
                        });
                    }
                }
                DropboxEntry::Folder(_) => {
                    return Ok(DropboxChangeOutcome::RebuildRequired);
                }
            }
        }
        let encoded = self.encode_change_cursor(&page.cursor)?;
        if page.has_more {
            Ok(DropboxChangeOutcome::Page {
                events,
                next_cursor: Some(encoded),
                checkpoint: None,
            })
        } else {
            Ok(DropboxChangeOutcome::Page {
                events,
                next_cursor: None,
                checkpoint: Some(encoded),
            })
        }
    }

    fn validate_plan(&self, plan: &SourcePlan) -> Result<(), SourceError> {
        self.model.admit(plan)?;
        if self.exact_path_filter(plan).is_err() {
            return Err(SourceError::InvalidPlan {
                source_id: self.model.id().clone(),
            });
        }
        Ok(())
    }

    fn exact_path_filter<'a>(&self, plan: &'a SourcePlan) -> Result<Option<&'a str>, ()> {
        match plan.pushed.filters.as_slice() {
            [] => Ok(None),
            [filter] if filter.attr == "path" && filter.op == ObjectFilterOp::Eq => {
                match &filter.value {
                    ObjectValue::Text(path) => Ok(Some(path.as_str())),
                    _ => Err(()),
                }
            }
            _ => Err(()),
        }
    }

    fn api_request(
        &self,
        path: &'static str,
        json_body: serde_json::Value,
    ) -> Result<DropboxHttpRequest, SourceError> {
        self.request(DropboxEndpoint::Api, path, json_body, BTreeMap::new())
    }

    fn content_request(&self, file: &DropboxFile) -> Result<DropboxHttpRequest, SourceError> {
        let argument = serde_json::to_string(&serde_json::json!({ "path": file.id }))
            .map_err(|_| self.malformed("download argument"))?;
        self.request(
            DropboxEndpoint::Content,
            "/2/files/download",
            serde_json::json!({}),
            BTreeMap::from([("dropbox-api-arg".to_string(), argument)]),
        )
    }

    fn content_request_path(&self, path: &str) -> Result<DropboxHttpRequest, SourceError> {
        self.validate_path_scope(path)?;
        let argument = serde_json::to_string(&serde_json::json!({ "path": path }))
            .map_err(|_| self.malformed("download argument"))?;
        self.request(
            DropboxEndpoint::Content,
            "/2/files/download",
            serde_json::json!({}),
            BTreeMap::from([("dropbox-api-arg".to_string(), argument)]),
        )
    }

    fn content_request_record(&self, record_key: &str) -> Result<DropboxHttpRequest, SourceError> {
        if !valid_provider_atom(record_key, 1_024) {
            return Err(self.malformed("document record key"));
        }
        let argument = serde_json::to_string(&serde_json::json!({ "path": record_key }))
            .map_err(|_| self.malformed("download argument"))?;
        self.request(
            DropboxEndpoint::Content,
            "/2/files/download",
            serde_json::json!({}),
            BTreeMap::from([("dropbox-api-arg".to_string(), argument)]),
        )
    }

    fn metadata_request_record(&self, record_key: &str) -> Result<DropboxHttpRequest, SourceError> {
        if !valid_provider_atom(record_key, 1_024) {
            return Err(self.malformed("document record key"));
        }
        self.api_request(
            "/2/files/get_metadata",
            serde_json::json!({
                "path": record_key,
                "include_deleted": false,
                "include_media_info": false,
                "include_has_explicit_shared_members": false,
            }),
        )
    }

    fn content_update_request(
        &self,
        selected_path: &str,
        revision: &DropboxRevision,
        body: &[u8],
    ) -> Result<DropboxHttpRequest, SourceError> {
        self.validate_path_scope(selected_path)?;
        let argument = serde_json::to_string(&serde_json::json!({
            "path": selected_path,
            "mode": { ".tag": "update", "update": revision.as_str() },
            "autorename": false,
            "mute": false,
            "strict_conflict": true,
        }))
        .map_err(|_| self.malformed("upload argument"))?;
        let mut request = self.request(
            DropboxEndpoint::Content,
            "/2/files/upload",
            serde_json::json!({}),
            BTreeMap::from([("dropbox-api-arg".to_string(), argument)]),
        )?;
        request.body = body.to_vec();
        Ok(request)
    }

    fn request(
        &self,
        endpoint: DropboxEndpoint,
        path: &'static str,
        json_body: serde_json::Value,
        headers: BTreeMap<String, String>,
    ) -> Result<DropboxHttpRequest, SourceError> {
        if !allowed_request(endpoint, path)
            || headers
                .iter()
                .any(|(name, value)| contains_control(name) || contains_control(value))
        {
            return Err(SourceError::Transport {
                source_id: self.model.id().clone(),
            });
        }
        let access = self
            .credentials
            .access
            .read()
            .map_err(|_| self.authentication())?;
        Ok(DropboxHttpRequest {
            endpoint,
            method: DropboxHttpMethod::Post,
            path: path.to_string(),
            json_body,
            headers,
            body: Vec::new(),
            bearer_token: Arc::from(access.grant.token.as_str()),
            credential_generation: access.generation,
        })
    }

    fn reauthorize_request(
        &self,
        request: &DropboxHttpRequest,
    ) -> Result<DropboxHttpRequest, SourceError> {
        let access = self
            .credentials
            .access
            .read()
            .map_err(|_| self.authentication())?;
        Ok(DropboxHttpRequest {
            endpoint: request.endpoint,
            method: request.method,
            path: request.path.clone(),
            json_body: request.json_body.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
            bearer_token: Arc::from(access.grant.token.as_str()),
            credential_generation: access.generation,
        })
    }

    async fn send(
        &self,
        mut request: DropboxHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
    ) -> Result<DropboxHttpResponse, SourceError> {
        if self.access_token_expired()? {
            self.refresh_credentials(request.credential_generation, false, context)
                .await?;
            request = self.reauthorize_request(&request)?;
        }
        let response = self
            .send_once(request.clone(), context, physical_bytes)
            .await?;
        if response.status != 401 {
            return Ok(response);
        }
        self.refresh_credentials(request.credential_generation, true, context)
            .await?;
        let retry = self.reauthorize_request(&request)?;
        self.send_once(retry, context, physical_bytes).await
    }

    async fn send_guarded_document_update(
        &self,
        mut request: DropboxHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
        reconciliation: DropboxWriteReconciliation,
    ) -> Result<DropboxHttpResponse, DropboxDocumentError> {
        context.check()?;
        if self.access_token_expired()? {
            self.refresh_credentials(request.credential_generation, false, context)
                .await?;
            request = self.reauthorize_request(&request)?;
        }
        self.dispatch_guarded_document_update(request, context, physical_bytes, reconciliation)
            .await
    }

    async fn dispatch_guarded_document_update(
        &self,
        request: DropboxHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
        reconciliation: DropboxWriteReconciliation,
    ) -> Result<DropboxHttpResponse, DropboxDocumentError> {
        // This is the final pre-dispatch boundary. From the first poll of the
        // transport future onward, an unavailable receipt is not proof that the
        // provider rejected the mutation and must never trigger a retry.
        context.check()?;
        let deadline = tokio::time::Instant::from_std(context.deadline());
        let result = tokio::select! {
            biased;
            _ = context.cancellation().cancelled() => {
                return Err(self.write_outcome_unknown(reconciliation));
            }
            result = tokio::time::timeout_at(deadline, self.transport.send(request)) => result,
        };
        let response = match result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) | Err(_) => return Err(self.write_outcome_unknown(reconciliation)),
        };
        *physical_bytes = match physical_bytes.checked_add(response.body.len()) {
            Some(bytes) if bytes <= context.remaining_bytes() => bytes,
            _ => return Err(self.write_outcome_unknown(reconciliation)),
        };
        if context.check().is_err() {
            return Err(self.write_outcome_unknown(reconciliation));
        }
        Ok(response)
    }

    async fn send_once(
        &self,
        request: DropboxHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
    ) -> Result<DropboxHttpResponse, SourceError> {
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
                DropboxTransportError::ResponseTooLarge => SourceError::ByteBudgetExceeded,
                DropboxTransportError::Failed | DropboxTransportError::InvalidResponse => {
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

    fn access_token_expired(&self) -> Result<bool, SourceError> {
        self.credentials
            .access
            .read()
            .map(|access| access.grant.is_expired())
            .map_err(|_| self.authentication())
    }

    async fn refresh_credentials(
        &self,
        observed_generation: u64,
        forced: bool,
        context: &ReadContext,
    ) -> Result<(), SourceError> {
        let resolver = self
            .credentials
            .resolver
            .as_ref()
            .ok_or_else(|| self.authentication())?;
        let refresh = self
            .credentials
            .refresh_credential
            .as_ref()
            .ok_or_else(|| self.authentication())?;
        let deadline = tokio::time::Instant::from_std(context.deadline());
        let guard = tokio::select! {
            biased;
            _ = context.cancellation().cancelled() => return Err(SourceError::Cancelled),
            result = tokio::time::timeout_at(deadline, self.credentials.refresh_lock.lock()) => {
                result.map_err(|_| SourceError::DeadlineExceeded)?
            }
        };
        {
            let access = self
                .credentials
                .access
                .read()
                .map_err(|_| self.authentication())?;
            if access.generation != observed_generation || (!forced && !access.grant.is_expired()) {
                drop(guard);
                return Ok(());
            }
        }
        let refreshed = tokio::select! {
            biased;
            _ = context.cancellation().cancelled() => return Err(SourceError::Cancelled),
            result = tokio::time::timeout_at(
                deadline,
                resolver.refresh(&self.spec.config.credential_reference, refresh),
            ) => {
                result
                    .map_err(|_| SourceError::DeadlineExceeded)?
                    .map_err(|error| self.map_refresh_failure(error))?
            }
        };
        if refreshed.is_expired() {
            return Err(self.authentication());
        }
        let mut access = self
            .credentials
            .access
            .write()
            .map_err(|_| self.authentication())?;
        access.generation = access
            .generation
            .checked_add(1)
            .ok_or_else(|| self.authentication())?;
        access.grant = refreshed;
        drop(guard);
        Ok(())
    }

    fn map_refresh_failure(&self, error: DropboxConnectorError) -> SourceError {
        match error {
            DropboxConnectorError::Transport => SourceError::Transport {
                source_id: self.model.id().clone(),
            },
            DropboxConnectorError::RateLimited { retry_after_ms } => SourceError::RateLimited {
                source_id: self.model.id().clone(),
                retry_after_ms,
            },
            DropboxConnectorError::InvalidConfiguration { .. }
            | DropboxConnectorError::InvalidSecret
            | DropboxConnectorError::InvalidPkce
            | DropboxConnectorError::InvalidRevision => self.authentication(),
        }
    }

    fn map_http_failure(&self, response: &DropboxHttpResponse) -> SourceError {
        match response.status {
            401 | 403 => self.authentication(),
            408 | 504 => SourceError::StatementTimeout {
                source_id: self.model.id().clone(),
            },
            429 => SourceError::RateLimited {
                source_id: self.model.id().clone(),
                retry_after_ms: retry_after_ms(response),
            },
            404 => SourceError::Read {
                source_id: self.model.id().clone(),
                path: "Dropbox source".to_string(),
                detail: "provider returned not found".to_string(),
            },
            _ => SourceError::Transport {
                source_id: self.model.id().clone(),
            },
        }
    }

    fn map_document_http_failure(&self, response: &DropboxHttpResponse) -> DropboxDocumentError {
        if response.status == 404 {
            return DropboxDocumentError::NotFound {
                source_id: self.model.id().clone(),
            };
        }
        self.map_http_failure(response).into()
    }

    fn map_document_write_http_failure(
        &self,
        response: &DropboxHttpResponse,
        reconciliation: DropboxWriteReconciliation,
    ) -> DropboxDocumentError {
        if matches!(response.status, 408 | 500..=599) {
            return self.write_outcome_unknown(reconciliation);
        }
        self.map_document_http_failure(response)
    }

    fn write_reconciliation(
        &self,
        record_key: &str,
    ) -> Result<DropboxWriteReconciliation, SourceError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.cursor_key.as_bytes()).map_err(|_| {
            SourceError::Authentication {
                source_id: self.model.id().clone(),
            }
        })?;
        mac.update(WRITE_RECONCILIATION_DOMAIN);
        mac.update(record_key.as_bytes());
        Ok(DropboxWriteReconciliation::new(
            self.model.id().clone(),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()),
        ))
    }

    fn write_outcome_unknown(
        &self,
        reconciliation: DropboxWriteReconciliation,
    ) -> DropboxDocumentError {
        DropboxDocumentError::WriteOutcomeUnknown { reconciliation }
    }

    fn authentication(&self) -> SourceError {
        SourceError::Authentication {
            source_id: self.model.id().clone(),
        }
    }

    fn validate_file_scope(&self, file: &DropboxFile) -> Result<(), SourceError> {
        file.validate(self.model.id())?;
        self.validate_path_scope(&file.path_lower)
    }

    fn validate_path_scope(&self, path_lower: &str) -> Result<(), SourceError> {
        let selected = self.spec.config.selected_path.to_ascii_lowercase();
        if selected.is_empty()
            || path_lower == selected
            || path_lower.starts_with(&format!("{selected}/"))
        {
            return Ok(());
        }
        Err(self.malformed("selected path"))
    }

    async fn observe_file(
        &self,
        file: DropboxFile,
        plan: &SourcePlan,
        context: &ReadContext,
        physical_bytes: &mut usize,
    ) -> Result<Option<SourceRow>, SourceError> {
        self.validate_file_scope(&file)?;
        if !is_exposed_path(self.spec.config.content_scope, &file.name) {
            return Ok(None);
        }
        let needs_body = plan
            .projected
            .iter()
            .map(String::as_str)
            .chain(plan.residual_attrs())
            .any(|field| field == "body");
        if needs_body && is_readable_text_path(&file.name) {
            let response = self
                .send(self.content_request(&file)?, context, physical_bytes)
                .await?;
            if response.status != 200 {
                return Err(self.map_http_failure(&response));
            }
            self.validate_download_result(&file, &response)?;
            let body =
                String::from_utf8(response.body).map_err(|_| self.malformed("body encoding"))?;
            return self.project_file(file, Some(body), plan);
        }
        self.project_file(file, None, plan)
    }

    fn project_file(
        &self,
        file: DropboxFile,
        body: Option<String>,
        plan: &SourcePlan,
    ) -> Result<Option<SourceRow>, SourceError> {
        let mut value = file.to_value(self.model.id())?;
        if let Some(body) = body {
            value
                .as_object_mut()
                .ok_or_else(|| self.malformed("file"))?
                .insert("body".to_string(), serde_json::Value::String(body));
        }
        if !plan.pushed_filters_match(&value) {
            return Err(self.malformed("pushed filter"));
        }
        if !plan.residual.iter().all(|residual| residual.passes(&value)) {
            return Ok(None);
        }
        let mut projected = value
            .as_object()
            .cloned()
            .ok_or_else(|| self.malformed("file"))?;
        projected.retain(|field, _| plan.projected.contains(field));
        Ok(Some(SourceRow {
            source: self.model.id().clone(),
            entity: self.model.entity().to_string(),
            key: file.path_lower,
            value: serde_json::Value::Object(projected),
        }))
    }

    fn validate_download_result(
        &self,
        requested: &DropboxFile,
        response: &DropboxHttpResponse,
    ) -> Result<(), SourceError> {
        let header = response
            .headers
            .get("dropbox-api-result")
            .ok_or_else(|| self.malformed("Dropbox-API-Result"))?;
        let observed: DropboxFile =
            serde_json::from_str(header).map_err(|_| self.malformed("Dropbox-API-Result"))?;
        observed.validate(self.model.id())?;
        if observed.id != requested.id
            || observed.name != requested.name
            || observed.path_lower != requested.path_lower
            || observed.path_display != requested.path_display
            || observed.client_modified != requested.client_modified
            || observed.server_modified != requested.server_modified
            || observed.rev != requested.rev
            || observed.size != requested.size
            || observed.content_hash != requested.content_hash
        {
            return Err(self.malformed("download identity"));
        }
        Ok(())
    }

    fn encode_query_cursor(
        &self,
        plan: &SourcePlan,
        query_id: Uuid,
        provider_cursor: &str,
    ) -> Result<SourceCursor, SourceError> {
        validate_provider_cursor(provider_cursor)?;
        let payload = QueryCursorPayload {
            version: CURSOR_VERSION,
            connector: "catdb-dropbox-query/v1".to_string(),
            source_spec_digest: self.source_spec_digest.clone(),
            schema_version: self.spec.schema_version.clone(),
            plan_digest: plan_digest(plan)?,
            query_id,
            provider_cursor: provider_cursor.to_string(),
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
        context: &ReadContext,
    ) -> Result<String, SourceError> {
        let envelope: CursorEnvelope<QueryCursorPayload> =
            serde_json::from_slice(cursor.as_bytes()).map_err(|_| SourceError::InvalidCursor)?;
        if !self.verify(
            QUERY_CURSOR_DOMAIN,
            &envelope.payload,
            &envelope.authentication_tag,
        )? || envelope.payload.version != CURSOR_VERSION
            || envelope.payload.connector != "catdb-dropbox-query/v1"
            || envelope.payload.source_spec_digest != self.source_spec_digest
            || envelope.payload.schema_version != self.spec.schema_version
            || envelope.payload.plan_digest != plan_digest(plan)?
            || envelope.payload.query_id != context.query_id()
        {
            return Err(SourceError::InvalidCursor);
        }
        validate_provider_cursor(&envelope.payload.provider_cursor)?;
        self.consume_query_cursor(context.query_id(), context.deadline(), cursor.as_bytes())?;
        Ok(envelope.payload.provider_cursor)
    }

    fn consume_query_cursor(
        &self,
        query_id: Uuid,
        deadline: Instant,
        cursor: &[u8],
    ) -> Result<(), SourceError> {
        let digest: [u8; 32] = Sha256::digest(cursor).into();
        let now = Instant::now();
        let mut replays = self
            .query_replays
            .lock()
            .map_err(|_| SourceError::InvalidCursor)?;
        replays.retain(|_, state| state.expires_at > now);
        if !replays.contains_key(&query_id) && replays.len() >= MAX_ACTIVE_QUERY_IDS {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Dropbox active query cursors",
            });
        }
        let state = replays.entry(query_id).or_insert_with(|| QueryReplayState {
            expires_at: deadline,
            used: BTreeSet::new(),
        });
        state.expires_at = state.expires_at.max(deadline);
        if state.used.len() >= MAX_USED_QUERY_CURSORS {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Dropbox used query cursors",
            });
        }
        if !state.used.insert(digest) {
            return Err(SourceError::InvalidCursor);
        }
        Ok(())
    }

    fn encode_change_cursor(
        &self,
        provider_cursor: &str,
    ) -> Result<DropboxChangeCursor, SourceError> {
        validate_provider_cursor(provider_cursor)?;
        let payload = ChangeCursorPayload {
            version: CURSOR_VERSION,
            connector: "catdb-dropbox-changes/v1".to_string(),
            source_spec_digest: self.source_spec_digest.clone(),
            schema_version: self.spec.schema_version.clone(),
            provider_cursor: provider_cursor.to_string(),
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
        Ok(DropboxChangeCursor(bytes))
    }

    fn decode_change_cursor(&self, cursor: &DropboxChangeCursor) -> Result<String, SourceError> {
        let envelope: CursorEnvelope<ChangeCursorPayload> =
            serde_json::from_slice(&cursor.0).map_err(|_| SourceError::InvalidCursor)?;
        if !self.verify(
            CHANGE_CURSOR_DOMAIN,
            &envelope.payload,
            &envelope.authentication_tag,
        )? || envelope.payload.version != CURSOR_VERSION
            || envelope.payload.connector != "catdb-dropbox-changes/v1"
            || envelope.payload.source_spec_digest != self.source_spec_digest
            || envelope.payload.schema_version != self.spec.schema_version
        {
            return Err(SourceError::InvalidCursor);
        }
        validate_provider_cursor(&envelope.payload.provider_cursor)?;
        Ok(envelope.payload.provider_cursor)
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
            path: "dropbox-response".to_string(),
            attr: attr.to_string(),
            detail: "provider response did not match the pinned Dropbox API v2 contract"
                .to_string(),
        }
    }
}

#[async_trait]
impl DataSource for DropboxSource {
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
        if let Some(expected_path) =
            self.exact_path_filter(plan)
                .map_err(|_| SourceError::InvalidPlan {
                    source_id: self.model.id().clone(),
                })?
        {
            if cursor.is_some() {
                return Err(SourceError::InvalidCursor);
            }
            self.validate_path_scope(expected_path)?;
            let needs_body = plan
                .projected
                .iter()
                .map(String::as_str)
                .chain(plan.residual_attrs())
                .any(|field| field == "body");
            let mut physical_bytes = 0usize;
            let (file, body) = if needs_body && is_readable_text_path(expected_path) {
                let response = self
                    .send(
                        self.content_request_path(expected_path)?,
                        context,
                        &mut physical_bytes,
                    )
                    .await?;
                if response.status != 200 {
                    return Err(self.map_http_failure(&response));
                }
                let header = response
                    .headers
                    .get("dropbox-api-result")
                    .ok_or_else(|| self.malformed("Dropbox-API-Result"))?;
                let file: DropboxFile = serde_json::from_str(header)
                    .map_err(|_| self.malformed("Dropbox-API-Result"))?;
                let body = String::from_utf8(response.body)
                    .map_err(|_| self.malformed("body encoding"))?;
                (file, Some(body))
            } else {
                let response = self
                    .send(
                        self.api_request(
                            "/2/files/get_metadata",
                            serde_json::json!({ "path": expected_path }),
                        )?,
                        context,
                        &mut physical_bytes,
                    )
                    .await?;
                if response.status != 200 {
                    return Err(self.map_http_failure(&response));
                }
                let file: DropboxFile =
                    serde_json::from_slice(&response.body).map_err(|_| self.malformed("file"))?;
                (file, None)
            };
            self.validate_file_scope(&file)?;
            if file.path_lower != expected_path {
                return Err(self.malformed("selected file identity"));
            }
            let row = if is_exposed_path(self.spec.config.content_scope, &file.name) {
                self.project_file(file, body, plan)?
            } else {
                None
            };
            return Ok(SourcePage {
                source: self.model.id().clone(),
                scanned: 1,
                rows: row.into_iter().collect(),
                next_cursor: None,
                observed_at_ms: now_ms(),
                freshness_ms: 0,
            });
        }
        let mut provider_cursor = match cursor {
            Some(cursor) => Some(self.decode_query_cursor(plan, cursor, context)?),
            None => None,
        };
        let mut physical_bytes = 0usize;
        let mut empty_pages = 0usize;
        loop {
            context.check()?;
            let request = if let Some(cursor) = &provider_cursor {
                self.api_request(
                    "/2/files/list_folder/continue",
                    serde_json::json!({ "cursor": cursor }),
                )?
            } else {
                self.api_request(
                    "/2/files/list_folder",
                    list_arguments(&self.spec.config.selected_path, page_size, false),
                )?
            };
            let response = self.send(request, context, &mut physical_bytes).await?;
            if response.status != 200 {
                return Err(self.map_http_failure(&response));
            }
            let page: DropboxListResponse =
                serde_json::from_slice(&response.body).map_err(|_| self.malformed("entries"))?;
            page.validate(self.model.id())?;
            if page.has_more
                && provider_cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor == &page.cursor)
            {
                return Err(SourceError::InvalidCursor);
            }
            if page.entries.len() > page_size {
                return Err(SourceError::ResourceLimit {
                    source_id: self.model.id().clone(),
                    resource: "Dropbox page rows",
                });
            }
            if page.entries.is_empty() && page.has_more {
                empty_pages += 1;
                if empty_pages > MAX_EMPTY_PROVIDER_PAGES {
                    return Err(SourceError::ResourceLimit {
                        source_id: self.model.id().clone(),
                        resource: "Dropbox empty provider pages",
                    });
                }
                provider_cursor = Some(page.cursor);
                continue;
            }
            let scanned = page.entries.len();
            let mut rows = Vec::with_capacity(scanned);
            for entry in page.entries {
                match parse_entry(entry, self.model.id())? {
                    DropboxEntry::File(file) => {
                        if let Some(row) = self
                            .observe_file(file, plan, context, &mut physical_bytes)
                            .await?
                        {
                            rows.push(row);
                        }
                    }
                    DropboxEntry::Folder(folder) => {
                        validate_folder(&folder, self.model.id())?;
                        self.validate_path_scope(&folder.path_lower)?;
                    }
                    DropboxEntry::Deleted(_) => {
                        return Err(self.malformed("deleted listing entry"));
                    }
                }
            }
            let next_cursor = if page.has_more {
                Some(self.encode_query_cursor(plan, context.query_id(), &page.cursor)?)
            } else {
                None
            };
            return Ok(SourcePage {
                source: self.model.id().clone(),
                scanned,
                rows,
                next_cursor,
                observed_at_ms: now_ms(),
                freshness_ms: 0,
            });
        }
    }
}

#[derive(Deserialize)]
struct DropboxListResponse {
    entries: Vec<serde_json::Value>,
    cursor: String,
    has_more: bool,
}

impl DropboxListResponse {
    fn validate(&self, source_id: &SourceId) -> Result<(), SourceError> {
        validate_provider_cursor(&self.cursor).map_err(|_| malformed_source(source_id, "cursor"))
    }
}

#[derive(Deserialize)]
struct DropboxStartCursor {
    cursor: String,
}

enum DropboxEntry {
    File(DropboxFile),
    Folder(DropboxFolder),
    Deleted(DropboxDeleted),
}

#[derive(Deserialize)]
struct DropboxFile {
    #[serde(rename = ".tag")]
    tag: String,
    name: String,
    path_lower: String,
    path_display: String,
    id: String,
    client_modified: String,
    server_modified: String,
    rev: String,
    size: u64,
    #[serde(default)]
    content_hash: Option<String>,
}

impl DropboxFile {
    fn validate(&self, source_id: &SourceId) -> Result<(), SourceError> {
        if self.tag != "file"
            || !valid_provider_atom(&self.name, 1_024)
            || !valid_dropbox_path(&self.path_lower)
            || !valid_display_path(&self.path_display)
            || !valid_provider_atom(&self.id, 1_024)
            || !valid_provider_atom(&self.rev, 512)
            || self
                .content_hash
                .as_ref()
                .is_some_and(|content_hash| !valid_provider_atom(content_hash, 512))
            || self.size > i64::MAX as u64
            || parse_timestamp_ms(&self.client_modified).is_err()
            || parse_timestamp_ms(&self.server_modified).is_err()
        {
            return Err(malformed_source(source_id, "file"));
        }
        Ok(())
    }

    fn to_value(&self, source_id: &SourceId) -> Result<serde_json::Value, SourceError> {
        self.validate(source_id)?;
        let mut value = serde_json::json!({
            "id": self.id,
            "name": self.name,
            "path": self.path_lower,
            "modifiedAt": parse_timestamp_ms(&self.server_modified)
                .map_err(|_| malformed_source(source_id, "modifiedAt"))?,
            "size": i64::try_from(self.size).map_err(|_| malformed_source(source_id, "size"))?,
            "rev": self.rev,
        });
        if let Some(content_hash) = &self.content_hash {
            value
                .as_object_mut()
                .ok_or_else(|| malformed_source(source_id, "file"))?
                .insert(
                    "contentHash".to_string(),
                    serde_json::Value::String(content_hash.clone()),
                );
        }
        Ok(value)
    }
}

#[derive(Deserialize)]
struct DropboxFolder {
    #[serde(rename = ".tag")]
    tag: String,
    name: String,
    path_lower: String,
    id: String,
}

#[derive(Deserialize)]
struct DropboxDeleted {
    #[serde(rename = ".tag")]
    tag: String,
    name: String,
    path_lower: String,
}

fn parse_entry(
    value: serde_json::Value,
    source_id: &SourceId,
) -> Result<DropboxEntry, SourceError> {
    let tag = value
        .get(".tag")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| malformed_source(source_id, "entry tag"))?;
    match tag {
        "file" => serde_json::from_value(value)
            .map(DropboxEntry::File)
            .map_err(|_| malformed_source(source_id, "file")),
        "folder" => serde_json::from_value(value)
            .map(DropboxEntry::Folder)
            .map_err(|_| malformed_source(source_id, "folder")),
        "deleted" => serde_json::from_value(value)
            .map(DropboxEntry::Deleted)
            .map_err(|_| malformed_source(source_id, "deleted")),
        _ => Err(malformed_source(source_id, "entry tag")),
    }
}

fn validate_folder(folder: &DropboxFolder, source_id: &SourceId) -> Result<(), SourceError> {
    if folder.tag == "folder"
        && valid_provider_atom(&folder.name, 1_024)
        && valid_dropbox_path(&folder.path_lower)
        && valid_provider_atom(&folder.id, 1_024)
    {
        Ok(())
    } else {
        Err(malformed_source(source_id, "folder"))
    }
}

fn validate_deleted(deleted: &DropboxDeleted, source_id: &SourceId) -> Result<(), SourceError> {
    if deleted.tag == "deleted"
        && valid_provider_atom(&deleted.name, 1_024)
        && valid_dropbox_path(&deleted.path_lower)
    {
        Ok(())
    } else {
        Err(malformed_source(source_id, "deleted"))
    }
}

fn list_arguments(
    selected_path: &str,
    page_size: usize,
    include_deleted: bool,
) -> serde_json::Value {
    serde_json::json!({
        "path": selected_path,
        "recursive": true,
        "include_deleted": include_deleted,
        "include_has_explicit_shared_members": false,
        "include_mounted_folders": true,
        "limit": page_size,
    })
}

fn allowed_request(endpoint: DropboxEndpoint, path: &str) -> bool {
    matches!(
        (endpoint, path),
        (DropboxEndpoint::Api, "/2/files/list_folder")
            | (DropboxEndpoint::Api, "/2/files/list_folder/continue")
            | (DropboxEndpoint::Api, "/2/files/get_metadata")
            | (
                DropboxEndpoint::Api,
                "/2/files/list_folder/get_latest_cursor"
            )
            | (DropboxEndpoint::Content, "/2/files/download")
            | (DropboxEndpoint::Content, "/2/files/upload")
    )
}

fn validate_selected_path(path: &str) -> Result<(), DropboxConnectorError> {
    if path.len() > 4_096
        || contains_control(path)
        || (!path.is_empty() && !path.starts_with('/'))
        || path.split('/').any(|component| component == "..")
    {
        return Err(DropboxConnectorError::InvalidConfiguration {
            field: "selected_path",
        });
    }
    Ok(())
}

fn normalize_selected_path(mut path: String) -> Result<String, DropboxConnectorError> {
    validate_selected_path(&path)?;
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    if path == "/" {
        path.clear();
    }
    Ok(path)
}

fn valid_dropbox_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4_096
        && path.starts_with('/')
        && !contains_control(path)
        && !path.split('/').any(|component| component == "..")
        && path == path.to_ascii_lowercase()
}

fn valid_display_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4_096
        && path.starts_with('/')
        && !contains_control(path)
        && !path.split('/').any(|component| component == "..")
}

fn valid_provider_atom(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !contains_control(value)
}

fn validate_provider_cursor(cursor: &str) -> Result<(), SourceError> {
    if cursor.is_empty() || cursor.len() > MAX_PROVIDER_CURSOR_BYTES || contains_control(cursor) {
        return Err(SourceError::InvalidCursor);
    }
    Ok(())
}

fn is_exposed_path(scope: DropboxContentScope, name: &str) -> bool {
    match scope {
        DropboxContentScope::AllMetadata => true,
        DropboxContentScope::ReadableText => is_readable_text_path(name),
        DropboxContentScope::Markdown => name.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
        }),
    }
}

fn is_readable_text_path(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "txt"
        )
    })
}

fn is_reset_response(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .and_then(|error| error.get(".tag").cloned())
        .and_then(|tag| tag.as_str().map(str::to_string))
        .as_deref()
        == Some("reset")
}

fn retry_after_ms(response: &DropboxHttpResponse) -> Option<u64> {
    response
        .headers
        .get("retry-after")
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|seconds| seconds.checked_mul(1_000))
}

fn digest<T: Serialize>(value: &T) -> Result<String, DropboxConnectorError> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| DropboxConnectorError::InvalidConfiguration {
            field: "source_spec",
        })?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn plan_digest(plan: &SourcePlan) -> Result<String, SourceError> {
    let bytes = serde_json::to_vec(plan).map_err(|_| SourceError::InvalidCursor)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn parse_timestamp_ms(value: &str) -> Result<i64, ()> {
    let parsed = time::OffsetDateTime::parse(value, &Rfc3339).map_err(|_| ())?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).map_err(|_| ())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn contains_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn malformed_source(source_id: &SourceId, attr: &'static str) -> SourceError {
    SourceError::Malformed {
        source_id: source_id.clone(),
        path: "dropbox-response".to_string(),
        attr: attr.to_string(),
        detail: "provider response did not match the pinned Dropbox API v2 contract".to_string(),
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    #[test]
    fn reqwest_content_upload_builds_an_octet_stream_request() {
        let transport = ReqwestDropboxTransport::new().expect("transport");
        let request = DropboxHttpRequest {
            endpoint: DropboxEndpoint::Content,
            method: DropboxHttpMethod::Post,
            path: "/2/files/upload".to_string(),
            json_body: serde_json::json!({}),
            headers: BTreeMap::from([(
                "dropbox-api-arg".to_string(),
                r#"{"path":"id:record","mode":{".tag":"update","update":"rev-1"},"strict_conflict":true,"autorename":false}"#.to_string(),
            )]),
            body: b"# Markdown".to_vec(),
            bearer_token: Arc::from("fixture-access-token"),
            credential_generation: 0,
        };

        let built = transport
            .build_http_request(&request)
            .expect("bounded upload request");

        assert_eq!(built.method(), reqwest::Method::POST);
        assert_eq!(
            built.headers().get(reqwest::header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(
            built.body().and_then(reqwest::Body::as_bytes),
            Some(&b"# Markdown"[..])
        );
    }
}
