//! Google Drive API v3 source for an MDReader Library.
//!
//! The semantic [`DataSource`] surface remains read-only. Provider-local
//! document operations are separately typed and do not widen that trait.
//! Interactive OAuth credentials and cursor-authentication keys are resolved
//! at runtime and never enter the immutable source specification.

#![deny(missing_docs)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use catdb_sdk::connectors::{
    DataSource, ReadContext, SourceCapabilities, SourceCursor, SourceError, SourceFreshness,
    SourceId, SourceModel, SourcePage, SourcePlan, SourceRow,
};
use catdb_sdk::core::{BaseType, Schema};
use catdb_sdk::query::{AttrOp, EntityFilterInput, EntityQuery, EntityTarget, EntityVocabulary};
use catdb_sdk::storage::{ObjectFilterOp, ObjectValue};
use futures_util::StreamExt as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use time::format_description::well_known::Rfc3339;
use url::Url;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// Google's read-only scope for browsing and downloading user-readable files.
pub const GOOGLE_DRIVE_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
/// Google's full Drive scope required for guarded Markdown updates.
pub const GOOGLE_DRIVE_FULL_SCOPE: &str = "https://www.googleapis.com/auth/drive";

const API_BASE: &str = "https://www.googleapis.com";
const AUTHORIZATION_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const MAX_HTTP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MARKDOWN_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONFIGURED_SELECTIONS: usize = 512;
const MAX_SELECTION_ID_BYTES: usize = 256;
const MAX_RESOURCE_KEY_BYTES: usize = 512;
const MAX_RESOURCE_KEY_HEADER_BYTES: usize = 8 * 1024;
const MAX_CHANGE_EVENTS: usize = 1_000;
const MAX_EMPTY_PROVIDER_PAGES: usize = 32;
const MAX_RECURSION_DEPTH: usize = 32;
const MAX_TRAVERSAL_ITEMS: usize = 100_000;
const MAX_POINT_READ_ANCESTRY_ITEMS: usize = 256;
const MAX_ACTIVE_TRAVERSALS: usize = 128;
const ACCESS_TOKEN_EXPIRY_SKEW: Duration = Duration::from_secs(30);
const CURSOR_VERSION: u8 = 1;
const QUERY_CURSOR_DOMAIN: &[u8] = b"catdb-google-drive-query-cursor\0";
const CHANGE_CURSOR_DOMAIN: &[u8] = b"catdb-google-drive-change-cursor\0";
const WRITE_RECONCILIATION_DOMAIN: &[u8] = b"catdb-google-drive-write-reconciliation\0";
const GOOGLE_DRIVE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const GOOGLE_DRIVE_DOCUMENT_MIME: &str = "application/vnd.google-apps.document";

/// The physical records exposed by one immutable Drive source.
///
/// The default preserves the general connector contract, including
/// metadata-only records. Reader applications may explicitly pin the narrower
/// text-document universe so provider filtering is part of the versioned
/// source meaning rather than an unreported UI shortcut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriveContentScope {
    /// Expose every selected file, including metadata-only records.
    #[default]
    AllMetadata,
    /// Expose Google Docs, Markdown, and plain-text records only.
    ReadableText,
    /// Expose Markdown files only.
    Markdown,
}

/// The fixed OAuth capability requested by a native Drive authorization flow.
///
/// The default remains read-only so existing applications cannot accidentally
/// widen a consent request. Markdown editing must request the explicit V2
/// profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DriveAuthorizationProfile {
    /// Read and download Drive content without mutation authority.
    #[default]
    ReadOnly,
    /// Read and guarded-update Markdown content.
    MarkdownReadWrite,
}

impl DriveAuthorizationProfile {
    fn scope(self) -> &'static str {
        match self {
            Self::ReadOnly => GOOGLE_DRIVE_READONLY_SCOPE,
            Self::MarkdownReadWrite => GOOGLE_DRIVE_FULL_SCOPE,
        }
    }
}

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
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DriveSelection {
    /// A specific Picker-selected file.
    File {
        /// Stable Google Drive file id.
        id: String,
        /// Picker resource key for a link-shared item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_key: Option<String>,
    },
    /// A Picker-selected folder whose visible descendants are listed.
    Folder {
        /// Stable Google Drive folder id.
        id: String,
        /// Picker resource key for a link-shared item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_key: Option<String>,
    },
    /// The provider's My Drive root, whose readable records may be searched
    /// directly instead of recursively walking every folder.
    Root {
        /// Stable provider root id resolved from the `root` alias.
        id: String,
        /// Resource key when the provider supplies one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_key: Option<String>,
    },
}

impl fmt::Debug for DriveSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { .. } => formatter.write_str("DriveSelection::File([redacted-id])"),
            Self::Folder { .. } => formatter.write_str("DriveSelection::Folder([redacted-id])"),
            Self::Root { .. } => formatter.write_str("DriveSelection::Root([redacted-id])"),
        }
    }
}

impl DriveSelection {
    /// Select one file.
    pub fn file(id: impl Into<String>) -> Self {
        Self::File {
            id: id.into(),
            resource_key: None,
        }
    }

    /// Select one folder.
    pub fn folder(id: impl Into<String>) -> Self {
        Self::Folder {
            id: id.into(),
            resource_key: None,
        }
    }

    /// Select the provider's My Drive root.
    pub fn root(id: impl Into<String>) -> Self {
        Self::Root {
            id: id.into(),
            resource_key: None,
        }
    }

    /// Select the provider root with its resource key.
    pub fn root_with_resource_key(
        id: impl Into<String>,
        resource_key: impl Into<String>,
    ) -> Result<Self, DriveConnectorError> {
        let id = id.into();
        let resource_key = resource_key.into();
        validate_drive_id("selection", &id)?;
        validate_resource_key(&resource_key)?;
        Ok(Self::Root {
            id,
            resource_key: Some(resource_key),
        })
    }

    /// Select one link-shared file with its Picker resource key.
    pub fn file_with_resource_key(
        id: impl Into<String>,
        resource_key: impl Into<String>,
    ) -> Result<Self, DriveConnectorError> {
        Self::with_resource_key(id, resource_key, true)
    }

    /// Select one link-shared folder with its Picker resource key.
    pub fn folder_with_resource_key(
        id: impl Into<String>,
        resource_key: impl Into<String>,
    ) -> Result<Self, DriveConnectorError> {
        Self::with_resource_key(id, resource_key, false)
    }

    fn with_resource_key(
        id: impl Into<String>,
        resource_key: impl Into<String>,
        file: bool,
    ) -> Result<Self, DriveConnectorError> {
        let id = id.into();
        let resource_key = resource_key.into();
        validate_drive_id("selection", &id)?;
        validate_resource_key(&resource_key)?;
        Ok(if file {
            Self::File {
                id,
                resource_key: Some(resource_key),
            }
        } else {
            Self::Folder {
                id,
                resource_key: Some(resource_key),
            }
        })
    }

    fn id(&self) -> &str {
        match self {
            Self::File { id, .. } | Self::Folder { id, .. } | Self::Root { id, .. } => id,
        }
    }

    fn resource_key(&self) -> Option<&str> {
        match self {
            Self::File { resource_key, .. }
            | Self::Folder { resource_key, .. }
            | Self::Root { resource_key, .. } => resource_key.as_deref(),
        }
    }

    fn validate(&self) -> Result<(), DriveConnectorError> {
        validate_drive_id("selection", self.id())?;
        if let Some(resource_key) = self.resource_key() {
            validate_resource_key(resource_key)?;
        }
        Ok(())
    }
}

/// Immutable, non-secret Google Drive source configuration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveSourceConfig {
    source_id: String,
    catdb_schema: String,
    catdb_entity: String,
    selections: Vec<DriveSelection>,
    credential_reference: DriveSecretReference,
    shared_drive_id: Option<String>,
    #[serde(default)]
    content_scope: DriveContentScope,
}

impl fmt::Debug for DriveSourceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveSourceConfig")
            .field("source_id", &self.source_id)
            .field("catdb_schema", &self.catdb_schema)
            .field("catdb_entity", &self.catdb_entity)
            .field("selection_count", &self.selections.len())
            .field("credential_reference", &self.credential_reference)
            .field("shared_drive", &self.shared_drive_id.is_some())
            .finish()
    }
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
            content_scope: DriveContentScope::AllMetadata,
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

    /// Pin the physical record universe exposed by this source.
    pub fn with_content_scope(mut self, scope: DriveContentScope) -> Self {
        self.content_scope = scope;
        self
    }

    /// Exact OAuth scope required by the connector.
    pub fn oauth_scope(&self) -> &'static str {
        GOOGLE_DRIVE_READONLY_SCOPE
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
        let mut unique_ids = BTreeSet::new();
        for selection in &self.selections {
            selection.validate()?;
            if !unique_ids.insert(selection.id()) {
                return Err(DriveConnectorError::InvalidConfiguration {
                    field: "selections",
                });
            }
        }
        let resource_key_header_bytes = self
            .selections
            .iter()
            .filter_map(|selection| {
                selection
                    .resource_key()
                    .map(|key| selection.id().len() + 1 + key.len())
            })
            .try_fold(0usize, |total, entry| {
                total.checked_add(entry.saturating_add(1))
            })
            .ok_or(DriveConnectorError::InvalidConfiguration {
                field: "selections",
            })?;
        if resource_key_header_bytes > MAX_RESOURCE_KEY_HEADER_BYTES {
            return Err(DriveConnectorError::InvalidConfiguration {
                field: "selections",
            });
        }
        if let Some(drive_id) = &self.shared_drive_id {
            validate_drive_id("shared_drive_id", drive_id)?;
        }
        Ok(())
    }
}

/// Catalog-pinned Drive configuration and semantic schema identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinnedDriveSourceSpec {
    config: DriveSourceConfig,
    schema_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPinnedDriveSourceSpec {
    config: DriveSourceConfig,
    schema_version: String,
}

impl<'de> Deserialize<'de> for PinnedDriveSourceSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawPinnedDriveSourceSpec::deserialize(deserializer)?;
        Self::new(raw.config, raw.schema_version).map_err(serde::de::Error::custom)
    }
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

/// Runtime-only access token and its provider expiry.
pub struct DriveAccessGrant {
    token: DriveAccessToken,
    expires_at: Option<SystemTime>,
}

impl DriveAccessGrant {
    /// Bind an access token to its expiry, when the provider supplied one.
    pub fn new(
        token: DriveAccessToken,
        expires_at: Option<SystemTime>,
    ) -> Result<Self, DriveConnectorError> {
        if expires_at.is_some_and(|expiry| expiry.duration_since(UNIX_EPOCH).is_err()) {
            return Err(DriveConnectorError::InvalidSecret);
        }
        Ok(Self { token, expires_at })
    }

    fn is_expired(&self) -> bool {
        match self.expires_at {
            None => false,
            Some(expiry) => expiry
                .duration_since(SystemTime::now())
                .map_or(true, |remaining| remaining <= ACCESS_TOKEN_EXPIRY_SKEW),
        }
    }
}

impl fmt::Debug for DriveAccessGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveAccessGrant")
            .field("token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Runtime-only refresh credential resolved from application-owned storage.
pub struct DriveRefreshCredential(Zeroizing<String>);

impl DriveRefreshCredential {
    /// Construct a bounded refresh credential that never enters source specs.
    pub fn new(credential: impl Into<String>) -> Result<Self, DriveConnectorError> {
        let credential = credential.into();
        if credential.is_empty()
            || credential.len() > 16 * 1024
            || credential.chars().any(char::is_control)
        {
            return Err(DriveConnectorError::InvalidSecret);
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

impl fmt::Debug for DriveRefreshCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveRefreshCredential([redacted])")
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
    grant: DriveAccessGrant,
    refresh_credential: Option<DriveRefreshCredential>,
    cursor_key: DriveCursorKey,
}

impl DriveRuntimeSecrets {
    /// Bind OAuth credentials to a stable cursor key.
    pub fn new(token: DriveAccessToken, cursor_key: DriveCursorKey) -> Self {
        Self {
            grant: DriveAccessGrant {
                token,
                expires_at: None,
            },
            refresh_credential: None,
            cursor_key,
        }
    }

    /// Bind an expiring access grant and refresh credential to a cursor key.
    pub fn refreshable(
        grant: DriveAccessGrant,
        refresh_credential: DriveRefreshCredential,
        cursor_key: DriveCursorKey,
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
        DriveAccessGrant,
        Option<DriveRefreshCredential>,
        DriveCursorKey,
    ) {
        (self.grant, self.refresh_credential, self.cursor_key)
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

    /// Exchange a runtime-only refresh credential for a new access grant.
    async fn refresh(
        &self,
        _reference: &DriveSecretReference,
        _credential: &DriveRefreshCredential,
    ) -> Result<DriveAccessGrant, DriveConnectorError> {
        Err(DriveConnectorError::InvalidSecret)
    }
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
        self.authorization_url_for_profile(client_id, DriveAuthorizationProfile::ReadOnly)
    }

    /// Build the native-app authorization URL for one fixed capability profile.
    pub fn authorization_url_for_profile(
        &self,
        client_id: &str,
        profile: DriveAuthorizationProfile,
    ) -> Result<String, DriveConnectorError> {
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
            .append_pair("scope", profile.scope())
            .append_pair("code_challenge", &self.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &self.state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
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

/// HTTP method emitted by this connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveHttpMethod {
    /// Read/list/export/download.
    Get,
    /// Guarded document content update.
    Patch,
}

/// Bounded authenticated Drive API request.
#[derive(Clone)]
pub struct DriveHttpRequest {
    method: DriveHttpMethod,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    bearer_token: Arc<str>,
    credential_generation: u64,
}

impl DriveHttpRequest {
    /// Request method.
    pub fn method(&self) -> DriveHttpMethod {
        self.method
    }

    /// API-relative path and encoded query.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Read one non-secret provider request header case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Bounded document bytes supplied to the injected transport.
    pub fn body(&self) -> &[u8] {
        self.body.as_slice()
    }

    /// Runtime credential generation used for bounded retry diagnostics.
    pub fn credential_generation(&self) -> u64 {
        self.credential_generation
    }
}

impl fmt::Debug for DriveHttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveHttpRequest")
            .field("method", &self.method)
            .field("path", &redacted_path(&self.path))
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("body_bytes", &self.body.len())
            .field("bearer_token", &"[redacted]")
            .field("credential_generation", &self.credential_generation)
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
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
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
        let mut builder = self
            .client
            .request(
                match request.method {
                    DriveHttpMethod::Get => reqwest::Method::GET,
                    DriveHttpMethod::Patch => reqwest::Method::PATCH,
                },
                url,
            )
            .bearer_auth(request.bearer_token.as_ref());
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if request.method == DriveHttpMethod::Patch {
            builder = builder
                .header("content-type", "text/markdown; charset=utf-8")
                .body(request.body);
        }
        let response = builder
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

impl DriveChangeCursor {
    /// Restore bounded opaque change state from application-managed storage.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, SourceError> {
        if bytes.len() > catdb_sdk::connectors::MAX_SOURCE_CURSOR_BYTES {
            return Err(SourceError::CursorTooLarge {
                actual: bytes.len(),
                maximum: catdb_sdk::connectors::MAX_SOURCE_CURSOR_BYTES,
            });
        }
        Ok(Self(bytes))
    }

    /// Opaque authenticated bytes suitable for application-managed storage.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for DriveChangeCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DriveChangeCursor([opaque])")
    }
}

/// Opaque provider revision used to guard a single Drive document operation.
#[derive(Clone, PartialEq, Eq)]
pub struct DriveRevision(String);

impl DriveRevision {
    /// Validate provider revision authority before it is used for a document operation.
    pub fn new(revision: impl Into<String>) -> Result<Self, DriveConnectorError> {
        let revision = revision.into();
        if revision.is_empty() || revision.len() > 512 || revision.chars().any(char::is_control) {
            return Err(DriveConnectorError::InvalidRevision);
        }
        Ok(Self(revision))
    }

    /// Return the opaque provider revision for an immediate guarded update.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for DriveRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveRevision")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// Safe identity for reconciling a Drive mutation whose provider outcome is unknown.
#[derive(Clone, PartialEq, Eq)]
pub struct DriveWriteReconciliation {
    source_id: SourceId,
    record_fingerprint: String,
}

impl DriveWriteReconciliation {
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

impl fmt::Debug for DriveWriteReconciliation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveWriteReconciliation")
            .field("source_id", &self.source_id)
            .field("record_fingerprint_bytes", &self.record_fingerprint.len())
            .finish()
    }
}

/// A provider-local document operation could not complete.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum DriveDocumentError {
    /// The requested provider record no longer exists.
    #[error("Drive document was not found")]
    NotFound {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider rejected a guarded update because its revision changed.
    #[error("Drive document changed before the guarded update for source `{source_id}`")]
    Conflict {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The document exceeded a fixed document-operation bound.
    #[error("Drive document exceeded a fixed bound")]
    TooLarge {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider returned document bytes that were not valid UTF-8.
    #[error("Drive document body was not valid UTF-8")]
    InvalidUtf8 {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider returned a record other than the requested record.
    #[error("Drive document identity did not match the request")]
    IdentityMismatch {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider response did not meet the pinned document protocol.
    #[error("Drive document response did not meet the pinned protocol")]
    Protocol {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The provider rejected the runtime credential.
    #[error("Drive document authentication failed")]
    Authentication {
        /// Non-secret source identity.
        source_id: SourceId,
    },
    /// The document operation exceeded its deadline.
    #[error("Drive document operation timed out")]
    Timeout,
    /// The document operation was cooperatively cancelled.
    #[error("Drive document operation was cancelled")]
    Cancelled,
    /// The provider rate-limited the document operation.
    #[error("Drive document operation was rate limited")]
    RateLimited {
        /// Non-secret source identity.
        source_id: SourceId,
        /// Server-requested wait, when supplied.
        retry_after_ms: Option<u64>,
    },
    /// The guarded mutation may have been accepted, but no usable outcome was received.
    #[error("Drive write outcome is unknown; reconcile the returned identity")]
    WriteOutcomeUnknown {
        /// Non-secret identity for a provider read/reconciliation pass.
        reconciliation: DriveWriteReconciliation,
    },
    /// A source failure that has no more-specific document-operation outcome.
    #[error("Drive document operation failed")]
    Source {
        /// Typed source failure preserved without exposing it through `Debug`.
        #[source]
        error: SourceError,
    },
}

impl From<SourceError> for DriveDocumentError {
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

impl fmt::Debug for DriveDocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { source_id } => formatter
                .debug_struct("DriveDocumentError::NotFound")
                .field("source_id", source_id)
                .finish(),
            Self::Conflict { source_id } => formatter
                .debug_struct("DriveDocumentError::Conflict")
                .field("source_id", source_id)
                .finish(),
            Self::TooLarge { source_id } => formatter
                .debug_struct("DriveDocumentError::TooLarge")
                .field("source_id", source_id)
                .finish(),
            Self::InvalidUtf8 { source_id } => formatter
                .debug_struct("DriveDocumentError::InvalidUtf8")
                .field("source_id", source_id)
                .finish(),
            Self::IdentityMismatch { source_id } => formatter
                .debug_struct("DriveDocumentError::IdentityMismatch")
                .field("source_id", source_id)
                .finish(),
            Self::Protocol { source_id } => formatter
                .debug_struct("DriveDocumentError::Protocol")
                .field("source_id", source_id)
                .finish(),
            Self::Authentication { source_id } => formatter
                .debug_struct("DriveDocumentError::Authentication")
                .field("source_id", source_id)
                .finish(),
            Self::Timeout => formatter.write_str("DriveDocumentError::Timeout"),
            Self::Cancelled => formatter.write_str("DriveDocumentError::Cancelled"),
            Self::RateLimited {
                source_id,
                retry_after_ms,
            } => formatter
                .debug_struct("DriveDocumentError::RateLimited")
                .field("source_id", source_id)
                .field("retry_after_ms", retry_after_ms)
                .finish(),
            Self::WriteOutcomeUnknown { reconciliation } => formatter
                .debug_struct("DriveDocumentError::WriteOutcomeUnknown")
                .field("reconciliation", reconciliation)
                .finish(),
            Self::Source { .. } => formatter.write_str("DriveDocumentError::Source([redacted])"),
        }
    }
}

/// One bounded Markdown document downloaded from Drive.
#[derive(Clone, PartialEq, Eq)]
pub struct DriveDownloadedDocument {
    record_key: String,
    revision: DriveRevision,
    body: Vec<u8>,
}

impl DriveDownloadedDocument {
    /// Stable provider record key requested for this document.
    pub fn record_key(&self) -> &str {
        self.record_key.as_str()
    }

    /// Provider revision authority captured with the downloaded body.
    pub fn revision(&self) -> &DriveRevision {
        &self.revision
    }

    /// Validated UTF-8 Markdown bytes.
    pub fn body(&self) -> &[u8] {
        self.body.as_slice()
    }
}

impl fmt::Debug for DriveDownloadedDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveDownloadedDocument")
            .field("record_key_bytes", &self.record_key.len())
            .field("revision_bytes", &self.revision.0.len())
            .field("body_bytes", &self.body.len())
            .field("body_encoding", &"utf-8")
            .finish()
    }
}

/// Receipt for one guarded Drive Markdown update.
#[derive(Clone, PartialEq, Eq)]
pub struct DriveWriteReceipt {
    record_key: String,
    revision: DriveRevision,
}

impl DriveWriteReceipt {
    /// Stable provider record key updated by this operation.
    pub fn record_key(&self) -> &str {
        self.record_key.as_str()
    }

    /// Provider revision authority returned by the update.
    pub fn revision(&self) -> &DriveRevision {
        &self.revision
    }
}

impl fmt::Debug for DriveWriteReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveWriteReceipt")
            .field("record_key_bytes", &self.record_key.len())
            .field("revision_bytes", &self.revision.0.len())
            .field("provider_response", &"File")
            .finish()
    }
}

/// One provider-native incremental change identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveChangeEvent {
    /// A tracked file was removed or trashed.
    Deleted {
        /// Stable Drive file id.
        file_id: String,
    },
    /// A tracked file is present in the provider change page.
    Upserted {
        /// Stable Drive file id.
        file_id: String,
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
    traversal_id: Uuid,
    generation: u64,
}

struct FolderTraversal {
    id: String,
    depth: usize,
    page_token: Option<String>,
    whole_drive: bool,
}

struct QueryTraversal {
    traversal_id: Uuid,
    generation: u64,
    expires_at: Instant,
    membership_epoch: u64,
    membership: DriveMembership,
    selected_file_index: usize,
    selected_folder_index: usize,
    folders: VecDeque<FolderTraversal>,
    seen_ids: BTreeSet<String>,
}

#[derive(Clone)]
struct DriveMembership {
    tracked_folders: BTreeSet<String>,
    resource_keys: BTreeMap<String, String>,
    resource_key_header_bytes: usize,
}

struct DriveMembershipState {
    current: DriveMembership,
    next_rebuild_epoch: u64,
    committed_rebuild_epoch: u64,
}

impl DriveMembership {
    fn from_selections(selections: &[DriveSelection]) -> Result<Self, DriveConnectorError> {
        let mut tracked_folders = BTreeSet::new();
        let mut resource_keys = BTreeMap::new();
        let mut resource_key_header_bytes = 0usize;
        for selection in selections {
            if matches!(
                selection,
                DriveSelection::Folder { .. } | DriveSelection::Root { .. }
            ) {
                tracked_folders.insert(selection.id().to_string());
            }
            if let Some(resource_key) = selection.resource_key() {
                resource_key_header_bytes = resource_key_header_bytes
                    .checked_add(selection.id().len() + 1 + resource_key.len() + 1)
                    .ok_or(DriveConnectorError::InvalidConfiguration {
                        field: "selections",
                    })?;
                resource_keys.insert(selection.id().to_string(), resource_key.to_string());
            }
        }
        if resource_key_header_bytes > MAX_RESOURCE_KEY_HEADER_BYTES {
            return Err(DriveConnectorError::InvalidConfiguration {
                field: "selections",
            });
        }
        Ok(Self {
            tracked_folders,
            resource_keys,
            resource_key_header_bytes,
        })
    }

    fn record_provider_file(
        &mut self,
        file: &DriveFile,
        track_folder: bool,
        source_id: &SourceId,
    ) -> Result<(), SourceError> {
        let next_header_bytes = match (
            self.resource_keys.get(&file.id),
            file.resource_key.as_deref(),
        ) {
            (Some(existing), Some(observed)) if existing != observed => {
                return Err(malformed_source(source_id, "resourceKey"));
            }
            (Some(_), _) | (None, None) => self.resource_key_header_bytes,
            (None, Some(resource_key)) => self
                .resource_key_header_bytes
                .checked_add(file.id.len() + 1 + resource_key.len() + 1)
                .ok_or(SourceError::ResourceLimit {
                    source_id: source_id.clone(),
                    resource: "Drive resource-key header",
                })?,
        };
        if next_header_bytes > MAX_RESOURCE_KEY_HEADER_BYTES {
            return Err(SourceError::ResourceLimit {
                source_id: source_id.clone(),
                resource: "Drive resource-key header",
            });
        }
        if track_folder {
            self.tracked_folders.insert(file.id.clone());
        }
        if let Some(resource_key) = &file.resource_key {
            self.resource_keys
                .entry(file.id.clone())
                .or_insert_with(|| resource_key.clone());
        }
        self.resource_key_header_bytes = next_header_bytes;
        Ok(())
    }

    fn resource_key_headers(
        &self,
        source_id: &SourceId,
    ) -> Result<BTreeMap<String, String>, SourceError> {
        if self.resource_key_header_bytes > MAX_RESOURCE_KEY_HEADER_BYTES {
            return Err(SourceError::ResourceLimit {
                source_id: source_id.clone(),
                resource: "Drive resource-key header",
            });
        }
        let value = self
            .resource_keys
            .iter()
            .map(|(id, key)| format!("{id}/{key}"))
            .collect::<Vec<_>>()
            .join(",");
        if value.len() > MAX_RESOURCE_KEY_HEADER_BYTES {
            return Err(SourceError::ResourceLimit {
                source_id: source_id.clone(),
                resource: "Drive resource-key header",
            });
        }
        Ok(if value.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([("x-goog-drive-resource-keys".to_string(), value)])
        })
    }
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

struct StoredAccessGrant {
    grant: DriveAccessGrant,
    generation: u64,
}

struct DriveCredentialState {
    access: RwLock<StoredAccessGrant>,
    refresh_credential: Option<DriveRefreshCredential>,
    resolver: Option<Arc<dyn DriveSecretResolver>>,
    refresh_lock: tokio::sync::Mutex<()>,
}

/// Read-only Drive source over selected items.
pub struct DriveSource {
    spec: PinnedDriveSourceSpec,
    model: SourceModel,
    source_spec: serde_json::Value,
    source_spec_digest: String,
    credentials: DriveCredentialState,
    cursor_key: DriveCursorKey,
    transport: Arc<dyn DriveTransport>,
    traversals: Mutex<BTreeMap<Uuid, QueryTraversal>>,
    configured_membership: DriveMembership,
    membership: Mutex<DriveMembershipState>,
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
    fn attributes(exact_id_lookup: bool) -> Vec<catdb_sdk::connectors::SourceAttr> {
        let id = if exact_id_lookup {
            catdb_sdk::connectors::SourceAttr::text("id").pushes([ObjectFilterOp::Eq])
        } else {
            catdb_sdk::connectors::SourceAttr::text("id")
        };
        vec![
            id,
            catdb_sdk::connectors::SourceAttr::text("name"),
            catdb_sdk::connectors::SourceAttr::text("mimeType"),
            catdb_sdk::connectors::SourceAttr::parsed("modifiedAt", BaseType::Timestamp),
            catdb_sdk::connectors::SourceAttr::parsed("size", BaseType::Int),
            catdb_sdk::connectors::SourceAttr::text("checksum"),
            catdb_sdk::connectors::SourceAttr::text("body"),
        ]
    }

    /// Return the connector-owned schema declaration without resolving or
    /// exposing runtime credentials.
    pub fn declared_model(
        spec: &PinnedDriveSourceSpec,
    ) -> Result<SourceModel, DriveConnectorError> {
        spec.config.validate()?;
        let exact_id_lookup = spec
            .config
            .selections
            .iter()
            .any(|selection| matches!(selection, DriveSelection::Root { .. }));
        Ok(SourceModel::new(
            SourceId::new(spec.config.source_id.clone()),
            &spec.config.catdb_schema,
            &spec.config.catdb_entity,
            Self::attributes(exact_id_lookup),
        ))
    }

    /// Resolve credentials and open with the production HTTPS transport.
    pub async fn open_pinned(
        spec: PinnedDriveSourceSpec,
        resolver: Arc<dyn DriveSecretResolver>,
    ) -> Result<Self, DriveConnectorError> {
        Self::open_with_resolver_and_transport(
            spec,
            resolver,
            Arc::new(ReqwestDriveTransport::new()?),
        )
        .await
    }

    /// Resolve credentials and open with an injected bounded transport.
    pub async fn open_with_resolver_and_transport(
        spec: PinnedDriveSourceSpec,
        resolver: Arc<dyn DriveSecretResolver>,
        transport: Arc<dyn DriveTransport>,
    ) -> Result<Self, DriveConnectorError> {
        spec.config.validate()?;
        let secrets = resolver.resolve(&spec.config.credential_reference).await?;
        Self::construct(spec, secrets, Some(resolver), transport)
    }

    /// Open a pinned source with injected runtime dependencies.
    pub fn open_with_transport(
        spec: PinnedDriveSourceSpec,
        secrets: DriveRuntimeSecrets,
        transport: Arc<dyn DriveTransport>,
    ) -> Result<Self, DriveConnectorError> {
        Self::construct(spec, secrets, None, transport)
    }

    fn construct(
        spec: PinnedDriveSourceSpec,
        secrets: DriveRuntimeSecrets,
        resolver: Option<Arc<dyn DriveSecretResolver>>,
        transport: Arc<dyn DriveTransport>,
    ) -> Result<Self, DriveConnectorError> {
        let model = Self::declared_model(&spec)?;
        let source_spec =
            serde_json::to_value(&spec).map_err(|_| DriveConnectorError::InvalidConfiguration {
                field: "source_spec",
            })?;
        let source_spec_digest = digest(&spec)?;
        let (grant, refresh_credential, cursor_key) = secrets.into_parts();
        let configured_membership = DriveMembership::from_selections(&spec.config.selections)?;
        Ok(Self {
            spec,
            model,
            source_spec,
            source_spec_digest,
            credentials: DriveCredentialState {
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
            traversals: Mutex::new(BTreeMap::new()),
            configured_membership: configured_membership.clone(),
            membership: Mutex::new(DriveMembershipState {
                current: configured_membership,
                next_rebuild_epoch: 0,
                committed_rebuild_epoch: 0,
            }),
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

    /// Download one source-authorized Markdown document and its revision authority.
    pub async fn download_markdown(
        &self,
        record_key: &str,
        context: &ReadContext,
    ) -> Result<DriveDownloadedDocument, DriveDocumentError> {
        context.check()?;
        validate_drive_id_source(self.model.id(), "record_key", record_key)?;
        let mut physical_bytes = 0usize;
        let mut membership = self.configured_membership.clone();
        let metadata = self
            .send(
                self.request_with_membership(self.metadata_path(record_key), &membership)?,
                context,
                &mut physical_bytes,
            )
            .await?;
        if metadata.status != 200 {
            return Err(self.map_document_http_failure(&metadata));
        }
        let file: DriveFile =
            serde_json::from_slice(&metadata.body).map_err(|_| self.malformed("document"))?;
        file.validate(self.model.id())?;
        if file.id != record_key {
            return Err(DriveDocumentError::IdentityMismatch {
                source_id: self.model.id().clone(),
            });
        }
        if !is_markdown_name(&file.name) {
            return Err(self.malformed("document name").into());
        }
        self.verify_point_read_authority(&file, &mut membership, context, &mut physical_bytes)
            .await?;
        membership.record_provider_file(&file, false, self.model.id())?;
        let revision = file.md5_checksum.as_ref().or_else(|| metadata.headers.get("etag"))
            .ok_or_else(|| self.malformed("document revision"))
            .and_then(|value| {
                DriveRevision::new(value.clone()).map_err(|_| self.malformed("document revision"))
            })?;
        let body_path = self
            .body_path(&file)
            .ok_or_else(|| self.malformed("document mime type"))?;
        let mut request = self.request_with_membership(body_path, &membership)?;
        if let Some(etag) = metadata.headers.get("etag") {
            request.headers.insert("if-match".to_string(), etag.clone());
        }
        let body = self
            .send(
                request,
                context,
                &mut physical_bytes,
            )
            .await?;
        if body.status != 200 {
            return Err(self.map_document_http_failure(&body));
        }
        if body.body.len() > MAX_MARKDOWN_BODY_BYTES {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Drive Markdown document",
            }
            .into());
        }
        if std::str::from_utf8(&body.body).is_err() {
            return Err(DriveDocumentError::InvalidUtf8 {
                source_id: self.model.id().clone(),
            });
        }
        Ok(DriveDownloadedDocument {
            record_key: record_key.to_string(),
            revision,
            body: body.body,
        })
    }

    /// Update one Markdown document only when the provider revision still matches.
    pub async fn update_markdown(
        &self,
        record_key: &str,
        revision: &DriveRevision,
        body: &[u8],
        context: &ReadContext,
    ) -> Result<DriveWriteReceipt, DriveDocumentError> {
        context.check()?;
        validate_drive_id_source(self.model.id(), "record_key", record_key)?;
        if body.len() > MAX_MARKDOWN_BODY_BYTES {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Drive Markdown document",
            }
            .into());
        }
        if std::str::from_utf8(body).is_err() {
            return Err(DriveDocumentError::InvalidUtf8 {
                source_id: self.model.id().clone(),
            });
        }
        let mut physical_bytes = 0usize;
        let mut membership = self.configured_membership.clone();
        let metadata = self
            .send(
                self.request_with_membership(self.metadata_path(record_key), &membership)?,
                context,
                &mut physical_bytes,
            )
            .await?;
        if metadata.status != 200 {
            return Err(self.map_document_http_failure(&metadata));
        }
        let file: DriveFile =
            serde_json::from_slice(&metadata.body).map_err(|_| self.malformed("document"))?;
        file.validate(self.model.id())?;
        if file.id != record_key {
            return Err(DriveDocumentError::IdentityMismatch {
                source_id: self.model.id().clone(),
            });
        }
        if !is_markdown_name(&file.name) || file.trashed {
            return Err(self.malformed("document name").into());
        }
        self.verify_point_read_authority(&file, &mut membership, context, &mut physical_bytes)
            .await?;
        membership.record_provider_file(&file, false, self.model.id())?;
        // Catalog/body revisions use content MD5 when available. Only a fresh
        // metadata ETag can guard the HTTP mutation; a checksum is never If-Match.
        let validator = if let Some(checksum) = &file.md5_checksum {
            if checksum != revision.as_str() {
                return Err(DriveDocumentError::Conflict { source_id: self.model.id().clone() });
            }
            DriveRevision::new(metadata.headers.get("etag").ok_or_else(|| self.malformed("conditional write validator"))?.clone())
                .map_err(|_| self.malformed("conditional write validator"))?
        } else {
            revision.clone()
        };
        let authorized_parents = file.parents.iter().cloned().collect::<BTreeSet<_>>();
        let reconciliation = self.write_reconciliation(record_key)?;
        let response = self
            .send_guarded_document_update(
                self.document_update_request(&file.id, &validator, body, &membership)?,
                context,
                &mut physical_bytes,
                reconciliation.clone(),
            )
            .await?;
        if response.status == 412 {
            return Err(DriveDocumentError::Conflict {
                source_id: self.model.id().clone(),
            });
        }
        if response.status != 200 {
            return Err(self.map_document_write_http_failure(&response, reconciliation));
        }
        let file: DriveFile = serde_json::from_slice(&response.body)
            .map_err(|_| self.write_outcome_unknown(reconciliation.clone()))?;
        file.validate(self.model.id())
            .map_err(|_| self.write_outcome_unknown(reconciliation.clone()))?;
        if file.id != record_key {
            return Err(self.write_outcome_unknown(reconciliation));
        }
        if !is_markdown_name(&file.name) || file.trashed {
            return Err(self.write_outcome_unknown(reconciliation));
        }
        if file.parents.iter().cloned().collect::<BTreeSet<_>>() != authorized_parents {
            return Err(self.write_outcome_unknown(reconciliation));
        }
        let revision = file.md5_checksum.as_ref().or_else(|| response.headers.get("etag"))
            .ok_or_else(|| self.write_outcome_unknown(reconciliation.clone()))
            .and_then(|value| {
                DriveRevision::new(value.clone())
                    .map_err(|_| self.write_outcome_unknown(reconciliation.clone()))
            })?;
        Ok(DriveWriteReceipt {
            record_key: record_key.to_string(),
            revision,
        })
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
                self.request(self.change_start_path())?,
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

    /// Reads metadata for one immutable Drive file identifier without fetching
    /// its body. The existing exact-id plan path owns membership checks,
    /// Markdown filtering, identity validation, provider error mapping, and
    /// read-context budgets.
    pub async fn metadata_for_sync(
        &self,
        file_id: &str,
        context: &ReadContext,
    ) -> Result<SourceRow, SourceError> {
        context.check()?;
        let fields = ["id", "name", "mimeType", "modifiedAt", "size", "checksum"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let entity = self
            .model
            .schema()
            .entities
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let [entity] = entity.as_slice() else {
            return Err(SourceError::InvalidPlan {
                source_id: self.model.id().clone(),
            });
        };
        let vocabulary = EntityVocabulary::derive(&[self.model.schema().clone()]);
        let query = catdb_sdk::query::validate_entity_request(
            EntityTarget::Entity(format!("{}.{}", self.model.schema().name, entity)),
            &[EntityFilterInput {
                field: "id".to_string(),
                op: AttrOp::Eq,
                value: file_id.to_string(),
            }],
            &vocabulary,
        )
        .map_err(|_| SourceError::InvalidPlan {
            source_id: self.model.id().clone(),
        })?;
        let plan = self.compile(&query, &fields)?;
        let page = self.execute_page(&plan, None, context).await?;
        let [row] = page.rows.as_slice() else {
            return Err(self.malformed("sync metadata row"));
        };
        if row.source != *self.model.id() || row.key != file_id || page.next_cursor.is_some() {
            return Err(self.malformed("sync metadata identity"));
        }
        Ok(row.clone())
    }

    /// Read one provider change page without collapsing reset or deletion.
    pub async fn read_changes(
        &self,
        cursor: &DriveChangeCursor,
        context: &ReadContext,
    ) -> Result<DriveChangeOutcome, SourceError> {
        context.check()?;
        let token = self.decode_change_cursor(cursor)?;
        let mut physical_bytes = 0usize;
        let response = self
            .send(
                self.request(self.change_list_path(&token, context.page_size()))?,
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
                if self.is_tracked_folder_id(&change.file_id)? {
                    return Ok(DriveChangeOutcome::RebuildRequired);
                }
                events.push(DriveChangeEvent::Deleted {
                    file_id: change.file_id,
                });
                continue;
            }
            let file = change.file.ok_or_else(|| self.malformed("change.file"))?;
            file.validate(self.model.id())?;
            if file.id != change.file_id {
                return Err(self.malformed("change.fileId"));
            }
            if file.mime_type == "application/vnd.google-apps.folder" {
                return Ok(DriveChangeOutcome::RebuildRequired);
            }
            let tracked = self.is_tracked(&file)?;
            if file.trashed || !tracked {
                events.push(DriveChangeEvent::Deleted { file_id: file.id });
            } else {
                events.push(DriveChangeEvent::Upserted { file_id: file.id });
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

    fn request(&self, path: String) -> Result<DriveHttpRequest, SourceError> {
        let headers = self.resource_key_headers()?;
        self.request_with_headers(path, headers)
    }

    fn request_with_membership(
        &self,
        path: String,
        membership: &DriveMembership,
    ) -> Result<DriveHttpRequest, SourceError> {
        let headers = membership.resource_key_headers(self.model.id())?;
        self.request_with_headers(path, headers)
    }

    fn request_with_headers(
        &self,
        path: String,
        headers: BTreeMap<String, String>,
    ) -> Result<DriveHttpRequest, SourceError> {
        self.request_with(DriveHttpMethod::Get, path, headers, Vec::new())
    }

    fn document_update_request(
        &self,
        record_key: &str,
        revision: &DriveRevision,
        body: &[u8],
        membership: &DriveMembership,
    ) -> Result<DriveHttpRequest, SourceError> {
        let mut headers = membership.resource_key_headers(self.model.id())?;
        headers.insert("if-match".to_string(), revision.as_str().to_string());
        self.request_with(
            DriveHttpMethod::Patch,
            encoded_path(
                &format!("/upload/drive/v3/files/{}", encode_path_segment(record_key)),
                vec![
                    ("uploadType", "media".to_string()),
                    ("supportsAllDrives", "true".to_string()),
                    (
                        "fields",
                        "id,name,mimeType,modifiedTime,size,md5Checksum,resourceKey,parents,trashed"
                            .to_string(),
                    ),
                ],
            ),
            headers,
            body.to_vec(),
        )
    }

    fn request_with(
        &self,
        method: DriveHttpMethod,
        path: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<DriveHttpRequest, SourceError> {
        if headers
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
            .map_err(|_| SourceError::Authentication {
                source_id: self.model.id().clone(),
            })?;
        Ok(DriveHttpRequest {
            method,
            path,
            headers,
            body,
            bearer_token: Arc::from(access.grant.token.as_str()),
            credential_generation: access.generation,
        })
    }

    fn reauthorize_request(
        &self,
        request: &DriveHttpRequest,
    ) -> Result<DriveHttpRequest, SourceError> {
        let access = self
            .credentials
            .access
            .read()
            .map_err(|_| SourceError::Authentication {
                source_id: self.model.id().clone(),
            })?;
        Ok(DriveHttpRequest {
            method: request.method,
            path: request.path.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
            bearer_token: Arc::from(access.grant.token.as_str()),
            credential_generation: access.generation,
        })
    }

    async fn send(
        &self,
        mut request: DriveHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
    ) -> Result<DriveHttpResponse, SourceError> {
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
        mut request: DriveHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
        reconciliation: DriveWriteReconciliation,
    ) -> Result<DriveHttpResponse, DriveDocumentError> {
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
        request: DriveHttpRequest,
        context: &ReadContext,
        physical_bytes: &mut usize,
        reconciliation: DriveWriteReconciliation,
    ) -> Result<DriveHttpResponse, DriveDocumentError> {
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

    fn access_token_expired(&self) -> Result<bool, SourceError> {
        self.credentials
            .access
            .read()
            .map(|access| access.grant.is_expired())
            .map_err(|_| SourceError::Authentication {
                source_id: self.model.id().clone(),
            })
    }

    async fn refresh_credentials(
        &self,
        observed_generation: u64,
        forced: bool,
        context: &ReadContext,
    ) -> Result<(), SourceError> {
        let resolver =
            self.credentials
                .resolver
                .as_ref()
                .ok_or_else(|| SourceError::Authentication {
                    source_id: self.model.id().clone(),
                })?;
        let refresh_credential = self
            .credentials
            .refresh_credential
            .as_ref()
            .ok_or_else(|| SourceError::Authentication {
                source_id: self.model.id().clone(),
            })?;
        let deadline = tokio::time::Instant::from_std(context.deadline());
        let refresh_guard = tokio::select! {
            biased;
            _ = context.cancellation().cancelled() => return Err(SourceError::Cancelled),
            result = tokio::time::timeout_at(deadline, self.credentials.refresh_lock.lock()) => {
                result.map_err(|_| SourceError::DeadlineExceeded)?
            }
        };
        {
            let access =
                self.credentials
                    .access
                    .read()
                    .map_err(|_| SourceError::Authentication {
                        source_id: self.model.id().clone(),
                    })?;
            if access.generation != observed_generation || (!forced && !access.grant.is_expired()) {
                drop(refresh_guard);
                return Ok(());
            }
        }
        let refreshed = tokio::select! {
            biased;
            _ = context.cancellation().cancelled() => return Err(SourceError::Cancelled),
            result = tokio::time::timeout_at(
                deadline,
                resolver.refresh(&self.spec.config.credential_reference, refresh_credential),
            ) => {
                result
                    .map_err(|_| SourceError::DeadlineExceeded)?
                    .map_err(|error| self.map_refresh_failure(error))?
            }
        };
        if refreshed.is_expired() {
            return Err(SourceError::Authentication {
                source_id: self.model.id().clone(),
            });
        }
        let mut access =
            self.credentials
                .access
                .write()
                .map_err(|_| SourceError::Authentication {
                    source_id: self.model.id().clone(),
                })?;
        access.generation =
            access
                .generation
                .checked_add(1)
                .ok_or_else(|| SourceError::Authentication {
                    source_id: self.model.id().clone(),
                })?;
        access.grant = refreshed;
        drop(refresh_guard);
        Ok(())
    }

    fn map_refresh_failure(&self, error: DriveConnectorError) -> SourceError {
        match error {
            DriveConnectorError::Transport => SourceError::Transport {
                source_id: self.model.id().clone(),
            },
            DriveConnectorError::RateLimited { retry_after_ms } => SourceError::RateLimited {
                source_id: self.model.id().clone(),
                retry_after_ms,
            },
            DriveConnectorError::InvalidConfiguration { .. }
            | DriveConnectorError::InvalidSecret
            | DriveConnectorError::InvalidPkce
            | DriveConnectorError::InvalidRevision => SourceError::Authentication {
                source_id: self.model.id().clone(),
            },
        }
    }

    fn resource_key_headers(&self) -> Result<BTreeMap<String, String>, SourceError> {
        let membership = self.membership.lock().map_err(|_| SourceError::Transport {
            source_id: self.model.id().clone(),
        })?;
        membership.current.resource_key_headers(self.model.id())
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

    fn map_document_http_failure(&self, response: &DriveHttpResponse) -> DriveDocumentError {
        if response.status == 404 {
            return DriveDocumentError::NotFound {
                source_id: self.model.id().clone(),
            };
        }
        self.map_http_failure(response).into()
    }

    fn map_document_write_http_failure(
        &self,
        response: &DriveHttpResponse,
        reconciliation: DriveWriteReconciliation,
    ) -> DriveDocumentError {
        if matches!(response.status, 408 | 500..=599) {
            return self.write_outcome_unknown(reconciliation);
        }
        self.map_document_http_failure(response)
    }

    fn write_reconciliation(
        &self,
        record_key: &str,
    ) -> Result<DriveWriteReconciliation, SourceError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.cursor_key.as_bytes()).map_err(|_| {
            SourceError::Authentication {
                source_id: self.model.id().clone(),
            }
        })?;
        mac.update(WRITE_RECONCILIATION_DOMAIN);
        mac.update(record_key.as_bytes());
        Ok(DriveWriteReconciliation::new(
            self.model.id().clone(),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()),
        ))
    }

    fn write_outcome_unknown(
        &self,
        reconciliation: DriveWriteReconciliation,
    ) -> DriveDocumentError {
        DriveDocumentError::WriteOutcomeUnknown { reconciliation }
    }

    fn list_path(
        &self,
        folder_id: &str,
        whole_drive: bool,
        page_token: Option<&str>,
        page_size: usize,
    ) -> String {
        let readable = match self.spec.config.content_scope {
            DriveContentScope::AllMetadata => None,
            DriveContentScope::ReadableText => Some(format!(
                "(mimeType = '{GOOGLE_DRIVE_DOCUMENT_MIME}' or mimeType = 'text/markdown' or mimeType = 'text/plain')"
            )),
            DriveContentScope::Markdown => {
                Some("(mimeType = 'text/markdown' or mimeType = 'text/plain')".to_string())
            }
        };
        let query = if whole_drive {
            match readable.as_deref() {
                None => "trashed = false".to_string(),
                Some(readable) => format!("trashed = false and {readable}"),
            }
        } else {
            match readable.as_deref() {
                Some(readable) => format!(
                    "trashed = false and '{}' in parents and (mimeType = '{GOOGLE_DRIVE_FOLDER_MIME}' or {readable})",
                    escape_query_literal(folder_id)
                ),
                None => format!(
                    "trashed = false and '{}' in parents",
                    escape_query_literal(folder_id)
                ),
            }
        };
        let mut params = vec![
            (
                "fields",
                "files(id,name,mimeType,modifiedTime,size,md5Checksum,resourceKey,parents,trashed),nextPageToken,incompleteSearch".to_string(),
            ),
            ("pageSize", page_size.to_string()),
            ("q", query),
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

    fn metadata_path(&self, file_id: &str) -> String {
        encoded_path(
            &format!("/drive/v3/files/{}", encode_path_segment(file_id)),
            vec![
                (
                    "fields",
                    "id,name,mimeType,modifiedTime,size,md5Checksum,resourceKey,parents,trashed"
                        .to_string(),
                ),
                ("supportsAllDrives", "true".to_string()),
            ],
        )
    }

    fn body_path(&self, file: &DriveFile) -> Option<String> {
        let encoded_id = encode_path_segment(&file.id);
        match file.mime_type.as_str() {
            GOOGLE_DRIVE_DOCUMENT_MIME => Some(encoded_path(
                &format!("/drive/v3/files/{encoded_id}/export"),
                vec![("mimeType", "text/plain".to_string())],
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
                "changes(changeType,removed,fileId,file(id,name,mimeType,modifiedTime,size,md5Checksum,resourceKey,parents,trashed)),nextPageToken,newStartPageToken".to_string(),
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

    fn is_tracked(&self, file: &DriveFile) -> Result<bool, SourceError> {
        if self
            .spec
            .config
            .selections
            .iter()
            .any(|selection| matches!(selection, DriveSelection::File { id, .. } if id == &file.id))
        {
            return Ok(self.exposes_file(file));
        }
        if self
            .spec
            .config
            .selections
            .iter()
            .any(|selection| matches!(selection, DriveSelection::Root { .. }))
        {
            return Ok(self.exposes_file(file));
        }
        let membership = self.membership.lock().map_err(|_| SourceError::Transport {
            source_id: self.model.id().clone(),
        })?;
        Ok(file
            .parents
            .iter()
            .any(|parent| membership.current.tracked_folders.contains(parent))
            && self.exposes_file(file))
    }

    fn exposes_file(&self, file: &DriveFile) -> bool {
        match self.spec.config.content_scope {
            DriveContentScope::AllMetadata => true,
            DriveContentScope::ReadableText => matches!(
                file.mime_type.as_str(),
                GOOGLE_DRIVE_DOCUMENT_MIME | "text/markdown" | "text/plain"
            ),
            DriveContentScope::Markdown => {
                matches!(file.mime_type.as_str(), "text/markdown" | "text/plain")
                    && is_markdown_name(&file.name)
            }
        }
    }

    fn is_tracked_folder_id(&self, file_id: &str) -> Result<bool, SourceError> {
        self.membership
            .lock()
            .map(|membership| membership.current.tracked_folders.contains(file_id))
            .map_err(|_| SourceError::Transport {
                source_id: self.model.id().clone(),
            })
    }

    fn validate_plan(&self, plan: &SourcePlan) -> Result<(), SourceError> {
        if plan.source != *self.model.id() || self.exact_id_filter(plan).is_err() {
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

    fn exact_id_filter<'a>(&self, plan: &'a SourcePlan) -> Result<Option<&'a str>, ()> {
        match plan.pushed.filters.as_slice() {
            [] => Ok(None),
            [filter] if filter.attr == "id" && filter.op == ObjectFilterOp::Eq => {
                match &filter.value {
                    ObjectValue::Text(id) => Ok(Some(id.as_str())),
                    _ => Err(()),
                }
            }
            _ => Err(()),
        }
    }

    async fn verify_point_read_authority(
        &self,
        file: &DriveFile,
        membership: &mut DriveMembership,
        context: &ReadContext,
        physical_bytes: &mut usize,
    ) -> Result<(), SourceError> {
        if self
            .spec
            .config
            .selections
            .iter()
            .any(|selection| matches!(selection, DriveSelection::File { id, .. } if id == &file.id))
        {
            return Ok(());
        }
        let selected_ancestors = self
            .spec
            .config
            .selections
            .iter()
            .filter_map(|selection| match selection {
                DriveSelection::Folder { id, .. } | DriveSelection::Root { id, .. } => {
                    Some(id.as_str())
                }
                DriveSelection::File { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        if selected_ancestors.is_empty() {
            return Err(SourceError::InvalidPlan {
                source_id: self.model.id().clone(),
            });
        }

        let mut pending = file
            .parents
            .iter()
            .cloned()
            .map(|parent| (parent, 0_usize))
            .collect::<VecDeque<_>>();
        let mut seen = BTreeSet::from([file.id.clone()]);
        while let Some((candidate, depth)) = pending.pop_front() {
            context.check()?;
            if selected_ancestors.contains(candidate.as_str()) {
                return Ok(());
            }
            if depth >= MAX_RECURSION_DEPTH || !seen.insert(candidate.clone()) {
                continue;
            }
            if seen.len() > MAX_POINT_READ_ANCESTRY_ITEMS {
                return Err(SourceError::ResourceLimit {
                    source_id: self.model.id().clone(),
                    resource: "Drive point-read ancestry",
                });
            }
            let response = self
                .send(
                    self.request_with_membership(self.metadata_path(&candidate), membership)?,
                    context,
                    physical_bytes,
                )
                .await?;
            if response.status == 404 {
                continue;
            }
            if response.status != 200 {
                return Err(self.map_http_failure(&response));
            }
            let parent: DriveFile = serde_json::from_slice(&response.body)
                .map_err(|_| self.malformed("point-read parent"))?;
            parent.validate(self.model.id())?;
            if parent.id != candidate
                || parent.trashed
                || parent.mime_type != GOOGLE_DRIVE_FOLDER_MIME
            {
                return Err(self.malformed("point-read parent"));
            }
            membership.record_provider_file(&parent, true, self.model.id())?;
            let next_depth = depth.checked_add(1).ok_or(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "Drive point-read ancestry",
            })?;
            pending.extend(
                parent
                    .parents
                    .iter()
                    .cloned()
                    .map(|ancestor| (ancestor, next_depth)),
            );
        }
        Err(SourceError::Read {
            source_id: self.model.id().clone(),
            path: "selected Drive item".to_string(),
            detail: "provider item is outside the configured Drive selections".to_string(),
        })
    }

    fn encode_query_cursor(
        &self,
        plan: &SourcePlan,
        query_id: Uuid,
        traversal_id: Uuid,
        generation: u64,
    ) -> Result<SourceCursor, SourceError> {
        let payload = QueryCursorPayload {
            version: CURSOR_VERSION,
            connector: "catdb-google-drive-source/v1".to_string(),
            source_spec_digest: self.source_spec_digest.clone(),
            schema_version: self.spec.schema_version.clone(),
            plan_digest: plan_digest(plan)?,
            query_id,
            traversal_id,
            generation,
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
    ) -> Result<(Uuid, u64), SourceError> {
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
        Ok((envelope.payload.traversal_id, envelope.payload.generation))
    }

    fn selected_file_ids(&self) -> Vec<&str> {
        self.spec
            .config
            .selections
            .iter()
            .filter_map(|selection| match selection {
                DriveSelection::File { id, .. } => Some(id.as_str()),
                DriveSelection::Folder { .. } | DriveSelection::Root { .. } => None,
            })
            .collect()
    }

    fn selected_folder_ids(&self) -> Vec<(&str, bool)> {
        self.spec
            .config
            .selections
            .iter()
            .filter_map(|selection| match selection {
                DriveSelection::Folder { id, .. } => Some((id.as_str(), false)),
                DriveSelection::Root { id, .. } => Some((id.as_str(), true)),
                DriveSelection::File { .. } => None,
            })
            .collect()
    }

    fn begin_query_traversal(
        &self,
        query_id: Uuid,
        expires_at: Instant,
    ) -> Result<QueryTraversal, SourceError> {
        let mut traversals = self.traversals.lock().map_err(|_| SourceError::Transport {
            source_id: self.model.id().clone(),
        })?;
        let now = Instant::now();
        traversals.retain(|_, traversal| traversal.expires_at > now);
        if traversals.contains_key(&query_id) {
            return Err(SourceError::InvalidCursor);
        }
        if traversals.len() >= MAX_ACTIVE_TRAVERSALS {
            return Err(SourceError::ResourceLimit {
                source_id: self.model.id().clone(),
                resource: "active Drive traversals",
            });
        }
        let membership_epoch = {
            let mut membership = self.membership.lock().map_err(|_| SourceError::Transport {
                source_id: self.model.id().clone(),
            })?;
            membership.next_rebuild_epoch =
                membership
                    .next_rebuild_epoch
                    .checked_add(1)
                    .ok_or(SourceError::ResourceLimit {
                        source_id: self.model.id().clone(),
                        resource: "Drive membership rebuild epoch",
                    })?;
            membership.next_rebuild_epoch
        };
        Ok(QueryTraversal {
            traversal_id: Uuid::new_v4(),
            generation: 0,
            expires_at,
            membership_epoch,
            membership: self.configured_membership.clone(),
            selected_file_index: 0,
            selected_folder_index: 0,
            folders: VecDeque::new(),
            seen_ids: BTreeSet::new(),
        })
    }

    fn resume_query_traversal(
        &self,
        query_id: Uuid,
        traversal_id: Uuid,
        generation: u64,
    ) -> Result<QueryTraversal, SourceError> {
        let mut traversals = self.traversals.lock().map_err(|_| SourceError::Transport {
            source_id: self.model.id().clone(),
        })?;
        let traversal = traversals
            .get(&query_id)
            .ok_or(SourceError::InvalidCursor)?;
        if traversal.traversal_id != traversal_id || traversal.generation != generation {
            return Err(SourceError::InvalidCursor);
        }
        if traversal.expires_at <= Instant::now() {
            traversals.remove(&query_id);
            return Err(SourceError::DeadlineExceeded);
        }
        traversals
            .remove(&query_id)
            .ok_or(SourceError::InvalidCursor)
    }

    fn save_query_traversal(
        &self,
        query_id: Uuid,
        traversal: QueryTraversal,
    ) -> Result<(), SourceError> {
        let mut traversals = self.traversals.lock().map_err(|_| SourceError::Transport {
            source_id: self.model.id().clone(),
        })?;
        if traversals.insert(query_id, traversal).is_some() {
            return Err(SourceError::InvalidCursor);
        }
        Ok(())
    }

    fn commit_membership(
        &self,
        rebuild_epoch: u64,
        rebuilt: DriveMembership,
    ) -> Result<(), SourceError> {
        let mut membership = self.membership.lock().map_err(|_| SourceError::Transport {
            source_id: self.model.id().clone(),
        })?;
        if rebuild_epoch > membership.committed_rebuild_epoch {
            membership.current = rebuilt;
            membership.committed_rebuild_epoch = rebuild_epoch;
        }
        Ok(())
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
        if bytes.len() > catdb_sdk::connectors::MAX_SOURCE_CURSOR_BYTES {
            return Err(SourceError::CursorTooLarge {
                actual: bytes.len(),
                maximum: catdb_sdk::connectors::MAX_SOURCE_CURSOR_BYTES,
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

    async fn observe_file(
        &self,
        file: DriveFile,
        membership: &DriveMembership,
        plan: &SourcePlan,
        context: &ReadContext,
        physical_bytes: &mut usize,
    ) -> Result<Option<SourceRow>, SourceError> {
        context.check()?;
        file.validate(self.model.id())?;
        if file.trashed {
            return Ok(None);
        }
        if !self.exposes_file(&file) {
            return Ok(None);
        }
        let needs_body = plan
            .projected
            .iter()
            .map(String::as_str)
            .chain(plan.residual_attrs())
            .any(|field| field == "body");
        let mut value = file.to_value(self.model.id())?;
        if needs_body {
            if let Some(path) = self.body_path(&file) {
                let body = self
                    .send(
                        self.request_with_membership(path, membership)?,
                        context,
                        physical_bytes,
                    )
                    .await?;
                if body.status != 200 {
                    return Err(self.map_http_failure(&body));
                }
                let text =
                    String::from_utf8(body.body).map_err(|_| self.malformed("body encoding"))?;
                value
                    .as_object_mut()
                    .ok_or_else(|| self.malformed("file"))?
                    .insert("body".to_string(), serde_json::Value::String(text));
            }
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
            key: file.id,
            value: serde_json::Value::Object(projected),
        }))
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
        if let Some(expected_id) =
            self.exact_id_filter(plan)
                .map_err(|_| SourceError::InvalidPlan {
                    source_id: self.model.id().clone(),
                })?
        {
            if cursor.is_some() {
                return Err(SourceError::InvalidCursor);
            }
            let mut physical_bytes = 0usize;
            let mut membership = self.configured_membership.clone();
            let response = self
                .send(
                    self.request_with_membership(self.metadata_path(expected_id), &membership)?,
                    context,
                    &mut physical_bytes,
                )
                .await?;
            if response.status == 404 {
                return Err(SourceError::Read {
                    source_id: self.model.id().clone(),
                    path: "selected Drive item".to_string(),
                    detail: "provider returned 404; deletion and lost authorization are ambiguous"
                        .to_string(),
                });
            }
            if response.status != 200 {
                return Err(self.map_http_failure(&response));
            }
            let file: DriveFile =
                serde_json::from_slice(&response.body).map_err(|_| self.malformed("file"))?;
            file.validate(self.model.id())?;
            if file.id != expected_id {
                return Err(self.malformed("selected file identity"));
            }
            if file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
                return Err(self.malformed("selected file kind"));
            }
            self.verify_point_read_authority(&file, &mut membership, context, &mut physical_bytes)
                .await?;
            membership.record_provider_file(&file, false, self.model.id())?;
            let row = self
                .observe_file(file, &membership, plan, context, &mut physical_bytes)
                .await?;
            return Ok(SourcePage {
                source: self.model.id().clone(),
                scanned: 1,
                rows: row.into_iter().collect(),
                next_cursor: None,
                observed_at_ms: now_ms(),
                freshness_ms: 0,
            });
        }
        let mut traversal = if let Some(cursor) = cursor {
            let (traversal_id, generation) =
                self.decode_query_cursor(plan, cursor, context.query_id())?;
            self.resume_query_traversal(context.query_id(), traversal_id, generation)?
        } else {
            self.begin_query_traversal(context.query_id(), context.deadline())?
        };
        let mut physical_bytes = 0usize;
        let selected_file_ids = self.selected_file_ids();
        let selected_folder_ids = self.selected_folder_ids();
        let mut scanned = 0usize;
        let mut rows = Vec::with_capacity(page_size);
        while traversal.selected_file_index < selected_file_ids.len() && scanned < page_size {
            context.check()?;
            let expected_id = selected_file_ids[traversal.selected_file_index];
            let response = self
                .send(
                    self.request_with_membership(
                        self.metadata_path(expected_id),
                        &traversal.membership,
                    )?,
                    context,
                    &mut physical_bytes,
                )
                .await?;
            scanned += 1;
            traversal.selected_file_index += 1;
            if response.status == 404 {
                return Err(SourceError::Read {
                    source_id: self.model.id().clone(),
                    path: "selected Drive item".to_string(),
                    detail: "provider returned 404; deletion and lost authorization are ambiguous"
                        .to_string(),
                });
            }
            if response.status != 200 {
                return Err(self.map_http_failure(&response));
            }
            let file: DriveFile =
                serde_json::from_slice(&response.body).map_err(|_| self.malformed("file"))?;
            file.validate(self.model.id())?;
            if file.id != expected_id {
                return Err(self.malformed("selected file identity"));
            }
            if file.mime_type == "application/vnd.google-apps.folder" {
                return Err(self.malformed("selected file kind"));
            }
            traversal
                .membership
                .record_provider_file(&file, false, self.model.id())?;
            traversal.seen_ids.insert(file.id.clone());
            if let Some(row) = self
                .observe_file(
                    file,
                    &traversal.membership,
                    plan,
                    context,
                    &mut physical_bytes,
                )
                .await?
            {
                rows.push(row);
            }
        }

        let mut empty_provider_pages = 0usize;
        while traversal.selected_file_index == selected_file_ids.len() && scanned < page_size {
            context.check()?;
            while traversal.folders.is_empty()
                && traversal.selected_folder_index < selected_folder_ids.len()
            {
                let (root, whole_drive) = selected_folder_ids[traversal.selected_folder_index];
                let response = self
                    .send(
                        self.request_with_membership(
                            self.metadata_path(root),
                            &traversal.membership,
                        )?,
                        context,
                        &mut physical_bytes,
                    )
                    .await?;
                if response.status == 404 {
                    return Err(SourceError::Read {
                        source_id: self.model.id().clone(),
                        path: "selected Drive folder".to_string(),
                        detail:
                            "provider returned 404; deletion and lost authorization are ambiguous"
                                .to_string(),
                    });
                }
                if response.status != 200 {
                    return Err(self.map_http_failure(&response));
                }
                let folder: DriveFile = serde_json::from_slice(&response.body)
                    .map_err(|_| self.malformed("selected folder"))?;
                folder.validate(self.model.id())?;
                if folder.id != root {
                    return Err(self.malformed("selected folder identity"));
                }
                if folder.trashed {
                    return Err(SourceError::Read {
                        source_id: self.model.id().clone(),
                        path: "selected Drive folder".to_string(),
                        detail: "provider reports the selected folder is trashed".to_string(),
                    });
                }
                if folder.mime_type != "application/vnd.google-apps.folder" {
                    return Err(self.malformed("selected folder kind"));
                }
                traversal
                    .membership
                    .record_provider_file(&folder, true, self.model.id())?;
                traversal.selected_folder_index += 1;
                if traversal.seen_ids.insert(root.to_string()) {
                    traversal.folders.push_back(FolderTraversal {
                        id: root.to_string(),
                        depth: 0,
                        page_token: None,
                        whole_drive,
                    });
                }
            }
            let Some(current) = traversal.folders.front() else {
                break;
            };
            let folder_id = current.id.clone();
            let folder_depth = current.depth;
            let provider_page_token = current.page_token.clone();
            let whole_drive = current.whole_drive;
            let remaining = page_size - scanned;
            let response = self
                .send(
                    self.request_with_membership(
                        self.list_path(
                            &folder_id,
                            whole_drive,
                            provider_page_token.as_deref(),
                            remaining,
                        ),
                        &traversal.membership,
                    )?,
                    context,
                    &mut physical_bytes,
                )
                .await?;
            if response.status == 400 && provider_page_token.is_some() {
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
            if list.files.len() > remaining {
                return Err(SourceError::ResourceLimit {
                    source_id: self.model.id().clone(),
                    resource: "Drive page rows",
                });
            }
            if let Some(next_page_token) = &list.next_page_token {
                validate_provider_token(next_page_token)?;
            }
            scanned += list.files.len();
            for file in list.files {
                context.check()?;
                file.validate(self.model.id())?;
                if !whole_drive && !file.parents.iter().any(|parent| parent == &folder_id) {
                    return Err(self.malformed("parents"));
                }
                let is_folder = file.mime_type == GOOGLE_DRIVE_FOLDER_MIME;
                traversal
                    .membership
                    .record_provider_file(&file, is_folder, self.model.id())?;
                if !traversal.seen_ids.insert(file.id.clone()) {
                    continue;
                }
                if traversal.seen_ids.len() > MAX_TRAVERSAL_ITEMS {
                    return Err(SourceError::ResourceLimit {
                        source_id: self.model.id().clone(),
                        resource: "Drive traversal items",
                    });
                }
                if is_folder {
                    let depth = folder_depth
                        .checked_add(1)
                        .ok_or(SourceError::ResourceLimit {
                            source_id: self.model.id().clone(),
                            resource: "Drive recursion depth",
                        })?;
                    if depth > MAX_RECURSION_DEPTH {
                        return Err(SourceError::ResourceLimit {
                            source_id: self.model.id().clone(),
                            resource: "Drive recursion depth",
                        });
                    }
                    traversal.folders.push_back(FolderTraversal {
                        id: file.id,
                        depth,
                        page_token: None,
                        whole_drive: false,
                    });
                    if traversal.folders.len() > MAX_TRAVERSAL_ITEMS {
                        return Err(SourceError::ResourceLimit {
                            source_id: self.model.id().clone(),
                            resource: "Drive folder queue",
                        });
                    }
                    continue;
                }
                if let Some(row) = self
                    .observe_file(
                        file,
                        &traversal.membership,
                        plan,
                        context,
                        &mut physical_bytes,
                    )
                    .await?
                {
                    rows.push(row);
                }
            }
            if let Some(next_page_token) = list.next_page_token {
                let current = traversal
                    .folders
                    .front_mut()
                    .ok_or(SourceError::InvalidCursor)?;
                current.page_token = Some(next_page_token);
            } else {
                traversal.folders.pop_front();
            }
            if scanned == 0 {
                empty_provider_pages += 1;
                if empty_provider_pages > MAX_EMPTY_PROVIDER_PAGES {
                    return Err(SourceError::ResourceLimit {
                        source_id: self.model.id().clone(),
                        resource: "empty Drive continuation pages",
                    });
                }
                continue;
            }
            break;
        }

        let has_more = traversal.selected_file_index < selected_file_ids.len()
            || traversal.selected_folder_index < selected_folder_ids.len()
            || !traversal.folders.is_empty();
        let next_cursor = if has_more {
            if scanned == 0 {
                return Err(SourceError::InvalidCursor);
            }
            traversal.generation = traversal
                .generation
                .checked_add(1)
                .ok_or(SourceError::InvalidCursor)?;
            let next = self.encode_query_cursor(
                plan,
                context.query_id(),
                traversal.traversal_id,
                traversal.generation,
            )?;
            self.save_query_traversal(context.query_id(), traversal)?;
            Some(next)
        } else {
            self.commit_membership(traversal.membership_epoch, traversal.membership)?;
            None
        };
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
    resource_key: Option<String>,
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
        if self
            .resource_key
            .as_ref()
            .is_some_and(|resource_key| !resource_key_bytes_are_valid(resource_key))
        {
            return Err(malformed_source(source_id, "resourceKey"));
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
    /// Runtime secret resolution or refresh was rate limited.
    #[error("Google Drive runtime secret operation was rate limited")]
    RateLimited {
        /// Provider-requested wait, when available.
        retry_after_ms: Option<u64>,
    },
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

fn validate_resource_key(resource_key: &str) -> Result<(), DriveConnectorError> {
    if !resource_key_bytes_are_valid(resource_key) {
        return Err(DriveConnectorError::InvalidConfiguration {
            field: "resource_key",
        });
    }
    Ok(())
}

fn resource_key_bytes_are_valid(resource_key: &str) -> bool {
    !resource_key.is_empty()
        && resource_key.len() <= MAX_RESOURCE_KEY_BYTES
        && resource_key
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b',' | b'/'))
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

fn is_markdown_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
    })
}

fn contains_control(value: &str) -> bool {
    value.chars().any(char::is_control)
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
                    if matches!(name.as_ref(), "pageToken" | "q" | "driveId") {
                        (name.into_owned(), "[redacted]".to_string())
                    } else {
                        (name.into_owned(), value.into_owned())
                    }
                })
                .collect::<Vec<_>>();
            url.set_query(None);
            url.query_pairs_mut().extend_pairs(pairs);
            let path = if let Some(rest) = url
                .path()
                .strip_prefix("/drive/v3/files/")
                .or_else(|| url.path().strip_prefix("/upload/drive/v3/files/"))
            {
                let suffix = rest
                    .find('/')
                    .map(|index| &rest[index..])
                    .unwrap_or_default();
                let prefix = if url.path().starts_with("/upload/") {
                    "/upload/drive/v3/files"
                } else {
                    "/drive/v3/files"
                };
                format!("{prefix}/[redacted-id]{suffix}")
            } else {
                url.path().to_string()
            };
            format!(
                "{}{}",
                path,
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

#[cfg(test)]
mod cursor_binding_tests {
    use super::*;
    use catdb_sdk::query::EntityTarget;

    struct NeverTransport;

    #[async_trait]
    impl DriveTransport for NeverTransport {
        async fn send(
            &self,
            _request: DriveHttpRequest,
        ) -> Result<DriveHttpResponse, DriveTransportError> {
            Err(DriveTransportError::Failed)
        }
    }

    fn source(schema_version: &str) -> DriveSource {
        let config = DriveSourceConfig::new(
            "drive-library",
            "drive",
            "Document",
            vec![DriveSelection::folder("selected-folder")],
            DriveSecretReference::new("keychain/mdreader/drive").unwrap(),
        )
        .unwrap();
        DriveSource::open_with_transport(
            PinnedDriveSourceSpec::new(config, schema_version).unwrap(),
            DriveRuntimeSecrets::new(
                DriveAccessToken::new("fixture-access-token").unwrap(),
                DriveCursorKey::new([0x41; 32]),
            ),
            Arc::new(NeverTransport),
        )
        .unwrap()
    }

    fn query_plan(source: &DriveSource, field: &str) -> SourcePlan {
        source
            .compile(
                &EntityQuery {
                    target: EntityTarget::Entity("drive.Document".to_string()),
                    filters: Vec::new(),
                },
                &[field.to_string()],
            )
            .unwrap()
    }

    #[test]
    fn query_cursor_binding_rejects_each_independent_dimension() {
        let mut source = source("drive-schema-v1");
        let query_id = Uuid::new_v4();
        let plan = query_plan(&source, "id");
        let cursor = source
            .encode_query_cursor(&plan, query_id, Uuid::new_v4(), 1)
            .unwrap();

        assert_eq!(
            source.decode_query_cursor(&query_plan(&source, "name"), &cursor, query_id),
            Err(SourceError::InvalidCursor)
        );
        assert_eq!(
            source.decode_query_cursor(&plan, &cursor, Uuid::new_v4()),
            Err(SourceError::InvalidCursor)
        );

        let original_digest = source.source_spec_digest.clone();
        source.source_spec_digest = "different-source-spec".to_string();
        assert_eq!(
            source.decode_query_cursor(&plan, &cursor, query_id),
            Err(SourceError::InvalidCursor)
        );
        source.source_spec_digest = original_digest;

        source.spec.schema_version = "drive-schema-v2".to_string();
        assert_eq!(
            source.decode_query_cursor(&plan, &cursor, query_id),
            Err(SourceError::InvalidCursor)
        );

        source.spec.schema_version = "drive-schema-v1".to_string();
        let payload = QueryCursorPayload {
            version: CURSOR_VERSION,
            connector: "not-a-query-cursor".to_string(),
            source_spec_digest: source.source_spec_digest.clone(),
            schema_version: source.spec.schema_version.clone(),
            plan_digest: plan_digest(&plan).unwrap(),
            query_id,
            traversal_id: Uuid::new_v4(),
            generation: 1,
        };
        let authentication_tag = source.authenticate(QUERY_CURSOR_DOMAIN, &payload).unwrap();
        let wrong_kind = SourceCursor::new(
            serde_json::to_vec(&CursorEnvelope {
                payload,
                authentication_tag,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            source.decode_query_cursor(&plan, &wrong_kind, query_id),
            Err(SourceError::InvalidCursor)
        );
    }

    #[test]
    fn change_cursor_binding_rejects_source_schema_and_kind_independently() {
        let mut source = source("drive-schema-v1");
        let cursor = source.encode_change_cursor("changes-100").unwrap();

        let original_digest = source.source_spec_digest.clone();
        source.source_spec_digest = "different-source-spec".to_string();
        assert_eq!(
            source.decode_change_cursor(&cursor),
            Err(SourceError::InvalidCursor)
        );
        source.source_spec_digest = original_digest;

        source.spec.schema_version = "drive-schema-v2".to_string();
        assert_eq!(
            source.decode_change_cursor(&cursor),
            Err(SourceError::InvalidCursor)
        );

        source.spec.schema_version = "drive-schema-v1".to_string();
        let payload = ChangeCursorPayload {
            version: CURSOR_VERSION,
            connector: "not-a-change-cursor".to_string(),
            source_spec_digest: source.source_spec_digest.clone(),
            schema_version: source.spec.schema_version.clone(),
            page_token: "changes-100".to_string(),
        };
        let authentication_tag = source.authenticate(CHANGE_CURSOR_DOMAIN, &payload).unwrap();
        let wrong_kind = DriveChangeCursor(
            serde_json::to_vec(&CursorEnvelope {
                payload,
                authentication_tag,
            })
            .unwrap(),
        );
        assert_eq!(
            source.decode_change_cursor(&wrong_kind),
            Err(SourceError::InvalidCursor)
        );
    }
}

#[cfg(test)]
mod membership_transaction_tests {
    use super::*;

    #[test]
    fn header_bound_failure_leaves_the_staged_membership_unchanged() {
        let source_id = SourceId::new("drive-library");
        let mut membership =
            DriveMembership::from_selections(&[DriveSelection::folder("selected-folder")]).unwrap();
        let mut observed_bound = false;
        for index in 0..20 {
            let before_folders = membership.tracked_folders.clone();
            let before_keys = membership.resource_keys.clone();
            let before_header_bytes = membership.resource_key_header_bytes;
            let file = DriveFile {
                id: format!("{}{:02}", "f".repeat(248), index),
                name: format!("Folder {index}"),
                mime_type: "application/vnd.google-apps.folder".to_string(),
                modified_time: "2026-07-20T12:34:56Z".to_string(),
                size: None,
                md5_checksum: None,
                resource_key: Some("k".repeat(500)),
                parents: vec!["selected-folder".to_string()],
                trashed: false,
            };
            match membership.record_provider_file(&file, true, &source_id) {
                Ok(()) => {}
                Err(SourceError::ResourceLimit {
                    resource: "Drive resource-key header",
                    ..
                }) => {
                    assert_eq!(membership.tracked_folders, before_folders);
                    assert_eq!(membership.resource_keys, before_keys);
                    assert_eq!(membership.resource_key_header_bytes, before_header_bytes);
                    observed_bound = true;
                    break;
                }
                Err(error) => panic!("unexpected membership failure: {error}"),
            }
        }
        assert!(observed_bound);
    }
}
