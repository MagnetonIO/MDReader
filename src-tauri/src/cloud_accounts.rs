//! Native cloud-account ownership for MDReader document sources.
//!
//! Provider files remain authoritative. This module persists only non-secret
//! source membership and keeps OAuth material behind an operating-system
//! credential-vault boundary. Pausing excludes a source from reads without
//! revoking it; disconnecting removes only MDReader-managed state.

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use url::{Position, Url};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{file_lock::AdvisoryFileLock, library::LibraryStore};
use catdb_api_types::SourceExecution;

const CLOUD_REGISTRY_VERSION: u32 = 1;
const CLOUD_LIFECYCLE_LOCK_FILE: &str = "cloud-accounts.lock";
const MAX_CLOUD_SOURCES: usize = 2;
const MAX_PENDING_CREDENTIAL_CLEANUPS: usize = 16;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const MAX_LIBRARY_CONTINUATIONS: usize = 8;
const MAX_CALLBACK_BYTES: usize = 16 * 1024;
const MAX_CLOUD_REQUEST_ID_BYTES: usize = 128;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const MAX_AUTHORIZATION_CODE_BYTES: usize = 4 * 1024;
const MAX_DRIVE_SELECTIONS: usize = 512;
const MAX_TOKEN_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_DRIVE_METADATA_BYTES: usize = 64 * 1024;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const MAX_CLOUD_QUERY_FILTERS: usize = 16;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const MAX_CLOUD_QUERY_FIELDS: usize = 16;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_QUERY_WORK_BUDGET: usize = 10_000;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_QUERY_BYTE_BUDGET: usize = 16 * 1024 * 1024;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_QUERY_PAGE_SIZE: usize = 100;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_QUERY_RESPONSE_BYTE_BUDGET: usize = 16 * 1024 * 1024;
const CLOUD_SYNC_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
#[cfg(feature = "e2e-build")]
const E2E_DRIVE_RECORD_ID: &str =
    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
#[cfg(feature = "e2e-build")]
const E2E_DRIVE_CREDENTIAL_REFERENCE: &str = "mdreader-e2e-google-drive";
#[cfg(feature = "e2e-build")]
const E2E_DRIVE_FILE_ID: &str = "mdreader-e2e-drive-document";
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_QUERY_DEADLINE: Duration = Duration::from_secs(45);
pub(crate) const CLOUD_DOCUMENT_MAPPING_VERSION: &str = "mdreader.Document.cloud-mapping/v2";
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_READ_POLICY_VERSION: &str = "mdreader.cloud-read/v2";
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_MAPPING_VERSION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_6d_61_70_5f_01_00);
const CLOUD_PHYSICAL_SCHEMA_VERSION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_73_63_68_5f_01_00);
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_SOURCE_VERSION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_73_72_63_5f_01_00);
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_POLICY_VERSION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_70_6f_6c_5f_01_00);
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_PROVENANCE_RULE_VERSION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_70_72_76_5f_01_00);
const CLOUD_CATALOG_REVISION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_63_61_74_5f_01_00);
const CLOUD_WORKSPACE_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_77_73_70_5f_01_00);
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_CATALOG_RESOLVER_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_72_73_6c_5f_01_00);
const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const DROPBOX_TOKEN_ENDPOINT: &str = "https://api.dropbox.com/oauth2/token";
const GOOGLE_DRIVE_FILES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/files/";
const GOOGLE_DRIVE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudProvider {
    GoogleDrive,
    Dropbox,
}

/// Versioned native OAuth authority retained with each cloud source record.
///
/// Legacy records intentionally default to V1 so a persisted account never
/// gains document-write authority merely by loading an older registry.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudAuthorizationProfile {
    /// The original read-only connection capability.
    #[default]
    ReadOnlyV1,
    /// The explicit Markdown read/write consent capability.
    MarkdownReadWriteV2,
}

/// Native-only declaration of a provider's completed Markdown write gate.
///
/// A write-capable OAuth profile does not imply that the provider's guarded
/// mutation path has passed its live acceptance gate. Task 9 changes only the
/// matching provider declaration after that gate succeeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeMarkdownWriteAcceptance {
    /// The provider mutation remains unavailable until its live gate passes.
    Pending,
    /// The provider mutation has passed its provider-specific live gate.
    Accepted,
}

const GOOGLE_DRIVE_MARKDOWN_WRITE_ACCEPTANCE: NativeMarkdownWriteAcceptance =
    NativeMarkdownWriteAcceptance::Accepted;
const DROPBOX_MARKDOWN_WRITE_ACCEPTANCE: NativeMarkdownWriteAcceptance =
    NativeMarkdownWriteAcceptance::Pending;

fn native_markdown_write_acceptance(provider: CloudProvider) -> NativeMarkdownWriteAcceptance {
    match provider {
        CloudProvider::GoogleDrive => GOOGLE_DRIVE_MARKDOWN_WRITE_ACCEPTANCE,
        CloudProvider::Dropbox => DROPBOX_MARKDOWN_WRITE_ACCEPTANCE,
    }
}

/// Editing readiness derived from the persisted authorization profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloudEditingCapability {
    /// The connected account must reconnect with the V2 OAuth profile.
    ReconnectRequired,
    /// Provider mutation is deliberately held pending the Task 9 live gate.
    WriteCapabilityPendingAcceptance,
}

fn requested_scopes(
    provider: CloudProvider,
    profile: CloudAuthorizationProfile,
) -> &'static [&'static str] {
    match (provider, profile) {
        (CloudProvider::GoogleDrive, CloudAuthorizationProfile::ReadOnlyV1) => {
            &["https://www.googleapis.com/auth/drive.readonly"]
        }
        (CloudProvider::GoogleDrive, CloudAuthorizationProfile::MarkdownReadWriteV2) => {
            &["https://www.googleapis.com/auth/drive"]
        }
        (CloudProvider::Dropbox, CloudAuthorizationProfile::ReadOnlyV1) => {
            &["files.metadata.read", "files.content.read"]
        }
        (CloudProvider::Dropbox, CloudAuthorizationProfile::MarkdownReadWriteV2) => &[
            "files.metadata.read",
            "files.content.read",
            "files.content.write",
        ],
    }
}

fn validated_authorization_profile(
    provider: CloudProvider,
    requested: CloudAuthorizationProfile,
    returned_scope: Option<&str>,
) -> Result<CloudAuthorizationProfile, CloudSourceError> {
    let Some(returned_scope) = returned_scope else {
        // Providers may omit unchanged scope. Authority remains exactly the
        // native request profile; omission can never upgrade a V1 record.
        return Ok(requested);
    };
    let returned = returned_scope.split_whitespace().collect::<HashSet<_>>();
    let expected = requested_scopes(provider, requested)
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    // The provider may return other scopes already granted to this application.
    // Those scopes never widen MDReader's persisted profile or connector
    // capabilities. Reject only when the token is missing authority MDReader
    // explicitly requested.
    if expected.is_subset(&returned) {
        Ok(requested)
    } else {
        let mut missing = expected.difference(&returned).copied().collect::<Vec<_>>();
        missing.sort_unstable();
        eprintln!(
            "MDReader OAuth scope validation rejected provider={} missing={}",
            provider.id(),
            missing.join(",")
        );
        Err(CloudSourceError::OauthTokenRejected)
    }
}

impl CloudProvider {
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    fn id(self) -> &'static str {
        match self {
            Self::GoogleDrive => "google-drive",
            Self::Dropbox => "dropbox",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::GoogleDrive => "Google Drive",
            Self::Dropbox => "Dropbox",
        }
    }

    fn credential_cleanup_reference(self) -> &'static str {
        match self {
            Self::GoogleDrive => "cloud-cleanup-google-drive",
            Self::Dropbox => "cloud-cleanup-dropbox",
        }
    }

    fn all() -> [Self; 2] {
        [Self::GoogleDrive, Self::Dropbox]
    }
}

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudConnectionState {
    Disconnected,
    Connected,
    Reauthorize,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct CloudSourceDto {
    pub id: String,
    pub provider: CloudProvider,
    pub label: String,
    pub connection: CloudConnectionState,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct CloudSourcesSnapshot {
    pub version: u32,
    pub sources: Vec<CloudSourceDto>,
}

/// Explicit readiness proof for the Library-owned cloud CatDB catalog.
#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct CloudCatalogReady {
    pub workspace_id: String,
    pub catalog_revision_id: String,
    pub schema_version_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudFilterOperator {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    Glob,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudFilterInput {
    pub field: String,
    pub operator: CloudFilterOperator,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
pub(crate) struct CloudDocumentRow {
    pub source_id: String,
    pub record_key: String,
    #[specta(type = specta_typescript::Unknown)]
    pub fields: serde_json::Value,
    pub provenance: Vec<CloudFieldProvenance>,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloudObservation {
    ObservedValue {
        #[specta(type = specta_typescript::Unknown)]
        value: serde_json::Value,
    },
    ObservedNull,
    ObservedNothing {
        reason: String,
    },
    Unobservable {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
pub(crate) struct CloudConsideredProvider {
    pub provider: CloudProvider,
    pub source_id: String,
    pub record_key: String,
    pub mapping_version: String,
    /// Exact physical provider attribute selected by the validated mapping.
    pub physical_attribute: Option<String>,
    pub observation: CloudObservation,
    #[specta(type = Option<f64>)]
    pub observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
pub(crate) struct CloudFieldProvenance {
    pub field: String,
    #[specta(type = Option<f64>)]
    pub selected: Option<u32>,
    pub providers: Vec<CloudConsideredProvider>,
}

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudCompleteness {
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudExecutionMode {
    Virtual,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloudSourceOutcome {
    Ran {
        mode: CloudExecutionMode,
        scanned: u32,
        returned: u32,
        pushed: Vec<String>,
        residual: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct CloudSourceReport {
    pub source_id: String,
    #[specta(type = f64)]
    pub freshness_ms: u64,
    pub outcome: CloudSourceOutcome,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct CloudVersionPins {
    pub catalog_version: String,
    pub schema_version: String,
    pub schema_checksum: String,
    pub source_schema_version: String,
    pub source_schema_checksum: String,
    pub mapping_version: String,
    pub mapping_checksum: String,
    pub connector_version: String,
    pub source_version: String,
    pub source_checksum: String,
    pub policy_version: String,
    pub policy_checksum: String,
    pub provenance_rule_version: String,
    pub provenance_rule_checksum: String,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
pub(crate) struct CloudDocumentQueryResponse {
    pub query_id: String,
    pub provider: CloudProvider,
    pub workspace_id: String,
    pub schema_version: String,
    pub version_pins: CloudVersionPins,
    pub completeness: CloudCompleteness,
    pub sources: Vec<CloudSourceReport>,
    pub scanned: u32,
    pub rows: Vec<CloudDocumentRow>,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloudSourceError {
    InvalidConfiguration,
    SourceNotFound,
    CredentialUnavailable,
    CredentialMissing,
    CorruptRegistry,
    UnsupportedVersion {
        found: u32,
        expected: u32,
    },
    PersistFailed,
    RollbackFailed,
    ClientConfigurationMissing,
    OauthCallbackUnavailable,
    OauthInvalidCallback,
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    OauthStateMismatch,
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    OauthDenied,
    OauthTimeout,
    OauthCancelled,
    OauthTransport,
    OauthTokenRejected,
    OauthOfflineGrantMissing,
    OauthRateLimited {
        retry_after_ms: Option<u32>,
    },
    ProviderRejected,
    #[allow(dead_code)]
    SourcePaused,
    #[allow(dead_code)]
    QueryInvalid {
        detail: String,
    },
    QueryCancelled,
    QueryRetired,
    #[allow(dead_code)]
    QueryCatalogUnavailable,
    QueryCatalogInvalid,
    QueryAuthorizationDenied,
    QueryAllSourcesRefused,
    QueryDeadlineExceeded,
    QueryTimeout {
        source_id: String,
    },
    QueryAuthentication {
        source_id: String,
    },
    QueryRateLimited {
        source_id: String,
        retry_after_ms: Option<u32>,
    },
    QueryIncomplete {
        source_id: String,
    },
    QueryIncompleteSources {
        source_ids: Vec<String>,
    },
    QueryStaleSources {
        source_ids: Vec<String>,
    },
    QueryTransport {
        source_id: String,
    },
    QuerySourceCancelled {
        source_id: String,
    },
    QueryLimitExceeded,
    QueryNotFound {
        source_id: String,
        record_key: String,
    },
    QueryRefused {
        source_id: String,
        reason: CloudQueryRefusal,
    },
    QueryReadFailed {
        source_id: String,
    },
    QuerySourceProtocol {
        source_id: String,
    },
    QuerySchemaDrift {
        source_id: String,
        expected: String,
        observed: String,
    },
    QueryExecutionFailed {
        sources: Vec<SourceExecution>,
    },
    QueryInvalidResponse,
    #[allow(dead_code)]
    QueryFailed {
        detail: String,
    },
    #[cfg(any(
        not(feature = "google-drive-connector"),
        not(feature = "dropbox-connector")
    ))]
    ConnectorUnavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudQueryRefusal {
    Unavailable,
    UnknownEntity,
    UnobservableAttribute,
    UnsupportedFilter,
    UnsupportedOrdering,
    UnsupportedLimit,
    UnsupportedPolicy,
}

impl fmt::Display for CloudSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "cloud-source-invalid-configuration",
            Self::SourceNotFound => "cloud-source-not-found",
            Self::CredentialUnavailable => "cloud-credential-unavailable",
            Self::CredentialMissing => "cloud-credential-missing",
            Self::CorruptRegistry => "cloud-source-registry-corrupt",
            Self::UnsupportedVersion { .. } => "cloud-source-registry-version-unsupported",
            Self::PersistFailed => "cloud-source-persist-failed",
            Self::RollbackFailed => "cloud-source-rollback-failed",
            Self::ClientConfigurationMissing => "cloud-oauth-client-configuration-missing",
            Self::OauthCallbackUnavailable => "cloud-oauth-callback-unavailable",
            Self::OauthInvalidCallback => "cloud-oauth-invalid-callback",
            #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
            Self::OauthStateMismatch => "cloud-oauth-state-mismatch",
            #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
            Self::OauthDenied => "cloud-oauth-denied",
            Self::OauthTimeout => "cloud-oauth-timeout",
            Self::OauthCancelled => "cloud-oauth-cancelled",
            Self::OauthTransport => "cloud-oauth-transport-failed",
            Self::OauthTokenRejected => "cloud-oauth-token-rejected",
            Self::OauthOfflineGrantMissing => "cloud-oauth-offline-grant-missing",
            Self::OauthRateLimited { .. } => "cloud-oauth-rate-limited",
            Self::ProviderRejected => "cloud-oauth-provider-rejected",
            Self::SourcePaused => "cloud-source-paused",
            Self::QueryInvalid { .. } => "cloud-query-invalid",
            Self::QueryCancelled => "cloud-query-cancelled",
            Self::QueryRetired => "cloud-query-retired",
            Self::QueryCatalogUnavailable => "cloud-query-catalog-unavailable",
            Self::QueryCatalogInvalid => "cloud-query-catalog-invalid",
            Self::QueryAuthorizationDenied => "cloud-query-authorization-denied",
            Self::QueryAllSourcesRefused => "cloud-query-all-sources-refused",
            Self::QueryDeadlineExceeded => "cloud-query-deadline-exceeded",
            Self::QueryTimeout { .. } => "cloud-query-source-timeout",
            Self::QueryAuthentication { .. } => "cloud-query-authentication-failed",
            Self::QueryRateLimited { .. } => "cloud-query-rate-limited",
            Self::QueryIncomplete { .. } => "cloud-query-incomplete",
            Self::QueryIncompleteSources { .. } => "cloud-query-incomplete",
            Self::QueryStaleSources { .. } => "cloud-query-stale",
            Self::QueryTransport { .. } => "cloud-query-transport-failed",
            Self::QuerySourceCancelled { .. } => "cloud-query-source-cancelled",
            Self::QueryLimitExceeded => "cloud-query-limit-exceeded",
            Self::QueryNotFound { .. } => "cloud-query-not-found",
            Self::QueryRefused { .. } => "cloud-query-refused",
            Self::QueryReadFailed { .. } => "cloud-query-read-failed",
            Self::QuerySourceProtocol { .. } => "cloud-query-source-protocol-failed",
            Self::QuerySchemaDrift { .. } => "cloud-query-schema-drift",
            Self::QueryExecutionFailed { .. } => "cloud-query-execution-failed",
            Self::QueryInvalidResponse => "cloud-query-invalid-response",
            Self::QueryFailed { .. } => "cloud-query-failed",
            #[cfg(any(
                not(feature = "google-drive-connector"),
                not(feature = "dropbox-connector")
            ))]
            Self::ConnectorUnavailable => "cloud-connector-unavailable",
        })
    }
}

impl std::error::Error for CloudSourceError {}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DriveItemSelection {
    id: String,
    resource_key: Option<String>,
    folder: bool,
    #[serde(default)]
    root: bool,
}

impl fmt::Debug for DriveItemSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveItemSelection")
            .field("id", &"[redacted-provider-id]")
            .field(
                "resource_key",
                &self
                    .resource_key
                    .as_ref()
                    .map(|_| "[redacted-resource-key]"),
            )
            .field("folder", &self.folder)
            .field("root", &self.root)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloudSourceConfig {
    GoogleDrive { selections: Vec<DriveItemSelection> },
    Dropbox,
}

impl fmt::Debug for CloudSourceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GoogleDrive { selections } => formatter
                .debug_struct("GoogleDrive")
                .field("selection_count", &selections.len())
                .finish(),
            Self::Dropbox => formatter.write_str("Dropbox"),
        }
    }
}

impl CloudSourceConfig {
    fn provider(&self) -> CloudProvider {
        match self {
            Self::GoogleDrive { .. } => CloudProvider::GoogleDrive,
            Self::Dropbox => CloudProvider::Dropbox,
        }
    }

    fn validate(&self) -> Result<(), CloudSourceError> {
        match self {
            Self::GoogleDrive { selections }
                if selections.is_empty() || selections.len() > MAX_DRIVE_SELECTIONS =>
            {
                Err(CloudSourceError::InvalidConfiguration)
            }
            Self::GoogleDrive { selections } => {
                let mut ids = HashSet::new();
                for selection in selections {
                    let valid_id = !selection.id.is_empty()
                        && selection.id.len() <= 256
                        && selection.id.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
                        });
                    let valid_resource_key = selection.resource_key.as_ref().is_none_or(|key| {
                        !key.is_empty()
                            && key.len() <= 512
                            && key
                                .bytes()
                                .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b',' | b'/'))
                    });
                    if !valid_id || !valid_resource_key || !ids.insert(selection.id.as_str()) {
                        return Err(CloudSourceError::InvalidConfiguration);
                    }
                    if selection.root && !selection.folder {
                        return Err(CloudSourceError::InvalidConfiguration);
                    }
                }
                Ok(())
            }
            Self::Dropbox => Ok(()),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub(crate) struct SecretBundle {
    access_token: String,
    refresh_token: String,
    #[zeroize(skip)]
    expires_at_unix_seconds: u64,
    cursor_key: [u8; 32],
}

impl SecretBundle {
    pub(crate) fn new(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        expires_at_unix_seconds: u64,
        cursor_key: [u8; 32],
    ) -> Result<Self, CloudSourceError> {
        let bundle = Self {
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            expires_at_unix_seconds,
            cursor_key,
        };
        bundle.validate()?;
        Ok(bundle)
    }

    fn validate(&self) -> Result<(), CloudSourceError> {
        let valid_secret = |secret: &str| {
            !secret.is_empty() && secret.len() <= 16 * 1024 && !secret.chars().any(char::is_control)
        };
        if !valid_secret(&self.access_token)
            || !valid_secret(&self.refresh_token)
            || self.expires_at_unix_seconds == 0
            || self.cursor_key.iter().all(|byte| *byte == 0)
        {
            return Err(CloudSourceError::InvalidConfiguration);
        }
        Ok(())
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    fn expires_at(&self) -> Result<SystemTime, CloudSourceError> {
        UNIX_EPOCH
            .checked_add(Duration::from_secs(self.expires_at_unix_seconds))
            .ok_or(CloudSourceError::InvalidConfiguration)
    }
}

impl fmt::Debug for SecretBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBundle([redacted])")
    }
}

pub(crate) trait CredentialVault: Send + Sync {
    fn get(&self, reference: &str) -> Result<Option<SecretBundle>, CloudSourceError>;
    fn set(&self, reference: &str, secret: &SecretBundle) -> Result<(), CloudSourceError>;
    fn delete(&self, reference: &str) -> Result<(), CloudSourceError>;
    fn cleanup_references(&self, provider: CloudProvider) -> Result<Vec<String>, CloudSourceError>;
    fn set_cleanup_references(
        &self,
        provider: CloudProvider,
        references: &[String],
    ) -> Result<(), CloudSourceError>;
}

fn valid_credential_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 256
        && !reference.chars().any(char::is_control)
        && !CloudProvider::all()
            .iter()
            .any(|provider| reference == provider.credential_cleanup_reference())
}

fn validate_cleanup_references(references: &[String]) -> Result<(), CloudSourceError> {
    if references.len() > MAX_PENDING_CREDENTIAL_CLEANUPS {
        return Err(CloudSourceError::CredentialUnavailable);
    }
    let mut unique = HashSet::new();
    if references
        .iter()
        .any(|reference| !valid_credential_reference(reference) || !unique.insert(reference))
    {
        return Err(CloudSourceError::CredentialUnavailable);
    }
    Ok(())
}

fn validate_current_cloud_record(
    sources: &[CloudSourceRecord],
    expected: &CloudSourceRecord,
) -> Result<(), CloudSourceError> {
    let current = sources
        .iter()
        .find(|source| source.provider == expected.provider)
        .ok_or(CloudSourceError::QueryCatalogInvalid)?;
    if current != expected || !current.enabled {
        return Err(CloudSourceError::QueryCatalogInvalid);
    }
    Ok(())
}

fn map_cloud_point_read_error(
    error: CloudSourceError,
) -> crate::cloud_documents::CloudDocumentError {
    match error {
        CloudSourceError::SourcePaused => crate::cloud_documents::CloudDocumentError::SourcePaused,
        CloudSourceError::CredentialUnavailable | CloudSourceError::CredentialMissing => {
            crate::cloud_documents::CloudDocumentError::CredentialUnavailable
        }
        CloudSourceError::QueryDeadlineExceeded | CloudSourceError::QueryTimeout { .. } => {
            crate::cloud_documents::CloudDocumentError::Timeout
        }
        CloudSourceError::QueryCancelled | CloudSourceError::QuerySourceCancelled { .. } => {
            crate::cloud_documents::CloudDocumentError::Cancelled
        }
        CloudSourceError::QueryRateLimited { retry_after_ms, .. } => {
            crate::cloud_documents::CloudDocumentError::RateLimited {
                retry_after_ms: retry_after_ms.map(u64::from),
            }
        }
        CloudSourceError::SourceNotFound
        | CloudSourceError::QueryRetired
        | CloudSourceError::QueryAuthorizationDenied
        | CloudSourceError::QueryCatalogInvalid => {
            crate::cloud_documents::CloudDocumentError::Unauthorized
        }
        #[cfg(any(
            not(feature = "google-drive-connector"),
            not(feature = "dropbox-connector")
        ))]
        CloudSourceError::ConnectorUnavailable => {
            crate::cloud_documents::CloudDocumentError::ConnectorUnavailable
        }
        _ => crate::cloud_documents::CloudDocumentError::ProtocolFailed,
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn validate_current_library_memberships(
    registry: &LibraryStore,
    expected_locals: &[crate::library::LocalFolderSource],
    expected_clouds: &[CloudSourceRecord],
) -> Result<(), CloudSourceError> {
    let current = registry
        .read_query_snapshot()
        .map_err(crate::library::map_library_cloud_error)?;
    for expected in expected_locals {
        if !current
            .local_sources
            .iter()
            .any(|source| source == expected)
        {
            return Err(CloudSourceError::QueryRetired);
        }
    }
    for expected in expected_clouds {
        validate_current_cloud_record(&current.cloud_sources, expected)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudSourceRecord {
    id: String,
    provider: CloudProvider,
    label: String,
    enabled: bool,
    credential_reference: String,
    config: CloudSourceConfig,
    #[serde(default)]
    authorization_profile: CloudAuthorizationProfile,
    #[serde(default)]
    contract: Option<crate::library::LibrarySourceContract>,
}

impl CloudSourceRecord {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn provider(&self) -> CloudProvider {
        self.provider
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn authorization_profile(&self) -> CloudAuthorizationProfile {
        self.authorization_profile
    }

    pub(crate) fn editing_capability(&self) -> CloudEditingCapability {
        match self.authorization_profile {
            CloudAuthorizationProfile::ReadOnlyV1 => CloudEditingCapability::ReconnectRequired,
            CloudAuthorizationProfile::MarkdownReadWriteV2 => {
                CloudEditingCapability::WriteCapabilityPendingAcceptance
            }
        }
    }

    pub(crate) fn contract(&self) -> Option<&crate::library::LibrarySourceContract> {
        self.contract.as_ref()
    }

    pub(crate) fn refresh_contract(&mut self) {
        self.contract = Some(crate::library::LibrarySourceContract::cloud(self.provider));
    }
}

pub(crate) fn validate_cloud_source_records(
    sources: &[CloudSourceRecord],
) -> Result<(), CloudSourceError> {
    if sources.len() > MAX_CLOUD_SOURCES {
        return Err(CloudSourceError::CorruptRegistry);
    }
    let mut ids = HashSet::new();
    let mut providers = HashSet::new();
    let mut references = HashSet::new();
    for source in sources {
        let valid_id = source.id.len() == 64
            && source
                .id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        let valid_reference = valid_credential_reference(&source.credential_reference);
        if !valid_id
            || !valid_reference
            || source.label != source.provider.label()
            || source.config.provider() != source.provider
            || source.config.validate().is_err()
            || source.contract.as_ref()
                != Some(&crate::library::LibrarySourceContract::cloud(
                    source.provider,
                ))
            || !ids.insert(source.id.as_str())
            || !providers.insert(source.provider)
            || !references.insert(source.credential_reference.as_str())
        {
            return Err(CloudSourceError::CorruptRegistry);
        }
    }
    Ok(())
}

pub(crate) struct CloudAccountService<'a, V> {
    registry: LibraryStore,
    vault: &'a V,
}

struct CloudDisconnectPublication {
    snapshot: CloudSourcesSnapshot,
    retired_source_id: Option<catdb_connectors::SourceId>,
}

/// One committed credential replacement and the exact source identities on
/// either side of it. The post-commit sync fence consumes these identities
/// before it opens a provider source, so an old scheduler cannot outlive a
/// newly published credential/source version.
struct CloudInstallPublication {
    snapshot: CloudSourcesSnapshot,
    retired_source_id: Option<catdb_connectors::SourceId>,
    activated_source_id: catdb_connectors::SourceId,
}

impl<'a, V: CredentialVault> CloudAccountService<'a, V> {
    pub(crate) fn new(registry: LibraryStore, vault: &'a V) -> Self {
        Self { registry, vault }
    }

    pub(crate) fn snapshot(&self) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let _lease = self.acquire_lifecycle_lease()?;
        self.snapshot_under_lease()
    }

    fn snapshot_under_lease(&self) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        for provider in CloudProvider::all() {
            self.recover_credential_cleanup(provider)?;
        }
        let records = self.registry.read_cloud(|sources| Ok(sources.to_vec()))?;
        self.snapshot_records(&records)
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    fn admitted_record(
        &self,
        provider: CloudProvider,
    ) -> Result<(CloudSourceRecord, SecretBundle), CloudSourceError> {
        self.registry.read_cloud(|sources| {
            let record = sources
                .iter()
                .find(|source| source.provider == provider)
                .cloned()
                .ok_or(CloudSourceError::SourceNotFound)?;
            if !record.enabled {
                return Err(CloudSourceError::SourcePaused);
            }
            let credential = self
                .vault
                .get(&record.credential_reference)?
                .ok_or(CloudSourceError::CredentialMissing)?;
            Ok((record, credential))
        })
    }

    fn snapshot_records(
        &self,
        records: &[CloudSourceRecord],
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let mut sources = Vec::with_capacity(MAX_CLOUD_SOURCES);
        for provider in CloudProvider::all() {
            let record = records.iter().find(|source| source.provider == provider);
            let (id, connection, enabled) = match record {
                None => (String::new(), CloudConnectionState::Disconnected, false),
                Some(record) => match self.vault.get(&record.credential_reference)? {
                    Some(_) => (
                        record.id.clone(),
                        CloudConnectionState::Connected,
                        record.enabled,
                    ),
                    None => (record.id.clone(), CloudConnectionState::Reauthorize, false),
                },
            };
            sources.push(CloudSourceDto {
                id,
                provider,
                label: provider.label().to_string(),
                connection,
                enabled,
            });
        }
        Ok(CloudSourcesSnapshot {
            version: CLOUD_REGISTRY_VERSION,
            sources,
        })
    }

    pub(crate) fn install(
        &self,
        provider: CloudProvider,
        config: CloudSourceConfig,
        secret: SecretBundle,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        self.install_with_profile(
            provider,
            config,
            secret,
            CloudAuthorizationProfile::ReadOnlyV1,
        )
    }

    pub(crate) fn install_with_profile(
        &self,
        provider: CloudProvider,
        config: CloudSourceConfig,
        secret: SecretBundle,
        authorization_profile: CloudAuthorizationProfile,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        Ok(self
            .install_with_profile_publication(provider, config, secret, authorization_profile)?
            .snapshot)
    }

    fn install_with_profile_publication(
        &self,
        provider: CloudProvider,
        config: CloudSourceConfig,
        secret: SecretBundle,
        authorization_profile: CloudAuthorizationProfile,
    ) -> Result<CloudInstallPublication, CloudSourceError> {
        config.validate()?;
        if config.provider() != provider {
            return Err(CloudSourceError::InvalidConfiguration);
        }
        let _lease = self.acquire_lifecycle_lease()?;
        self.recover_credential_cleanup(provider)?;
        let expected = self.registry.read_cloud(|sources| {
            Ok(sources
                .iter()
                .find(|source| source.provider == provider)
                .cloned())
        })?;
        let id = new_source_id();
        let credential_reference = format!("cloud-credential-{id}");
        if self.vault.get(&credential_reference)?.is_some() {
            return Err(CloudSourceError::CredentialUnavailable);
        }
        let record = CloudSourceRecord {
            id,
            provider,
            label: provider.label().to_string(),
            enabled: true,
            credential_reference: credential_reference.clone(),
            config,
            authorization_profile,
            contract: Some(crate::library::LibrarySourceContract::cloud(provider)),
        };
        let mut cleanup_references = vec![credential_reference.clone()];
        if let Some(reference) = expected
            .as_ref()
            .map(|source| source.credential_reference.clone())
        {
            cleanup_references.push(reference);
        }
        self.journal_credential_cleanup(provider, cleanup_references)?;
        if let Err(error) = self.vault.set(&credential_reference, &secret) {
            return match self.recover_credential_cleanup(provider) {
                Ok(()) => Err(error),
                Err(_) => Err(CloudSourceError::RollbackFailed),
            };
        }
        let expected_for_commit = expected.clone();
        let record_for_commit = record.clone();
        let update = self.registry.update_cloud(|sources| {
            let current = sources
                .iter()
                .find(|source| source.provider == provider)
                .cloned();
            if current != expected_for_commit {
                return Err(CloudSourceError::PersistFailed);
            }
            if let Some(index) = sources
                .iter()
                .position(|source| source.provider == provider)
            {
                sources[index] = record_for_commit;
            } else {
                sources.push(record_for_commit);
            }
            Ok(())
        });
        if let Err(error) = update {
            return match self.recover_credential_cleanup(provider) {
                Ok(()) => Err(error),
                Err(_) => Err(CloudSourceError::RollbackFailed),
            };
        }
        self.recover_credential_cleanup(provider)?;
        Ok(CloudInstallPublication {
            snapshot: self.snapshot_under_lease()?,
            retired_source_id: expected
                .as_ref()
                .map(|source| catdb_connectors::SourceId::new(source.id.clone())),
            activated_source_id: catdb_connectors::SourceId::new(record.id.clone()),
        })
    }

    pub(crate) fn set_enabled(
        &self,
        provider: CloudProvider,
        enabled: bool,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let _lease = self.acquire_lifecycle_lease()?;
        self.recover_credential_cleanup(provider)?;
        self.registry.update_cloud(|sources| {
            let record = sources
                .iter_mut()
                .find(|source| source.provider == provider)
                .ok_or(CloudSourceError::SourceNotFound)?;
            if enabled && self.vault.get(&record.credential_reference)?.is_none() {
                return Err(CloudSourceError::CredentialMissing);
            }
            record.enabled = enabled;
            Ok(())
        })?;
        self.snapshot_under_lease()
    }

    /// Removes one provider record and returns the exact source identity that
    /// was committed as disconnected under the same lifecycle lease.
    fn disconnect_with_id(
        &self,
        provider: CloudProvider,
    ) -> Result<CloudDisconnectPublication, CloudSourceError> {
        let _lease = self.acquire_lifecycle_lease()?;
        self.recover_credential_cleanup(provider)?;
        let expected = self.registry.read_cloud(|sources| {
            Ok(sources
                .iter()
                .find(|source| source.provider == provider)
                .cloned())
        })?;
        let Some(expected) = expected else {
            return Ok(CloudDisconnectPublication {
                snapshot: self.snapshot_under_lease()?,
                retired_source_id: None,
            });
        };
        let retired_source_id = catdb_connectors::SourceId::new(expected.id.clone());
        self.journal_credential_cleanup(provider, vec![expected.credential_reference.clone()])?;
        let expected_for_commit = expected.clone();
        let update = self.registry.update_cloud(|sources| {
            let Some(index) = sources
                .iter()
                .position(|source| source.provider == provider)
            else {
                return Err(CloudSourceError::PersistFailed);
            };
            if sources[index] != expected_for_commit {
                return Err(CloudSourceError::PersistFailed);
            }
            sources.remove(index);
            Ok(())
        });
        if let Err(error) = update {
            return match self.recover_credential_cleanup(provider) {
                Ok(()) => Err(error),
                Err(_) => Err(CloudSourceError::RollbackFailed),
            };
        }
        self.recover_credential_cleanup(provider)?;
        Ok(CloudDisconnectPublication {
            snapshot: self.snapshot_under_lease()?,
            retired_source_id: Some(retired_source_id),
        })
    }

    fn acquire_lifecycle_lease(&self) -> Result<AdvisoryFileLock, CloudSourceError> {
        let lock_path = self
            .registry
            .registry_path()
            .parent()
            .map(|parent| parent.join(CLOUD_LIFECYCLE_LOCK_FILE))
            .ok_or(CloudSourceError::PersistFailed)?;
        AdvisoryFileLock::acquire(&lock_path).map_err(|_| CloudSourceError::PersistFailed)
    }

    fn journal_credential_cleanup(
        &self,
        provider: CloudProvider,
        references: Vec<String>,
    ) -> Result<(), CloudSourceError> {
        let mut pending = self.vault.cleanup_references(provider)?;
        for reference in references {
            if !pending.contains(&reference) {
                pending.push(reference);
            }
        }
        validate_cleanup_references(&pending)?;
        self.vault.set_cleanup_references(provider, &pending)
    }

    fn recover_credential_cleanup(&self, provider: CloudProvider) -> Result<(), CloudSourceError> {
        let mut pending = self.vault.cleanup_references(provider)?;
        validate_cleanup_references(&pending)?;
        let active_by_provider = self.registry.read_cloud(|sources| {
            Ok(sources
                .iter()
                .map(|source| (source.provider, source.credential_reference.clone()))
                .collect::<HashMap<_, _>>())
        })?;
        let active_references = active_by_provider.values().collect::<HashSet<_>>();
        for reference in pending.clone() {
            if active_by_provider.get(&provider) == Some(&reference) {
                continue;
            }
            if !active_references.contains(&reference) {
                self.vault.delete(&reference)?;
            }
            pending.retain(|candidate| candidate != &reference);
            self.vault.set_cleanup_references(provider, &pending)?;
        }
        Ok(())
    }
}

fn new_source_id() -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-cloud-source-id-v1\0");
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(crate) struct OAuthAttempt {
    provider: CloudProvider,
    authorization_profile: CloudAuthorizationProfile,
    redirect_uri: String,
    pkce: OAuthPkce,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
enum OAuthPkce {
    #[cfg(feature = "google-drive-connector")]
    GoogleDrive(catdb_connector_google_drive::DriveOAuthPkce),
    #[cfg(feature = "dropbox-connector")]
    Dropbox(catdb_connector_dropbox::DropboxOAuthPkce),
}

#[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
struct OAuthPkce;

impl OAuthAttempt {
    pub(crate) fn new(
        provider: CloudProvider,
        verifier: impl Into<String>,
        state: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Result<Self, CloudSourceError> {
        Self::with_profile(
            provider,
            CloudAuthorizationProfile::ReadOnlyV1,
            verifier,
            state,
            redirect_uri,
        )
    }

    pub(crate) fn with_profile(
        provider: CloudProvider,
        authorization_profile: CloudAuthorizationProfile,
        verifier: impl Into<String>,
        state: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Result<Self, CloudSourceError> {
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = (
                provider,
                authorization_profile,
                verifier.into(),
                state.into(),
                redirect_uri.into(),
            );
            Err(CloudSourceError::ConnectorUnavailable)
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        {
            let verifier = verifier.into();
            let state = state.into();
            let redirect_uri = redirect_uri.into();
            let pkce = match provider {
                #[cfg(feature = "google-drive-connector")]
                CloudProvider::GoogleDrive => OAuthPkce::GoogleDrive(
                    catdb_connector_google_drive::DriveOAuthPkce::new(
                        verifier,
                        state,
                        redirect_uri.clone(),
                    )
                    .map_err(|_| CloudSourceError::InvalidConfiguration)?,
                ),
                #[cfg(not(feature = "google-drive-connector"))]
                CloudProvider::GoogleDrive => return Err(CloudSourceError::ConnectorUnavailable),
                #[cfg(feature = "dropbox-connector")]
                CloudProvider::Dropbox => OAuthPkce::Dropbox(
                    catdb_connector_dropbox::DropboxOAuthPkce::new(
                        verifier,
                        state,
                        redirect_uri.clone(),
                    )
                    .map_err(|_| CloudSourceError::InvalidConfiguration)?,
                ),
                #[cfg(not(feature = "dropbox-connector"))]
                CloudProvider::Dropbox => return Err(CloudSourceError::ConnectorUnavailable),
            };
            Ok(Self {
                provider,
                authorization_profile,
                redirect_uri,
                pkce,
            })
        }
    }

    pub(crate) fn authorization_url(&self, client_id: &str) -> Result<String, CloudSourceError> {
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = (&self.pkce, client_id);
            Err(CloudSourceError::ConnectorUnavailable)
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        match &self.pkce {
            #[cfg(feature = "google-drive-connector")]
            OAuthPkce::GoogleDrive(pkce) => pkce
                .authorization_url_for_profile(
                    client_id,
                    match self.authorization_profile {
                        CloudAuthorizationProfile::ReadOnlyV1 => {
                            catdb_connector_google_drive::DriveAuthorizationProfile::ReadOnly
                        }
                        CloudAuthorizationProfile::MarkdownReadWriteV2 => {
                            catdb_connector_google_drive::DriveAuthorizationProfile::MarkdownReadWrite
                        }
                    },
                )
                .map_err(|_| CloudSourceError::InvalidConfiguration),
            #[cfg(feature = "dropbox-connector")]
            OAuthPkce::Dropbox(pkce) => pkce
                .authorization_url_for_profile(
                    client_id,
                    match self.authorization_profile {
                        CloudAuthorizationProfile::ReadOnlyV1 => {
                            catdb_connector_dropbox::DropboxAuthorizationProfile::ReadOnly
                        }
                        CloudAuthorizationProfile::MarkdownReadWriteV2 => {
                            catdb_connector_dropbox::DropboxAuthorizationProfile::MarkdownReadWrite
                        }
                    },
                )
                .map_err(|_| CloudSourceError::InvalidConfiguration),
        }
    }

    pub(crate) fn verifier(&self) -> Result<&str, CloudSourceError> {
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = &self.pkce;
            Err(CloudSourceError::ConnectorUnavailable)
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        match &self.pkce {
            #[cfg(feature = "google-drive-connector")]
            OAuthPkce::GoogleDrive(pkce) => Ok(pkce.verifier()),
            #[cfg(feature = "dropbox-connector")]
            OAuthPkce::Dropbox(pkce) => Ok(pkce.verifier()),
        }
    }

    fn authorization_profile(&self) -> CloudAuthorizationProfile {
        self.authorization_profile
    }

    fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub(crate) fn verify_callback(
        &self,
        callback: &str,
    ) -> Result<VerifiedAuthorization, CloudSourceError> {
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = (&self.pkce, callback);
            Err(CloudSourceError::ConnectorUnavailable)
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        {
            if callback.len() > MAX_CALLBACK_BYTES {
                return Err(CloudSourceError::OauthInvalidCallback);
            }
            let expected = Url::parse(&self.redirect_uri)
                .map_err(|_| CloudSourceError::OauthInvalidCallback)?;
            let callback =
                Url::parse(callback).map_err(|_| CloudSourceError::OauthInvalidCallback)?;
            if callback.scheme() != expected.scheme()
                || callback.host_str() != expected.host_str()
                || callback.port_or_known_default() != expected.port_or_known_default()
                || callback.path() != expected.path()
                || callback.fragment().is_some()
                || !callback.username().is_empty()
                || callback.password().is_some()
            {
                return Err(CloudSourceError::OauthInvalidCallback);
            }
            let mut parameters: HashMap<String, String> = HashMap::new();
            for (key, value) in callback.query_pairs() {
                if parameters
                    .insert(key.into_owned(), value.into_owned())
                    .is_some()
                {
                    return Err(CloudSourceError::OauthInvalidCallback);
                }
            }
            let state = parameters
                .get("state")
                .ok_or(CloudSourceError::OauthInvalidCallback)?;
            let valid_state = match &self.pkce {
                #[cfg(feature = "google-drive-connector")]
                OAuthPkce::GoogleDrive(pkce) => pkce.verify_state(state),
                #[cfg(feature = "dropbox-connector")]
                OAuthPkce::Dropbox(pkce) => pkce.verify_state(state),
            };
            if !valid_state {
                return Err(CloudSourceError::OauthStateMismatch);
            }
            if parameters.contains_key("error") {
                return Err(CloudSourceError::OauthDenied);
            }
            let code = parameters
                .get("code")
                .filter(|code| {
                    !code.is_empty()
                        && code.len() <= MAX_AUTHORIZATION_CODE_BYTES
                        && !code.chars().any(char::is_control)
                })
                .ok_or(CloudSourceError::OauthInvalidCallback)?;
            let picked_file_ids = if self.provider == CloudProvider::GoogleDrive {
                let picked_file_ids =
                    parameters
                        .get("picked_file_ids")
                        .map_or_else(Vec::new, |value| {
                            value
                                .split(',')
                                .map(str::to_string)
                                .collect::<Vec<String>>()
                        });
                if picked_file_ids.is_empty() {
                    vec!["root".to_string()]
                } else {
                    picked_file_ids
                }
            } else {
                Vec::new()
            };
            if picked_file_ids.len() > MAX_DRIVE_SELECTIONS {
                return Err(CloudSourceError::OauthInvalidCallback);
            }
            Ok(VerifiedAuthorization {
                code: Zeroizing::new(code.clone()),
                picked_file_ids,
            })
        }
    }
}

impl fmt::Debug for OAuthAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthAttempt")
            .field("provider", &self.provider)
            .field("authorization_profile", &self.authorization_profile)
            .field("redirect_uri", &self.redirect_uri)
            .field("pkce", &"[redacted]")
            .finish()
    }
}

pub(crate) struct VerifiedAuthorization {
    code: Zeroizing<String>,
    picked_file_ids: Vec<String>,
}

impl fmt::Debug for VerifiedAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedAuthorization")
            .field("code", &"[redacted]")
            .field("picked_file_count", &self.picked_file_ids.len())
            .finish()
    }
}

impl VerifiedAuthorization {
    fn code(&self) -> &str {
        self.code.as_str()
    }

    fn picked_file_ids(&self) -> &[String] {
        &self.picked_file_ids
    }
}

#[derive(Debug)]
struct ActiveCloudQuery {
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct StoredLibraryContinuation {
    continuation: crate::library_query::LibraryQueryContinuation,
    local_records: Vec<crate::library::LocalFolderSource>,
    cloud_records: Vec<CloudSourceRecord>,
    registry: LibraryStore,
}

#[derive(Debug)]
struct ActiveCloudConnect {
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
    phase: Arc<AtomicU8>,
}

#[derive(Debug)]
struct CloudConnectHandle {
    provider: CloudProvider,
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
    phase: Arc<AtomicU8>,
}

const CLOUD_CONNECT_PENDING: u8 = 0;
const CLOUD_CONNECT_COMMITTING: u8 = 1;
const CLOUD_CONNECT_COMPLETE: u8 = 2;

#[derive(Clone)]
struct CloudConnectCommitAuthority {
    active_connects: Arc<Mutex<HashMap<CloudProvider, ActiveCloudConnect>>>,
    provider: CloudProvider,
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
    phase: Arc<AtomicU8>,
}

impl CloudConnectCommitAuthority {
    fn begin_commit(&self) -> Result<(), CloudSourceError> {
        let active = self
            .active_connects
            .lock()
            .map_err(|_| CloudSourceError::OauthTransport)?;
        match active.get(&self.provider) {
            Some(entry)
                if entry.serial == self.serial
                    && !self.cancellation.is_cancelled()
                    && entry
                        .phase
                        .compare_exchange(
                            CLOUD_CONNECT_PENDING,
                            CLOUD_CONNECT_COMMITTING,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .is_ok() =>
            {
                Ok(())
            }
            _ => Err(CloudSourceError::OauthCancelled),
        }
    }

    fn finish_commit(&self) {
        self.phase.store(CLOUD_CONNECT_COMPLETE, Ordering::SeqCst);
        if let Ok(mut active) = self.active_connects.lock() {
            if active
                .get(&self.provider)
                .is_some_and(|entry| entry.serial == self.serial)
            {
                active.remove(&self.provider);
            }
        }
    }
}

struct CloudConnectOwner<'a> {
    state: &'a CloudAccountsState,
    handle: CloudConnectHandle,
    settled: bool,
}

impl<'a> CloudConnectOwner<'a> {
    fn begin(
        state: &'a CloudAccountsState,
        provider: CloudProvider,
    ) -> Result<Self, CloudSourceError> {
        Ok(Self {
            state,
            handle: state.begin_connect(provider)?,
            settled: false,
        })
    }

    fn cancellation(&self) -> catdb_connectors::CancellationToken {
        self.handle.cancellation.clone()
    }

    fn commit_authority(&self) -> CloudConnectCommitAuthority {
        CloudConnectCommitAuthority {
            active_connects: Arc::clone(&self.state.active_connects),
            provider: self.handle.provider,
            serial: self.handle.serial,
            cancellation: self.handle.cancellation.clone(),
            phase: Arc::clone(&self.handle.phase),
        }
    }

    fn finish_committed(mut self) {
        self.settled = true;
    }

    #[cfg(test)]
    fn finish(mut self) -> Result<(), CloudSourceError> {
        let result = self.state.finish_connect(&self.handle);
        self.settled = true;
        result
    }
}

impl Drop for CloudConnectOwner<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if self.handle.phase.load(Ordering::SeqCst) == CLOUD_CONNECT_PENDING {
            self.handle.cancellation.cancel();
        }
        let _ = self.state.finish_connect(&self.handle);
        self.settled = true;
    }
}

#[derive(Debug)]
struct CloudQueryHandle {
    request_id: String,
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
}

struct CloudQueryOwner<'a> {
    state: &'a CloudAccountsState,
    handle: CloudQueryHandle,
    settled: bool,
}

impl<'a> CloudQueryOwner<'a> {
    fn begin(state: &'a CloudAccountsState, request_id: &str) -> Result<Self, CloudSourceError> {
        Ok(Self {
            state,
            handle: state.begin_query(request_id)?,
            settled: false,
        })
    }

    fn cancellation(&self) -> catdb_connectors::CancellationToken {
        self.handle.cancellation.clone()
    }

    fn finish(mut self) -> Result<(), CloudSourceError> {
        let result = self.state.finish_query(&self.handle);
        self.settled = true;
        result
    }
}

impl Drop for CloudQueryOwner<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.handle.cancellation.cancel();
        let _ = self.state.finish_query(&self.handle);
        self.settled = true;
    }
}

#[derive(Debug, Clone, Copy)]
struct RequestedCatalogPins {
    workspace_id: uuid::Uuid,
    catalog_revision_id: Option<uuid::Uuid>,
    schema_version_id: Option<uuid::Uuid>,
}

/// Process-local owner of the cloud metadata tables within the Library's
/// configured CatDB catalog. The account registry remains authoritative for
/// source lifecycle; this runtime is created lazily so application setup never
/// performs provider I/O.
#[derive(Default)]
struct CloudSyncRuntime {
    location: Mutex<Option<PathBuf>>,
    coordinator: tokio::sync::Mutex<Option<crate::cloud_documents::CloudSyncCoordinator>>,
    refresh_schedules: Mutex<HashMap<String, CloudRefreshSchedule>>,
}

#[derive(Clone)]
struct CloudRefreshSchedule {
    cancellation: catdb_connectors::CancellationToken,
}

impl CloudSyncRuntime {
    fn configure(&self, location: PathBuf) -> Result<(), CloudSourceError> {
        let mut configured = self
            .location
            .lock()
            .map_err(|_| CloudSourceError::PersistFailed)?;
        match configured.as_ref() {
            Some(existing) if existing != &location => Err(CloudSourceError::PersistFailed),
            Some(_) => Ok(()),
            None => {
                *configured = Some(location);
                Ok(())
            }
        }
    }

    async fn coordinator(
        &self,
    ) -> Result<crate::cloud_documents::CloudSyncCoordinator, CloudSourceError> {
        let mut cached = self.coordinator.lock().await;
        if let Some(coordinator) = cached.as_ref() {
            return Ok(coordinator.clone());
        }
        let path = self
            .location
            .lock()
            .map_err(|_| CloudSourceError::PersistFailed)?
            .clone()
            .ok_or(CloudSourceError::QueryCatalogUnavailable)?;
        let store = tauri::async_runtime::spawn_blocking(move || {
            crate::cloud_documents::store::CloudDocumentStore::open(path)
        })
        .await
        .map_err(|_| CloudSourceError::PersistFailed)?
        .map_err(|_| CloudSourceError::PersistFailed)?;
        let coordinator =
            crate::cloud_documents::CloudSyncCoordinator::with_source_owned_pins(Arc::new(store));
        *cached = Some(coordinator.clone());
        Ok(coordinator)
    }

    async fn existing_coordinator(&self) -> Option<crate::cloud_documents::CloudSyncCoordinator> {
        self.coordinator.lock().await.clone()
    }

    /// Replaces the source's recurring metadata refresh task. The source must
    /// already have an activated coordinator owner; lifecycle callers cancel
    /// this task before pausing, retiring, or replacing that owner.
    fn schedule_background_refresh(
        &self,
        coordinator: crate::cloud_documents::CloudSyncCoordinator,
        source_id: catdb_connectors::SourceId,
    ) -> Result<(), CloudSourceError> {
        self.schedule_background_refresh_with_interval(
            coordinator,
            source_id,
            CLOUD_SYNC_REFRESH_INTERVAL,
        )
    }

    #[cfg(test)]
    fn schedule_background_refresh_for_test(
        &self,
        coordinator: crate::cloud_documents::CloudSyncCoordinator,
        source_id: catdb_connectors::SourceId,
        interval: Duration,
    ) -> Result<(), CloudSourceError> {
        self.schedule_background_refresh_with_interval(coordinator, source_id, interval)
    }

    fn schedule_background_refresh_with_interval(
        &self,
        coordinator: crate::cloud_documents::CloudSyncCoordinator,
        source_id: catdb_connectors::SourceId,
        interval: Duration,
    ) -> Result<(), CloudSourceError> {
        let schedule = CloudRefreshSchedule {
            cancellation: catdb_connectors::CancellationToken::new(),
        };
        let previous = self
            .refresh_schedules
            .lock()
            .map_err(|_| CloudSourceError::PersistFailed)?
            .insert(source_id.as_str().to_owned(), schedule.clone());
        if let Some(previous) = previous {
            previous.cancellation.cancel();
        }
        Self::spawn_background_refresh(coordinator, source_id, schedule.cancellation, interval);
        Ok(())
    }

    /// Cancels and removes the exact source's recurring task. It intentionally
    /// happens before the coordinator lifecycle fence so a late wake cannot
    /// reclaim a paused, retired, or replacement owner.
    fn cancel_background_refresh(
        &self,
        source_id: &catdb_connectors::SourceId,
    ) -> Result<(), CloudSourceError> {
        if let Some(schedule) = self
            .refresh_schedules
            .lock()
            .map_err(|_| CloudSourceError::PersistFailed)?
            .remove(source_id.as_str())
        {
            schedule.cancellation.cancel();
        }
        Ok(())
    }

    fn spawn_background_refresh(
        coordinator: crate::cloud_documents::CloudSyncCoordinator,
        source_id: catdb_connectors::SourceId,
        cancellation: catdb_connectors::CancellationToken,
        interval: Duration,
    ) {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => return,
                    _ = tokio::time::sleep(interval) => {}
                }
                if cancellation.is_cancelled() {
                    return;
                }
                match coordinator.refresh(&source_id).await {
                    Ok(()) => {}
                    Err(crate::cloud_documents::CloudSyncError::UnknownSource)
                    | Err(crate::cloud_documents::CloudSyncError::Retired) => return,
                    Err(_) => return,
                }
            }
        });
    }
}

pub(crate) struct CloudAccountsState {
    lifecycle_commits: Arc<Mutex<()>>,
    catalog: tokio::sync::Mutex<Option<CloudCatalogAuthority>>,
    catalog_location: Mutex<Option<(PathBuf, uuid::Uuid)>>,
    cloud_sync: Arc<CloudSyncRuntime>,
    active_connects: Arc<Mutex<HashMap<CloudProvider, ActiveCloudConnect>>>,
    active_queries: Mutex<HashMap<String, ActiveCloudQuery>>,
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    library_continuations: Mutex<HashMap<String, StoredLibraryContinuation>>,
    next_connect_serial: AtomicU64,
    next_query_serial: AtomicU64,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct CloudPointReadAdmission {
    expected: CloudSourceRecord,
    registry: LibraryStore,
    cursor_key: [u8; 32],
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct CurrentCloudDocumentAuthority {
    expected: CloudSourceRecord,
    registry: LibraryStore,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[async_trait::async_trait]
impl crate::cloud_documents::service::CloudDocumentReadAuthority for CurrentCloudDocumentAuthority {
    async fn revalidate(&self) -> Result<(), crate::cloud_documents::CloudDocumentError> {
        let expected = self.expected.clone();
        let registry = self.registry.clone();
        tauri::async_runtime::spawn_blocking(move || {
            registry.read_cloud(|sources| validate_current_cloud_record(sources, &expected))
        })
        .await
        .map_err(|_| crate::cloud_documents::CloudDocumentError::ProtocolFailed)?
        .map_err(map_cloud_point_read_error)
    }
}

impl Default for CloudAccountsState {
    fn default() -> Self {
        Self {
            lifecycle_commits: Arc::new(Mutex::new(())),
            catalog: tokio::sync::Mutex::new(None),
            catalog_location: Mutex::new(None),
            cloud_sync: Arc::new(CloudSyncRuntime::default()),
            active_connects: Arc::new(Mutex::new(HashMap::new())),
            active_queries: Mutex::new(HashMap::new()),
            #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
            library_continuations: Mutex::new(HashMap::new()),
            next_connect_serial: AtomicU64::new(1),
            next_query_serial: AtomicU64::new(1),
        }
    }
}

impl CloudAccountsState {
    pub(crate) fn configure_catalog(&self, app: &AppHandle) -> Result<(), CloudSourceError> {
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|_| CloudSourceError::PersistFailed)?;
        let location = (
            app_data.join("cloud-library.catdb"),
            cloud_workspace_id(&app.config().identifier),
        );
        let mut configured = self
            .catalog_location
            .lock()
            .map_err(|_| CloudSourceError::PersistFailed)?;
        match configured.as_ref() {
            Some(existing) if existing != &location => Err(CloudSourceError::PersistFailed),
            Some(_) => self.cloud_sync.configure(location.0.clone()),
            None => {
                self.cloud_sync.configure(location.0.clone())?;
                *configured = Some(location);
                Ok(())
            }
        }
    }

    pub(crate) async fn read_cloud_markdown(
        &self,
        app: AppHandle,
        source_id: String,
        record_key: String,
    ) -> Result<
        crate::cloud_documents::CloudMarkdownDocument,
        crate::cloud_documents::CloudDocumentError,
    > {
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = (app, source_id, record_key);
            return Err(crate::cloud_documents::CloudDocumentError::ConnectorUnavailable);
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        {
            let admitted_source_id = source_id.clone();
            let admission = blocking_cloud_operation(app, move |registry, vault| {
                let expected = registry.read_cloud(|sources| {
                    sources
                        .iter()
                        .find(|source| source.id() == admitted_source_id)
                        .cloned()
                        .ok_or(CloudSourceError::SourceNotFound)
                })?;
                if !expected.enabled() {
                    return Err(CloudSourceError::SourcePaused);
                }
                let credential = vault
                    .get(&expected.credential_reference)?
                    .ok_or(CloudSourceError::CredentialMissing)?;
                Ok(CloudPointReadAdmission {
                    expected,
                    registry,
                    cursor_key: credential.cursor_key,
                })
            })
            .await
            .map_err(map_cloud_point_read_error)?;
            let library_id = self
                .catalog_location
                .lock()
                .map_err(|_| crate::cloud_documents::CloudDocumentError::ProtocolFailed)?
                .as_ref()
                .map(|(_, workspace_id)| *workspace_id.as_bytes())
                .ok_or(crate::cloud_documents::CloudDocumentError::SyncInProgress)?;
            let coordinator = self
                .cloud_sync
                .coordinator()
                .await
                .map_err(map_cloud_point_read_error)?;
            let source_id = catdb_connectors::SourceId::new(source_id);
            let (source, pins) = coordinator.point_read_owner(&source_id)?;
            let authority = Arc::new(CurrentCloudDocumentAuthority {
                expected: admission.expected,
                registry: admission.registry,
            });
            crate::cloud_documents::service::CloudDocumentService::new(
                coordinator.store(),
                source,
                pins,
                library_id,
                admission.cursor_key,
                authority,
            )
            .read_cloud_markdown(&source_id, &record_key)
            .await
        }
    }

    pub(crate) async fn save_cloud_markdown(
        &self,
        app: AppHandle,
        edit_token: String,
        content: String,
    ) -> Result<
        crate::cloud_documents::CloudMarkdownSaveOutcome,
        crate::cloud_documents::CloudDocumentError,
    > {
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = (app, edit_token, content);
            return Err(crate::cloud_documents::CloudDocumentError::ConnectorUnavailable);
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        {
            let admissions = blocking_cloud_operation(app, move |registry, vault| {
                registry.read_cloud(|sources| {
                    sources
                        .iter()
                        .filter(|source| source.enabled())
                        .map(|expected| {
                            let credential = vault
                                .get(&expected.credential_reference)?
                                .ok_or(CloudSourceError::CredentialMissing)?;
                            Ok(CloudPointReadAdmission {
                                expected: expected.clone(),
                                registry: registry.clone(),
                                cursor_key: credential.cursor_key,
                            })
                        })
                        .collect::<Result<Vec<_>, CloudSourceError>>()
                })
            })
            .await
            .map_err(map_cloud_point_read_error)?;
            let library_id = self
                .catalog_location
                .lock()
                .map_err(|_| crate::cloud_documents::CloudDocumentError::ProtocolFailed)?
                .as_ref()
                .map(|(_, workspace_id)| *workspace_id.as_bytes())
                .ok_or(crate::cloud_documents::CloudDocumentError::SyncInProgress)?;
            let coordinator = self
                .cloud_sync
                .coordinator()
                .await
                .map_err(map_cloud_point_read_error)?;
            for admission in admissions {
                let source_id =
                    catdb_connectors::SourceId::new(admission.expected.id().to_string());
                let Ok((source, pins)) = coordinator.point_read_owner(&source_id) else {
                    continue;
                };
                let authority = Arc::new(CurrentCloudDocumentAuthority {
                    expected: admission.expected,
                    registry: admission.registry,
                });
                let service = crate::cloud_documents::service::CloudDocumentService::new(
                    coordinator.store(),
                    source,
                    pins,
                    library_id,
                    admission.cursor_key,
                    authority,
                );
                match service.save_cloud_markdown(&edit_token, &content).await {
                    Err(crate::cloud_documents::CloudDocumentError::Unauthorized) => continue,
                    outcome => return outcome,
                }
            }
            Err(crate::cloud_documents::CloudDocumentError::Unauthorized)
        }
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    async fn advance_catalog(
        &self,
        authority: &CloudExecutionAuthority,
        requested_revision_id: Option<uuid::Uuid>,
    ) {
        if requested_revision_id
            .is_some_and(|requested| requested.to_string() == authority.pins.catalog_version)
        {
            return;
        }
        let mut cached = self.catalog.lock().await;
        let Some(catalog) = cached.as_mut() else {
            return;
        };
        if catalog.schema_version == authority.pins.schema_version {
            catalog.catalog_version = authority.pins.catalog_version.clone();
        }
    }

    fn begin_connect(
        &self,
        provider: CloudProvider,
    ) -> Result<CloudConnectHandle, CloudSourceError> {
        let serial = self.next_connect_serial.fetch_add(1, Ordering::SeqCst);
        let cancellation = catdb_connectors::CancellationToken::new();
        let phase = Arc::new(AtomicU8::new(CLOUD_CONNECT_PENDING));
        let mut active = self
            .active_connects
            .lock()
            .map_err(|_| CloudSourceError::OauthTransport)?;
        if let Some(retired) = active.insert(
            provider,
            ActiveCloudConnect {
                serial,
                cancellation: cancellation.clone(),
                phase: Arc::clone(&phase),
            },
        ) {
            if retired.phase.load(Ordering::SeqCst) == CLOUD_CONNECT_PENDING {
                retired.cancellation.cancel();
            }
        }
        Ok(CloudConnectHandle {
            provider,
            serial,
            cancellation,
            phase,
        })
    }

    fn finish_connect(&self, connect: &CloudConnectHandle) -> Result<(), CloudSourceError> {
        let mut active = self
            .active_connects
            .lock()
            .map_err(|_| CloudSourceError::OauthTransport)?;
        match active.get(&connect.provider) {
            Some(entry) if entry.serial == connect.serial => {
                active.remove(&connect.provider);
                if connect.cancellation.is_cancelled() {
                    Err(CloudSourceError::OauthCancelled)
                } else {
                    Ok(())
                }
            }
            _ => Err(CloudSourceError::OauthCancelled),
        }
    }

    fn cancel_provider_activity(&self, provider: CloudProvider) -> Result<(), CloudSourceError> {
        if let Some(connect) = self
            .active_connects
            .lock()
            .map_err(|_| CloudSourceError::OauthTransport)?
            .remove(&provider)
        {
            connect.cancellation.cancel();
        }
        let mut queries = self
            .active_queries
            .lock()
            .map_err(|_| CloudSourceError::QueryInvalidResponse)?;
        for (_, query) in queries.drain() {
            query.cancellation.cancel();
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        self.library_continuations
            .lock()
            .map_err(|_| CloudSourceError::QueryInvalidResponse)?
            .clear();
        Ok(())
    }

    fn begin_query(&self, request_id: &str) -> Result<CloudQueryHandle, CloudSourceError> {
        if request_id.is_empty()
            || request_id.len() > MAX_CLOUD_REQUEST_ID_BYTES
            || request_id.chars().any(char::is_control)
        {
            return Err(CloudSourceError::QueryInvalid {
                detail: "cloud query request ID is invalid".to_string(),
            });
        }
        let serial = self.next_query_serial.fetch_add(1, Ordering::SeqCst);
        let cancellation = catdb_connectors::CancellationToken::new();
        let mut active = self
            .active_queries
            .lock()
            .map_err(|_| CloudSourceError::QueryInvalidResponse)?;
        if let Some(retired) = active.insert(
            request_id.to_string(),
            ActiveCloudQuery {
                serial,
                cancellation: cancellation.clone(),
            },
        ) {
            retired.cancellation.cancel();
        }
        Ok(CloudQueryHandle {
            request_id: request_id.to_string(),
            serial,
            cancellation,
        })
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    fn store_library_continuation(
        &self,
        continuation: StoredLibraryContinuation,
    ) -> Result<(), CloudSourceError> {
        let token = continuation.continuation.token().as_str().to_string();
        let now = Instant::now();
        let mut stored = self
            .library_continuations
            .lock()
            .map_err(|_| CloudSourceError::QueryInvalidResponse)?;
        stored.retain(|_, entry| entry.continuation.expires_at() > now);
        if stored.contains_key(&token) {
            return Err(CloudSourceError::QueryInvalidResponse);
        }
        while stored.len() >= MAX_LIBRARY_CONTINUATIONS {
            let Some(oldest) = stored
                .iter()
                .min_by_key(|(_, entry)| entry.continuation.expires_at())
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            stored.remove(&oldest);
        }
        stored.insert(token, continuation);
        Ok(())
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    fn take_library_continuation(
        &self,
        cursor: &catdb_api_types::EntityCursor,
    ) -> Result<StoredLibraryContinuation, CloudSourceError> {
        self.take_library_continuation_at(cursor, Instant::now())
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    fn take_library_continuation_at(
        &self,
        cursor: &catdb_api_types::EntityCursor,
        now: Instant,
    ) -> Result<StoredLibraryContinuation, CloudSourceError> {
        let mut stored = self
            .library_continuations
            .lock()
            .map_err(|_| CloudSourceError::QueryInvalidResponse)?;
        stored.retain(|_, entry| entry.continuation.expires_at() > now);
        stored
            .remove(cursor.as_str())
            .ok_or_else(|| query_invalid("Library continuation is invalid, expired, or replayed"))
    }

    fn finish_query(&self, query: &CloudQueryHandle) -> Result<(), CloudSourceError> {
        let mut active = self
            .active_queries
            .lock()
            .map_err(|_| CloudSourceError::QueryInvalidResponse)?;
        match active.get(&query.request_id) {
            Some(entry) if entry.serial == query.serial => {
                active.remove(&query.request_id);
                if query.cancellation.is_cancelled() {
                    Err(CloudSourceError::QueryCancelled)
                } else {
                    Ok(())
                }
            }
            Some(_) => Err(CloudSourceError::QueryRetired),
            None if query.cancellation.is_cancelled() => Err(CloudSourceError::QueryCancelled),
            None => Err(CloudSourceError::QueryRetired),
        }
    }

    fn cancel_query(&self, request_id: &str) -> bool {
        let Ok(mut active) = self.active_queries.lock() else {
            return false;
        };
        let Some(query) = active.remove(request_id) else {
            return false;
        };
        query.cancellation.cancel();
        true
    }
}

#[derive(Clone)]
struct KeyringCredentialVault {
    service: String,
}

impl KeyringCredentialVault {
    fn new(identifier: &str) -> Result<Self, CloudSourceError> {
        if identifier.is_empty()
            || identifier.len() > 200
            || identifier.chars().any(char::is_control)
        {
            return Err(CloudSourceError::CredentialUnavailable);
        }
        Ok(Self {
            service: format!("{identifier}.cloud-credentials"),
        })
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn entry(&self, reference: &str) -> Result<keyring::Entry, CloudSourceError> {
        keyring::Entry::new(&self.service, reference)
            .map_err(|_| CloudSourceError::CredentialUnavailable)
    }
}

impl CredentialVault for KeyringCredentialVault {
    fn get(&self, reference: &str) -> Result<Option<SecretBundle>, CloudSourceError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let encoded = match self.entry(reference)?.get_password() {
                Ok(encoded) => Zeroizing::new(encoded),
                Err(keyring::Error::NoEntry) => return Ok(None),
                Err(_) => return Err(CloudSourceError::CredentialUnavailable),
            };
            let secret: SecretBundle = serde_json::from_str(encoded.as_str())
                .map_err(|_| CloudSourceError::CredentialUnavailable)?;
            secret
                .validate()
                .map_err(|_| CloudSourceError::CredentialUnavailable)?;
            Ok(Some(secret))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = reference;
            Err(CloudSourceError::CredentialUnavailable)
        }
    }

    fn set(&self, reference: &str, secret: &SecretBundle) -> Result<(), CloudSourceError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let encoded = Zeroizing::new(
                serde_json::to_string(secret)
                    .map_err(|_| CloudSourceError::CredentialUnavailable)?,
            );
            self.entry(reference)?
                .set_password(encoded.as_str())
                .map_err(|_| CloudSourceError::CredentialUnavailable)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (reference, secret);
            Err(CloudSourceError::CredentialUnavailable)
        }
    }

    fn delete(&self, reference: &str) -> Result<(), CloudSourceError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            match self.entry(reference)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(CloudSourceError::CredentialUnavailable),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = reference;
            Err(CloudSourceError::CredentialUnavailable)
        }
    }

    fn cleanup_references(&self, provider: CloudProvider) -> Result<Vec<String>, CloudSourceError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let encoded = match self
                .entry(provider.credential_cleanup_reference())?
                .get_password()
            {
                Ok(encoded) => Zeroizing::new(encoded),
                Err(keyring::Error::NoEntry) => return Ok(Vec::new()),
                Err(_) => return Err(CloudSourceError::CredentialUnavailable),
            };
            let references: Vec<String> = serde_json::from_str(encoded.as_str())
                .map_err(|_| CloudSourceError::CredentialUnavailable)?;
            validate_cleanup_references(&references)?;
            Ok(references)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = provider;
            Err(CloudSourceError::CredentialUnavailable)
        }
    }

    fn set_cleanup_references(
        &self,
        provider: CloudProvider,
        references: &[String],
    ) -> Result<(), CloudSourceError> {
        validate_cleanup_references(references)?;
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let entry = self.entry(provider.credential_cleanup_reference())?;
            if references.is_empty() {
                return match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(_) => Err(CloudSourceError::CredentialUnavailable),
                };
            }
            let encoded = Zeroizing::new(
                serde_json::to_string(references)
                    .map_err(|_| CloudSourceError::CredentialUnavailable)?,
            );
            entry
                .set_password(encoded.as_str())
                .map_err(|_| CloudSourceError::CredentialUnavailable)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (provider, references);
            Err(CloudSourceError::CredentialUnavailable)
        }
    }
}

async fn blocking_cloud_operation<T: Send + 'static>(
    app: AppHandle,
    operation: impl FnOnce(LibraryStore, KeyringCredentialVault) -> Result<T, CloudSourceError>
        + Send
        + 'static,
) -> Result<T, CloudSourceError> {
    tauri::async_runtime::spawn_blocking(move || cloud_operation(app, operation))
        .await
        .map_err(|_| CloudSourceError::PersistFailed)?
}

fn cloud_operation<T>(
    app: AppHandle,
    operation: impl FnOnce(LibraryStore, KeyringCredentialVault) -> Result<T, CloudSourceError>,
) -> Result<T, CloudSourceError> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|_| CloudSourceError::PersistFailed)?;
    let vault = KeyringCredentialVault::new(&app.config().identifier)?;
    operation(LibraryStore::new(app_data.join("library.json")), vault)
}

#[cfg(feature = "e2e-build")]
fn e2e_drive_record() -> CloudSourceRecord {
    CloudSourceRecord {
        id: E2E_DRIVE_RECORD_ID.to_string(),
        provider: CloudProvider::GoogleDrive,
        label: CloudProvider::GoogleDrive.label().to_string(),
        enabled: true,
        credential_reference: E2E_DRIVE_CREDENTIAL_REFERENCE.to_string(),
        config: CloudSourceConfig::GoogleDrive {
            selections: vec![DriveItemSelection {
                id: E2E_DRIVE_FILE_ID.to_string(),
                resource_key: None,
                folder: false,
                root: false,
            }],
        },
        authorization_profile: CloudAuthorizationProfile::ReadOnlyV1,
        contract: Some(crate::library::LibrarySourceContract::cloud(
            CloudProvider::GoogleDrive,
        )),
    }
}

#[cfg(feature = "e2e-build")]
pub(crate) fn seed_e2e_cloud_membership(app: &AppHandle) -> Result<(), CloudSourceError> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|_| CloudSourceError::PersistFailed)?;
    let registry = LibraryStore::new(app_data.join("library.json"));
    let vault = KeyringCredentialVault::new(&app.config().identifier)?;
    let credential = SecretBundle::new(
        "mdreader-e2e-drive-access",
        "mdreader-e2e-drive-refresh",
        4_102_444_800,
        [7; 32],
    )?;
    vault.set(E2E_DRIVE_CREDENTIAL_REFERENCE, &credential)?;
    let record = e2e_drive_record();
    registry.update_cloud(|sources| {
        sources.retain(|source| source.provider != CloudProvider::GoogleDrive);
        sources.push(record);
        Ok(())
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Clone)]
struct CloudCredentialAuthority {
    expected: CloudSourceRecord,
    registry: LibraryStore,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Clone)]
pub(crate) struct CloudCredentialResolver {
    provider: CloudProvider,
    reference: String,
    vault: Arc<dyn CredentialVault>,
    admitted: Option<Arc<Mutex<SecretBundle>>>,
    authority: Option<CloudCredentialAuthority>,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
impl CloudCredentialResolver {
    #[cfg(test)]
    fn new(provider: CloudProvider, reference: String, vault: Arc<dyn CredentialVault>) -> Self {
        Self {
            provider,
            reference,
            vault,
            admitted: None,
            authority: None,
        }
    }

    pub(crate) fn from_admitted(
        record: CloudSourceRecord,
        credential: SecretBundle,
        vault: Arc<dyn CredentialVault>,
        registry: LibraryStore,
    ) -> Self {
        Self {
            provider: record.provider,
            reference: record.credential_reference.clone(),
            vault,
            admitted: Some(Arc::new(Mutex::new(credential))),
            authority: Some(CloudCredentialAuthority {
                expected: record,
                registry,
            }),
        }
    }

    pub(crate) async fn validate_authority(&self) -> Result<(), CloudSourceError> {
        let Some(authority) = self.authority.clone() else {
            return Ok(());
        };
        tauri::async_runtime::spawn_blocking(move || {
            authority
                .registry
                .read_cloud(|sources| validate_current_cloud_record(sources, &authority.expected))
        })
        .await
        .map_err(|_| CloudSourceError::PersistFailed)?
    }

    pub(crate) async fn validate_document_authority(&self) -> Result<(), CloudSourceError> {
        let Some(authority) = self.authority.clone() else {
            return Ok(());
        };
        tauri::async_runtime::spawn_blocking(move || {
            authority.registry.read_cloud(|sources| {
                let Some(current) = sources
                    .iter()
                    .find(|source| source.provider == authority.expected.provider)
                else {
                    return Err(CloudSourceError::SourceNotFound);
                };
                if !current.enabled {
                    return Err(CloudSourceError::SourcePaused);
                }
                if current != &authority.expected {
                    return Err(CloudSourceError::QueryRetired);
                }
                Ok(())
            })
        })
        .await
        .map_err(|_| CloudSourceError::PersistFailed)?
    }

    async fn load(&self) -> Result<SecretBundle, CloudSourceError> {
        if let Some(admitted) = &self.admitted {
            return admitted
                .lock()
                .map_err(|_| CloudSourceError::CredentialUnavailable)
                .map(|credential| credential.clone());
        }
        let vault = self.vault.clone();
        let reference = self.reference.clone();
        tauri::async_runtime::spawn_blocking(move || vault.get(&reference))
            .await
            .map_err(|_| CloudSourceError::CredentialUnavailable)?
            .and_then(|secret| secret.ok_or(CloudSourceError::CredentialMissing))
    }

    async fn store(&self, secret: SecretBundle) -> Result<(), CloudSourceError> {
        let vault = self.vault.clone();
        let reference = self.reference.clone();
        let stored = secret.clone();
        let authority = self.authority.clone();
        tauri::async_runtime::spawn_blocking(move || match authority {
            Some(authority) => authority.registry.read_cloud(|sources| {
                validate_current_cloud_record(sources, &authority.expected)?;
                vault.set(&reference, &stored)
            }),
            None => vault.set(&reference, &stored),
        })
        .await
        .map_err(|_| CloudSourceError::CredentialUnavailable)??;
        if let Some(admitted) = &self.admitted {
            *admitted
                .lock()
                .map_err(|_| CloudSourceError::CredentialUnavailable)? = secret;
        }
        Ok(())
    }

    async fn refresh_bundle(
        &self,
        refresh_credential: &str,
    ) -> Result<SecretBundle, CloudSourceError> {
        let mut secret = self.load().await?;
        if secret.refresh_token != refresh_credential {
            return Err(CloudSourceError::CredentialMissing);
        }
        let profile = self
            .authority
            .as_ref()
            .map(|authority| authority.expected.authorization_profile)
            .unwrap_or_default();
        let refreshed = exchange_refresh_token(self.provider, profile, refresh_credential).await?;
        secret.access_token = refreshed.access_token.to_string();
        secret.expires_at_unix_seconds = refreshed.expires_at_unix_seconds;
        secret.validate()?;
        self.store(secret.clone()).await?;
        Ok(secret)
    }
}

/// One admitted source plus the exact catalog-derived pins carried into the
/// local materialization. The foreground Library query path is never reused
/// for synchronization.
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct AdmittedMarkdownSyncSource {
    source: Arc<dyn crate::cloud_documents::MarkdownDocumentSource>,
    pins: crate::cloud_documents::store::CloudDocumentVersionPins,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn ensure_cloud_document_connector_available(
    provider: CloudProvider,
) -> Result<(), crate::cloud_documents::CloudDocumentError> {
    match provider {
        #[cfg(not(feature = "google-drive-connector"))]
        CloudProvider::GoogleDrive => {
            Err(crate::cloud_documents::CloudDocumentError::ConnectorUnavailable)
        }
        #[cfg(not(feature = "dropbox-connector"))]
        CloudProvider::Dropbox => {
            Err(crate::cloud_documents::CloudDocumentError::ConnectorUnavailable)
        }
        _ => Ok(()),
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn with_available_cloud_document_connector<T, F, Fut>(
    provider: CloudProvider,
    remainder: F,
) -> Result<T, crate::cloud_documents::CloudDocumentError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, crate::cloud_documents::CloudDocumentError>>,
{
    ensure_cloud_document_connector_available(provider)?;
    remainder().await
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn markdown_sync_pins(
    authority: &CloudExecutionAuthority,
) -> Result<crate::cloud_documents::store::CloudDocumentVersionPins, CloudSourceError> {
    let schema = parse_pin(&authority.pins.schema_version)?;
    let mapping = parse_pin(&authority.pins.mapping_version)?;
    let source = parse_pin(&authority.pins.source_version)?;
    crate::cloud_documents::store::CloudDocumentVersionPins::new(
        schema.as_bytes(),
        mapping.as_bytes(),
        source.as_bytes(),
    )
    .map_err(|_| CloudSourceError::QueryCatalogInvalid)
}

/// Opens a native Markdown adapter only after validating the enabled account,
/// resolving its catalog authority, and converting UUID pins to the store's
/// opaque byte contract.
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn open_admitted_cloud_markdown_sync_source(
    app: AppHandle,
    provider: CloudProvider,
    state: &CloudAccountsState,
) -> Result<AdmittedMarkdownSyncSource, CloudSourceError> {
    let admitted = blocking_cloud_operation(app.clone(), move |registry, vault| {
        admit_cloud_read(registry, vault, provider)
    })
    .await?;
    let catalog = ensure_cloud_catalog(&app, state).await?;
    let AdmittedCloudRead {
        record,
        credential,
        read_authority,
        registry,
        vault,
    } = admitted;
    let profile = record.authorization_profile();
    let source_id = record.id.clone();
    let credential_reference = record.credential_reference.clone();
    let configuration = record.config.clone();
    let resolver = Arc::new(CloudCredentialResolver::from_admitted(
        record.clone(),
        credential,
        Arc::new(vault),
        registry,
    ));
    match configuration {
        #[cfg(feature = "google-drive-connector")]
        CloudSourceConfig::GoogleDrive { selections } => {
            let reference =
                catdb_connector_google_drive::DriveSecretReference::new(credential_reference)
                    .map_err(|error| query_invalid(error.to_string()))?;
            let config = catdb_connector_google_drive::DriveSourceConfig::new(
                source_id,
                "mdreader",
                "Document",
                drive_selections(selections)?,
                reference,
            )
            .map_err(|error| query_invalid(error.to_string()))?
            .with_content_scope(catdb_connector_google_drive::DriveContentScope::Markdown);
            let spec = catdb_connector_google_drive::PinnedDriveSourceSpec::new(
                config,
                &catalog.schema_version,
            )
            .map_err(|error| query_invalid(error.to_string()))?;
            let source = Arc::new(
                catdb_connector_google_drive::DriveSource::open_pinned(spec, resolver.clone())
                    .await
                    .map_err(|error| query_failed(error.to_string()))?,
            );
            let authority_source = Arc::clone(&source);
            let authority_catalog = catalog.clone();
            let authority_read = read_authority.clone();
            let authority = tauri::async_runtime::spawn_blocking(move || {
                resolve_cloud_execution_authority(
                    provider,
                    authority_source.as_ref(),
                    &authority_catalog,
                    &authority_read,
                    None,
                )
            })
            .await
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)??;
            state.advance_catalog(&authority, None).await;
            Ok(AdmittedMarkdownSyncSource {
                pins: markdown_sync_pins(&authority)?,
                source: Arc::new(
                    crate::cloud_documents::source::DriveMarkdownDocumentSource::new(
                        source,
                        resolver,
                        profile,
                        native_markdown_write_acceptance(CloudProvider::GoogleDrive),
                    ),
                ),
            })
        }
        #[cfg(feature = "dropbox-connector")]
        CloudSourceConfig::Dropbox => {
            let reference =
                catdb_connector_dropbox::DropboxSecretReference::new(credential_reference)
                    .map_err(|error| query_invalid(error.to_string()))?;
            let config = catdb_connector_dropbox::DropboxSourceConfig::new(
                source_id,
                "mdreader",
                "Document",
                "",
                catdb_connector_dropbox::DropboxAuthorizationMode::FullDropbox,
                reference,
            )
            .map_err(|error| query_invalid(error.to_string()))?
            .with_content_scope(catdb_connector_dropbox::DropboxContentScope::Markdown);
            let spec = catdb_connector_dropbox::PinnedDropboxSourceSpec::new(
                config,
                &catalog.schema_version,
            )
            .map_err(|error| query_invalid(error.to_string()))?;
            let source = Arc::new(
                catdb_connector_dropbox::DropboxSource::open_pinned(spec, resolver.clone())
                    .await
                    .map_err(|error| query_failed(error.to_string()))?,
            );
            let authority_source = Arc::clone(&source);
            let authority_catalog = catalog.clone();
            let authority_read = read_authority.clone();
            let authority = tauri::async_runtime::spawn_blocking(move || {
                resolve_cloud_execution_authority(
                    provider,
                    authority_source.as_ref(),
                    &authority_catalog,
                    &authority_read,
                    None,
                )
            })
            .await
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)??;
            state.advance_catalog(&authority, None).await;
            Ok(AdmittedMarkdownSyncSource {
                pins: markdown_sync_pins(&authority)?,
                source: Arc::new(
                    crate::cloud_documents::source::DropboxMarkdownDocumentSource::new(
                        source,
                        resolver,
                        profile,
                        native_markdown_write_acceptance(CloudProvider::Dropbox),
                    ),
                ),
            })
        }
        #[cfg(not(feature = "google-drive-connector"))]
        CloudSourceConfig::GoogleDrive { .. } => Err(CloudSourceError::ConnectorUnavailable),
        #[cfg(not(feature = "dropbox-connector"))]
        CloudSourceConfig::Dropbox => Err(CloudSourceError::ConnectorUnavailable),
    }
}

#[cfg(feature = "google-drive-connector")]
#[async_trait::async_trait]
impl catdb_connector_google_drive::DriveSecretResolver for CloudCredentialResolver {
    async fn resolve(
        &self,
        reference: &catdb_connector_google_drive::DriveSecretReference,
    ) -> Result<
        catdb_connector_google_drive::DriveRuntimeSecrets,
        catdb_connector_google_drive::DriveConnectorError,
    > {
        let expected =
            catdb_connector_google_drive::DriveSecretReference::new(self.reference.clone())?;
        if self.provider != CloudProvider::GoogleDrive || reference != &expected {
            return Err(catdb_connector_google_drive::DriveConnectorError::InvalidSecret);
        }
        let secret = self.load().await.map_err(map_drive_credential_error)?;
        drive_runtime_secrets(secret)
    }

    async fn refresh(
        &self,
        reference: &catdb_connector_google_drive::DriveSecretReference,
        credential: &catdb_connector_google_drive::DriveRefreshCredential,
    ) -> Result<
        catdb_connector_google_drive::DriveAccessGrant,
        catdb_connector_google_drive::DriveConnectorError,
    > {
        let expected =
            catdb_connector_google_drive::DriveSecretReference::new(self.reference.clone())?;
        if self.provider != CloudProvider::GoogleDrive || reference != &expected {
            return Err(catdb_connector_google_drive::DriveConnectorError::InvalidSecret);
        }
        let secret = self
            .refresh_bundle(credential.expose_for_exchange())
            .await
            .map_err(map_drive_credential_error)?;
        drive_access_grant(&secret)
    }
}

#[cfg(feature = "dropbox-connector")]
#[async_trait::async_trait]
impl catdb_connector_dropbox::DropboxSecretResolver for CloudCredentialResolver {
    async fn resolve(
        &self,
        reference: &catdb_connector_dropbox::DropboxSecretReference,
    ) -> Result<
        catdb_connector_dropbox::DropboxRuntimeSecrets,
        catdb_connector_dropbox::DropboxConnectorError,
    > {
        let expected =
            catdb_connector_dropbox::DropboxSecretReference::new(self.reference.clone())?;
        if self.provider != CloudProvider::Dropbox || reference != &expected {
            return Err(catdb_connector_dropbox::DropboxConnectorError::InvalidSecret);
        }
        let secret = self.load().await.map_err(map_dropbox_credential_error)?;
        dropbox_runtime_secrets(secret)
    }

    async fn refresh(
        &self,
        reference: &catdb_connector_dropbox::DropboxSecretReference,
        credential: &catdb_connector_dropbox::DropboxRefreshCredential,
    ) -> Result<
        catdb_connector_dropbox::DropboxAccessGrant,
        catdb_connector_dropbox::DropboxConnectorError,
    > {
        let expected =
            catdb_connector_dropbox::DropboxSecretReference::new(self.reference.clone())?;
        if self.provider != CloudProvider::Dropbox || reference != &expected {
            return Err(catdb_connector_dropbox::DropboxConnectorError::InvalidSecret);
        }
        let secret = self
            .refresh_bundle(credential.expose_for_exchange())
            .await
            .map_err(map_dropbox_credential_error)?;
        dropbox_access_grant(&secret)
    }
}

#[cfg(feature = "google-drive-connector")]
fn drive_access_grant(
    secret: &SecretBundle,
) -> Result<
    catdb_connector_google_drive::DriveAccessGrant,
    catdb_connector_google_drive::DriveConnectorError,
> {
    catdb_connector_google_drive::DriveAccessGrant::new(
        catdb_connector_google_drive::DriveAccessToken::new(secret.access_token.clone())?,
        Some(
            secret
                .expires_at()
                .map_err(|_| catdb_connector_google_drive::DriveConnectorError::InvalidSecret)?,
        ),
    )
}

#[cfg(feature = "google-drive-connector")]
fn drive_runtime_secrets(
    secret: SecretBundle,
) -> Result<
    catdb_connector_google_drive::DriveRuntimeSecrets,
    catdb_connector_google_drive::DriveConnectorError,
> {
    Ok(
        catdb_connector_google_drive::DriveRuntimeSecrets::refreshable(
            drive_access_grant(&secret)?,
            catdb_connector_google_drive::DriveRefreshCredential::new(
                secret.refresh_token.clone(),
            )?,
            catdb_connector_google_drive::DriveCursorKey::new(secret.cursor_key),
        ),
    )
}

#[cfg(feature = "dropbox-connector")]
fn dropbox_access_grant(
    secret: &SecretBundle,
) -> Result<
    catdb_connector_dropbox::DropboxAccessGrant,
    catdb_connector_dropbox::DropboxConnectorError,
> {
    catdb_connector_dropbox::DropboxAccessGrant::new(
        catdb_connector_dropbox::DropboxAccessToken::new(secret.access_token.clone())?,
        Some(
            secret
                .expires_at()
                .map_err(|_| catdb_connector_dropbox::DropboxConnectorError::InvalidSecret)?,
        ),
    )
}

#[cfg(feature = "dropbox-connector")]
fn dropbox_runtime_secrets(
    secret: SecretBundle,
) -> Result<
    catdb_connector_dropbox::DropboxRuntimeSecrets,
    catdb_connector_dropbox::DropboxConnectorError,
> {
    Ok(catdb_connector_dropbox::DropboxRuntimeSecrets::refreshable(
        dropbox_access_grant(&secret)?,
        catdb_connector_dropbox::DropboxRefreshCredential::new(secret.refresh_token.clone())?,
        catdb_connector_dropbox::DropboxCursorKey::new(secret.cursor_key),
    ))
}

#[cfg(feature = "google-drive-connector")]
fn map_drive_credential_error(
    error: CloudSourceError,
) -> catdb_connector_google_drive::DriveConnectorError {
    match error {
        CloudSourceError::OauthTransport => {
            catdb_connector_google_drive::DriveConnectorError::Transport
        }
        CloudSourceError::OauthRateLimited { retry_after_ms } => {
            catdb_connector_google_drive::DriveConnectorError::RateLimited {
                retry_after_ms: retry_after_ms.map(u64::from),
            }
        }
        _ => catdb_connector_google_drive::DriveConnectorError::InvalidSecret,
    }
}

#[cfg(feature = "dropbox-connector")]
fn map_dropbox_credential_error(
    error: CloudSourceError,
) -> catdb_connector_dropbox::DropboxConnectorError {
    match error {
        CloudSourceError::OauthTransport => {
            catdb_connector_dropbox::DropboxConnectorError::Transport
        }
        CloudSourceError::OauthRateLimited { retry_after_ms } => {
            catdb_connector_dropbox::DropboxConnectorError::RateLimited {
                retry_after_ms: retry_after_ms.map(u64::from),
            }
        }
        _ => catdb_connector_dropbox::DropboxConnectorError::InvalidSecret,
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct AdmittedCloudRead<V> {
    record: CloudSourceRecord,
    credential: SecretBundle,
    read_authority: CloudReadAuthority,
    registry: LibraryStore,
    vault: V,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct AdmittedCloudQuery {
    record: CloudSourceRecord,
    read_authority: CloudReadAuthority,
    catalog: CloudCatalogAuthority,
    resolver: Arc<CloudCredentialResolver>,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn admit_cloud_read<V: CredentialVault>(
    registry: LibraryStore,
    vault: V,
    provider: CloudProvider,
) -> Result<AdmittedCloudRead<V>, CloudSourceError> {
    let (record, credential) =
        CloudAccountService::new(registry.clone(), &vault).admitted_record(provider)?;
    let read_authority = CloudReadAuthority::from_record(&record)?;
    Ok(AdmittedCloudRead {
        record,
        credential,
        read_authority,
        registry,
        vault,
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn open_admitted_cloud_query<V, A, C, CFut>(
    admission: A,
    catalog: C,
) -> Result<AdmittedCloudQuery, CloudSourceError>
where
    V: CredentialVault + 'static,
    A: std::future::Future<Output = Result<AdmittedCloudRead<V>, CloudSourceError>>,
    C: FnOnce() -> CFut,
    CFut: std::future::Future<Output = Result<CloudCatalogAuthority, CloudSourceError>>,
{
    let AdmittedCloudRead {
        record,
        credential,
        read_authority,
        registry,
        vault,
    } = admission.await?;
    let catalog = catalog().await?;
    let resolver = Arc::new(CloudCredentialResolver::from_admitted(
        record.clone(),
        credential,
        Arc::new(vault),
        registry,
    ));
    Ok(AdmittedCloudQuery {
        record,
        read_authority,
        catalog,
        resolver,
    })
}

async fn query_cloud_documents_inner(
    app: AppHandle,
    request_id: String,
    provider: CloudProvider,
    filters: Vec<CloudFilterInput>,
    fields: Vec<String>,
    requested_pins: Option<RequestedCatalogPins>,
    state: &CloudAccountsState,
) -> Result<CloudDocumentQueryResponse, CloudSourceError> {
    #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
    {
        let _ = (
            app,
            request_id,
            provider,
            filters,
            fields,
            requested_pins,
            state,
        );
        Err(CloudSourceError::ConnectorUnavailable)
    }
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    {
        let query = CloudQueryOwner::begin(state, &request_id)?;
        let cancellation = query.cancellation();
        let result = async {
            if cancellation.is_cancelled() {
                return Err(CloudSourceError::QueryCancelled);
            }
            let admission = blocking_cloud_operation(app.clone(), move |registry, vault| {
                admit_cloud_read(registry, vault, provider)
            });
            let AdmittedCloudQuery {
                record,
                read_authority,
                catalog,
                resolver,
            } = open_admitted_cloud_query(admission, || ensure_cloud_catalog(&app, state)).await?;

            let response = match record.config {
                #[cfg(feature = "google-drive-connector")]
                CloudSourceConfig::GoogleDrive { selections } => {
                    let selections = drive_selections(selections)?;
                    let reference = catdb_connector_google_drive::DriveSecretReference::new(
                        record.credential_reference,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let config = catdb_connector_google_drive::DriveSourceConfig::new(
                        record.id, "mdreader", "Document", selections, reference,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?
                    .with_content_scope(catdb_connector_google_drive::DriveContentScope::Markdown);
                    let spec = catdb_connector_google_drive::PinnedDriveSourceSpec::new(
                        config,
                        &catalog.schema_version,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let source = catdb_connector_google_drive::DriveSource::open_pinned(
                        spec,
                        resolver.clone(),
                    )
                    .await
                    .map_err(|error| query_failed(error.to_string()))?;
                    let authority = resolve_cloud_execution_authority(
                        provider,
                        &source,
                        &catalog,
                        &read_authority,
                        requested_pins.and_then(|pins| pins.catalog_revision_id),
                    )?;
                    state
                        .advance_catalog(
                            &authority,
                            requested_pins.and_then(|pins| pins.catalog_revision_id),
                        )
                        .await;
                    validate_requested_catalog_pins(&authority, requested_pins)?;
                    execute_cloud_query(
                        provider,
                        &source,
                        &authority,
                        filters,
                        fields,
                        cancellation.clone(),
                    )
                    .await
                }
                #[cfg(not(feature = "google-drive-connector"))]
                CloudSourceConfig::GoogleDrive { .. } => {
                    Err(CloudSourceError::ConnectorUnavailable)
                }
                #[cfg(feature = "dropbox-connector")]
                CloudSourceConfig::Dropbox => {
                    let reference = catdb_connector_dropbox::DropboxSecretReference::new(
                        record.credential_reference,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let config = catdb_connector_dropbox::DropboxSourceConfig::new(
                        record.id,
                        "mdreader",
                        "Document",
                        "",
                        catdb_connector_dropbox::DropboxAuthorizationMode::FullDropbox,
                        reference,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?
                    .with_content_scope(catdb_connector_dropbox::DropboxContentScope::Markdown);
                    let spec = catdb_connector_dropbox::PinnedDropboxSourceSpec::new(
                        config,
                        &catalog.schema_version,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let source =
                        catdb_connector_dropbox::DropboxSource::open_pinned(spec, resolver.clone())
                            .await
                            .map_err(|error| query_failed(error.to_string()))?;
                    let authority = resolve_cloud_execution_authority(
                        provider,
                        &source,
                        &catalog,
                        &read_authority,
                        requested_pins.and_then(|pins| pins.catalog_revision_id),
                    )?;
                    state
                        .advance_catalog(
                            &authority,
                            requested_pins.and_then(|pins| pins.catalog_revision_id),
                        )
                        .await;
                    validate_requested_catalog_pins(&authority, requested_pins)?;
                    execute_cloud_query(
                        provider,
                        &source,
                        &authority,
                        filters,
                        fields,
                        cancellation.clone(),
                    )
                    .await
                }
                #[cfg(not(feature = "dropbox-connector"))]
                CloudSourceConfig::Dropbox => Err(CloudSourceError::ConnectorUnavailable),
            }?;
            resolver.validate_authority().await?;
            Ok(response)
        }
        .await;
        query.finish()?;
        result
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct AdmittedLibraryQuery {
    members: Vec<crate::library_query::LibraryQueryMember>,
    local_records: Vec<crate::library::LocalFolderSource>,
    clouds: Vec<(CloudSourceRecord, Option<SecretBundle>)>,
    registry: LibraryStore,
    vault: Arc<dyn CredentialVault>,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct IdentityBoundLocalSource {
    root: mdreader_ingest::SourceRoot,
    source: Arc<dyn catdb_connectors::DataSource>,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
impl IdentityBoundLocalSource {
    fn verify_identity(&self) -> Result<(), catdb_connectors::SourceError> {
        self.root
            .verify()
            .map_err(|error| catdb_connectors::SourceError::Unavailable {
                source_id: self.source.id().clone(),
                reason: error.to_string(),
            })
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[async_trait::async_trait]
impl catdb_connectors::DataSource for IdentityBoundLocalSource {
    fn id(&self) -> &catdb_connectors::SourceId {
        self.source.id()
    }

    fn schema(&self) -> &catdb_core::Schema {
        self.source.schema()
    }

    fn capabilities(&self) -> &catdb_connectors::SourceCapabilities {
        self.source.capabilities()
    }

    fn source_spec(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": "mdreader-identity-bound-local",
            "root_identity": &self.root.identity,
            "source": self.source.source_spec(),
        })
    }

    async fn freshness(
        &self,
        context: &catdb_connectors::ReadContext,
    ) -> Result<catdb_connectors::SourceFreshness, catdb_connectors::SourceError> {
        self.verify_identity()?;
        let freshness = self.source.freshness(context).await;
        self.verify_identity()?;
        freshness
    }

    fn compile(
        &self,
        query: &catdb_query::EntityQuery,
        authorized_fields: &[String],
    ) -> Result<catdb_connectors::SourcePlan, catdb_connectors::SourceError> {
        self.verify_identity()?;
        let plan = self.source.compile(query, authorized_fields);
        self.verify_identity()?;
        plan
    }

    async fn execute_page(
        &self,
        plan: &catdb_connectors::SourcePlan,
        cursor: Option<&catdb_connectors::SourceCursor>,
        context: &catdb_connectors::ReadContext,
    ) -> Result<catdb_connectors::SourcePage, catdb_connectors::SourceError> {
        self.verify_identity()?;
        let page = self.source.execute_page(plan, cursor, context).await;
        self.verify_identity()?;
        page
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn bind_identity_bound_local_sources(
    membership_id: uuid::Uuid,
    root: &mdreader_ingest::SourceRoot,
) -> Result<Vec<crate::library_query::LibraryPhysicalSource>, CloudSourceError> {
    use catdb_connectors::{
        DataSource, DirectoryAuthority, FilesystemSource, FrontmatterSource, GitSource,
    };

    let directory =
        root.try_clone_directory_handle()
            .map_err(|_| CloudSourceError::QueryRefused {
                source_id: membership_id.to_string(),
                reason: CloudQueryRefusal::Unavailable,
            })?;
    let authority =
        DirectoryAuthority::from_open_directory(&root.path, directory).map_err(|_| {
            CloudSourceError::QueryRefused {
                source_id: membership_id.to_string(),
                reason: CloudQueryRefusal::Unavailable,
            }
        })?;
    let sources: [Arc<dyn DataSource>; 3] = [
        Arc::new(FilesystemSource::from_directory_authority(&authority)),
        Arc::new(GitSource::open_directory_authority(&authority)),
        Arc::new(FrontmatterSource::from_directory_authority(&authority)),
    ];
    sources
        .into_iter()
        .map(|source| {
            let source: Arc<dyn DataSource> = Arc::new(IdentityBoundLocalSource {
                root: root.clone(),
                source,
            });
            let mapping = mdreader_domain::federated::source_mapping(source.as_ref())
                .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
            Ok(crate::library_query::LibraryPhysicalSource::new(
                membership_id,
                source,
                mapping,
            ))
        })
        .collect()
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn bind_unavailable_local_sources(
    membership_id: uuid::Uuid,
    path: &Path,
) -> Result<Vec<crate::library_query::LibraryPhysicalSource>, CloudSourceError> {
    use catdb_connectors::{DataSource, FilesystemSource, FrontmatterSource, GitSource};

    let reason = "the registered directory identity is not currently available";
    let sources: [Arc<dyn DataSource>; 3] = [
        Arc::new(FilesystemSource::unavailable(path, reason)),
        Arc::new(GitSource::unavailable(path, reason)),
        Arc::new(FrontmatterSource::unavailable(path, reason)),
    ];
    sources
        .into_iter()
        .map(|source| {
            let mapping = mdreader_domain::federated::source_mapping(source.as_ref())
                .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
            Ok(crate::library_query::LibraryPhysicalSource::new(
                membership_id,
                source,
                mapping,
            ))
        })
        .collect()
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn admit_library_query(
    registry: LibraryStore,
    vault: Arc<dyn CredentialVault>,
    requested_source_ids: Option<&[uuid::Uuid]>,
    active_root: Option<&mdreader_ingest::SourceRoot>,
) -> Result<AdmittedLibraryQuery, CloudSourceError> {
    let snapshot = registry
        .read_query_snapshot()
        .map_err(crate::library::map_library_cloud_error)?;
    let requested = requested_source_ids.map(|ids| ids.iter().copied().collect::<HashSet<_>>());
    let available = snapshot
        .local_sources
        .iter()
        .map(|source| source.engine_source_id())
        .chain(
            snapshot
                .cloud_sources
                .iter()
                .map(|source| crate::library_query::membership_id(source.id())),
        )
        .collect::<HashSet<_>>();
    if requested
        .as_ref()
        .is_some_and(|selected| selected.is_empty() || !selected.is_subset(&available))
    {
        return Err(query_invalid(
            "Library source selection contains an unregistered membership",
        ));
    }
    let selected = |source_id: uuid::Uuid| {
        requested
            .as_ref()
            .is_none_or(|requested| requested.contains(&source_id))
    };

    let local_records = snapshot
        .local_sources
        .into_iter()
        .filter(|source| selected(source.engine_source_id()))
        .collect::<Vec<_>>();
    let mut members = Vec::new();
    for local in local_records.iter().cloned() {
        let membership_id = local.engine_source_id();
        let registry_id = local.id.clone();
        let path = local
            .path()
            .map_err(crate::library::map_library_cloud_error)?;
        let (physical, name, active) = match local.root() {
            Ok(root) => {
                let name = root
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Local folder")
                    .to_string();
                (
                    bind_identity_bound_local_sources(membership_id, &root)?,
                    name,
                    active_root == Some(&root),
                )
            }
            Err(crate::library::LibraryError::SourceUnavailable) => (
                bind_unavailable_local_sources(membership_id, &path)?,
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Local folder")
                    .to_string(),
                false,
            ),
            Err(error) => return Err(crate::library::map_library_cloud_error(error)),
        };
        members.push(crate::library_query::LibraryQueryMember {
            membership_id,
            registry_id,
            name,
            kind: crate::library_query::LibraryQueryMemberKind::LocalFolder,
            editable: true,
            active,
            sources: physical,
        });
    }

    let mut clouds = Vec::new();
    for record in snapshot
        .cloud_sources
        .into_iter()
        .filter(|source| selected(crate::library_query::membership_id(source.id())))
    {
        // Foreground Library queries only bind the last locally published
        // metadata. Credential resolution and provider refresh are owned by
        // the background synchronizer.
        clouds.push((record, None));
    }
    if members.is_empty() && clouds.is_empty() {
        return Err(query_invalid("Library has no selected enabled sources"));
    }
    Ok(AdmittedLibraryQuery {
        members,
        local_records,
        clouds,
        registry,
        vault,
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn bind_materialized_library_cloud_source(
    record: CloudSourceRecord,
    store: Arc<crate::cloud_documents::store::CloudDocumentStore>,
) -> Result<crate::library_query::LibraryQueryMember, CloudSourceError> {
    use catdb_connectors::DataSource;

    let provider = record.provider;
    let editable = record.authorization_profile() == CloudAuthorizationProfile::MarkdownReadWriteV2
        && native_markdown_write_acceptance(provider) == NativeMarkdownWriteAcceptance::Accepted;
    let membership_id = crate::library_query::membership_id(record.id());
    let registry_id = record.id().to_string();
    let source_id = catdb_connectors::SourceId::new(record.id.clone());
    let materialized = match provider {
        CloudProvider::GoogleDrive => {
            crate::cloud_documents::materialized::MaterializedMarkdownSource::google_drive(
                store, source_id,
            )
        }
        CloudProvider::Dropbox => {
            crate::cloud_documents::materialized::MaterializedMarkdownSource::dropbox(
                store, source_id,
            )
        }
    };
    let pins = materialized
        .published_version_pins()
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let source: Arc<dyn DataSource> = Arc::new(materialized);
    let mapping = mdreader_domain::library::cloud_document_mapping(
        source.as_ref(),
        domain_provider(provider),
    )
    .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let physical = match pins {
        Some(pins) => crate::library_query::LibraryPhysicalSource::new_as_published(
            membership_id,
            source,
            mapping,
            catdb_connectors::SourceId::new(match provider {
                CloudProvider::GoogleDrive => "google-drive",
                CloudProvider::Dropbox => "dropbox",
            }),
            crate::library_query::PublishedLibraryVersionPins {
                source_schema_version_id: pins.schema_version_id,
                mapping_version_id: pins.mapping_version_id,
                source_version_id: pins.source_version_id,
            },
        ),
        None => crate::library_query::LibraryPhysicalSource::new_as(
            membership_id,
            source,
            mapping,
            catdb_connectors::SourceId::new(match provider {
                CloudProvider::GoogleDrive => "google-drive",
                CloudProvider::Dropbox => "dropbox",
            }),
        ),
    };
    Ok(crate::library_query::LibraryQueryMember {
        membership_id,
        registry_id,
        name: record.label,
        kind: match provider {
            CloudProvider::GoogleDrive => crate::library_query::LibraryQueryMemberKind::GoogleDrive,
            CloudProvider::Dropbox => crate::library_query::LibraryQueryMemberKind::Dropbox,
        },
        editable,
        active: false,
        sources: vec![physical],
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn bind_library_cloud_source(
    record: CloudSourceRecord,
    credential: Option<SecretBundle>,
    vault: Arc<dyn CredentialVault>,
    registry: LibraryStore,
    schema_version: &str,
) -> Result<crate::library_query::LibraryQueryMember, CloudSourceError> {
    use catdb_connectors::DataSource;

    let provider = record.provider;
    let membership_id = crate::library_query::membership_id(record.id());
    let registry_id = record.id().to_string();
    let resolver = credential.map(|credential| {
        Arc::new(CloudCredentialResolver::from_admitted(
            record.clone(),
            credential,
            vault,
            registry,
        ))
    });
    let source: Arc<dyn DataSource> = match record.config {
        #[cfg(feature = "google-drive-connector")]
        CloudSourceConfig::GoogleDrive { selections } => {
            let reference = catdb_connector_google_drive::DriveSecretReference::new(
                record.credential_reference,
            )
            .map_err(|error| query_invalid(error.to_string()))?;
            let config = catdb_connector_google_drive::DriveSourceConfig::new(
                record.id,
                "mdreader",
                "Document",
                drive_selections(selections)?,
                reference,
            )
            .map_err(|error| query_invalid(error.to_string()))?
            .with_content_scope(catdb_connector_google_drive::DriveContentScope::Markdown);
            let spec =
                catdb_connector_google_drive::PinnedDriveSourceSpec::new(config, schema_version)
                    .map_err(|error| query_invalid(error.to_string()))?;
            match resolver {
                Some(resolver) => Arc::new(open_library_drive_source(spec, resolver).await?),
                None => {
                    let model = catdb_connector_google_drive::DriveSource::declared_model(&spec)
                        .map_err(|error| query_invalid(error.to_string()))?;
                    let source_spec = serde_json::to_value(&spec)
                        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
                    crate::library_query::declared_unavailable_source(
                        model,
                        source_spec,
                        "the registered Google Drive credential is not currently available",
                    )
                }
            }
        }
        #[cfg(not(feature = "google-drive-connector"))]
        CloudSourceConfig::GoogleDrive { .. } => {
            return Err(CloudSourceError::ConnectorUnavailable)
        }
        #[cfg(feature = "dropbox-connector")]
        CloudSourceConfig::Dropbox => {
            let reference =
                catdb_connector_dropbox::DropboxSecretReference::new(record.credential_reference)
                    .map_err(|error| query_invalid(error.to_string()))?;
            let config = catdb_connector_dropbox::DropboxSourceConfig::new(
                record.id,
                "mdreader",
                "Document",
                "",
                catdb_connector_dropbox::DropboxAuthorizationMode::FullDropbox,
                reference,
            )
            .map_err(|error| query_invalid(error.to_string()))?
            .with_content_scope(catdb_connector_dropbox::DropboxContentScope::Markdown);
            let spec =
                catdb_connector_dropbox::PinnedDropboxSourceSpec::new(config, schema_version)
                    .map_err(|error| query_invalid(error.to_string()))?;
            match resolver {
                Some(resolver) => Arc::new(
                    catdb_connector_dropbox::DropboxSource::open_pinned(spec, resolver)
                        .await
                        .map_err(|error| query_failed(error.to_string()))?,
                ),
                None => {
                    let model = catdb_connector_dropbox::DropboxSource::declared_model(&spec)
                        .map_err(|error| query_invalid(error.to_string()))?;
                    let source_spec = serde_json::to_value(&spec)
                        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
                    crate::library_query::declared_unavailable_source(
                        model,
                        source_spec,
                        "the registered Dropbox credential is not currently available",
                    )
                }
            }
        }
        #[cfg(not(feature = "dropbox-connector"))]
        CloudSourceConfig::Dropbox => return Err(CloudSourceError::ConnectorUnavailable),
    };
    let domain_provider = domain_provider(provider);
    let mapping =
        mdreader_domain::library::cloud_document_mapping(source.as_ref(), domain_provider)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    Ok(crate::library_query::LibraryQueryMember {
        membership_id,
        registry_id,
        name: record.label,
        kind: match provider {
            CloudProvider::GoogleDrive => crate::library_query::LibraryQueryMemberKind::GoogleDrive,
            CloudProvider::Dropbox => crate::library_query::LibraryQueryMemberKind::Dropbox,
        },
        editable: false,
        active: false,
        sources: vec![crate::library_query::LibraryPhysicalSource::new_as(
            membership_id,
            source,
            mapping,
            catdb_connectors::SourceId::new(match provider {
                CloudProvider::GoogleDrive => "google-drive",
                CloudProvider::Dropbox => "dropbox",
            }),
        )],
    })
}

#[cfg(all(feature = "google-drive-connector", not(feature = "e2e-build")))]
async fn open_library_drive_source(
    spec: catdb_connector_google_drive::PinnedDriveSourceSpec,
    resolver: Arc<CloudCredentialResolver>,
) -> Result<catdb_connector_google_drive::DriveSource, CloudSourceError> {
    catdb_connector_google_drive::DriveSource::open_pinned(spec, resolver)
        .await
        .map_err(|error| query_failed(error.to_string()))
}

#[cfg(all(feature = "google-drive-connector", feature = "e2e-build"))]
async fn open_library_drive_source(
    spec: catdb_connector_google_drive::PinnedDriveSourceSpec,
    resolver: Arc<CloudCredentialResolver>,
) -> Result<catdb_connector_google_drive::DriveSource, CloudSourceError> {
    catdb_connector_google_drive::DriveSource::open_with_resolver_and_transport(
        spec,
        resolver,
        Arc::new(E2eDriveTransport),
    )
    .await
    .map_err(|error| query_failed(error.to_string()))
}

#[cfg(all(feature = "google-drive-connector", feature = "e2e-build"))]
#[derive(Debug)]
struct E2eDriveTransport;

#[cfg(all(feature = "google-drive-connector", feature = "e2e-build"))]
const E2E_DRIVE_BODY: &[u8] =
    b"# Drive fixture note\n\nThis document came through the Google Drive connector.";

#[cfg(all(feature = "google-drive-connector", feature = "e2e-build"))]
#[async_trait::async_trait]
impl catdb_connector_google_drive::DriveTransport for E2eDriveTransport {
    async fn send(
        &self,
        request: catdb_connector_google_drive::DriveHttpRequest,
    ) -> Result<
        catdb_connector_google_drive::DriveHttpResponse,
        catdb_connector_google_drive::DriveTransportError,
    > {
        let path = request.path();
        let body = if path.contains("alt=media") {
            E2E_DRIVE_BODY.to_vec()
        } else if path.starts_with(&format!("/drive/v3/files/{E2E_DRIVE_FILE_ID}?")) {
            serde_json::to_vec(&serde_json::json!({
                "id": E2E_DRIVE_FILE_ID,
                "name": "Drive fixture note.md",
                "mimeType": "text/markdown",
                "modifiedTime": "2026-07-28T12:00:00Z",
                "size": E2E_DRIVE_BODY.len().to_string(),
                "md5Checksum": "11111111111111111111111111111111",
                "parents": [],
                "trashed": false
            }))
            .map_err(|_| catdb_connector_google_drive::DriveTransportError::InvalidResponse)?
        } else {
            return Err(catdb_connector_google_drive::DriveTransportError::InvalidResponse);
        };
        catdb_connector_google_drive::DriveHttpResponse::new(200, BTreeMap::new(), body)
    }
}

/// Query all caller-selected Library memberships through one CatDB execution.
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn map_registered_library_query_error(
    error: crate::library_query::LibraryQueryError,
) -> CloudSourceError {
    match error {
        crate::library_query::LibraryQueryError::InvalidRequest
        | crate::library_query::LibraryQueryError::NoEnabledSources
        | crate::library_query::LibraryQueryError::UnknownSource => {
            query_invalid(error.to_string())
        }
        crate::library_query::LibraryQueryError::InvalidCatalog => {
            CloudSourceError::QueryCatalogInvalid
        }
        crate::library_query::LibraryQueryError::AuthorizationDenied => {
            CloudSourceError::QueryAuthorizationDenied
        }
        crate::library_query::LibraryQueryError::Refused => {
            CloudSourceError::QueryAllSourcesRefused
        }
        crate::library_query::LibraryQueryError::Incomplete { source_ids } => {
            CloudSourceError::QueryIncompleteSources {
                source_ids: source_ids
                    .into_iter()
                    .map(|source_id| source_id.to_string())
                    .collect(),
            }
        }
        crate::library_query::LibraryQueryError::Stale { source_ids } => {
            CloudSourceError::QueryStaleSources {
                source_ids: source_ids
                    .into_iter()
                    .map(|source_id| source_id.to_string())
                    .collect(),
            }
        }
        crate::library_query::LibraryQueryError::SourceFailed { source_id } => {
            CloudSourceError::QueryReadFailed {
                source_id: source_id.to_string(),
            }
        }
        crate::library_query::LibraryQueryError::SourceProtocol { source_id } => {
            CloudSourceError::QuerySourceProtocol {
                source_id: source_id.to_string(),
            }
        }
        crate::library_query::LibraryQueryError::InvalidResponse => {
            CloudSourceError::QueryInvalidResponse
        }
        crate::library_query::LibraryQueryError::ExecutionFailed { sources } => {
            CloudSourceError::QueryExecutionFailed { sources }
        }
        crate::library_query::LibraryQueryError::Cancelled => CloudSourceError::QueryCancelled,
        crate::library_query::LibraryQueryError::Timeout => CloudSourceError::QueryDeadlineExceeded,
        crate::library_query::LibraryQueryError::LimitExceeded => {
            CloudSourceError::QueryLimitExceeded
        }
        crate::library_query::LibraryQueryError::QueryFailed => query_failed(error.to_string()),
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn query_library_documents(
    app: AppHandle,
    request_id: String,
    request: catdb_api_types::EntityQueryRequest,
    state: tauri::State<'_, CloudAccountsState>,
    workspace: tauri::State<'_, crate::commands::WorkspaceState>,
) -> Result<crate::library_query::LibraryEntityQueryResponse, CloudSourceError> {
    #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
    {
        let _ = (app, request_id, request, state, workspace);
        Err(CloudSourceError::ConnectorUnavailable)
    }
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    {
        request
            .validate()
            .map_err(|error| query_invalid(error.to_string()))?;
        let owner = CloudQueryOwner::begin(&state, &request_id)?;
        let cancellation = owner.cancellation();
        let result = async {
            if cancellation.is_cancelled() {
                return Err(CloudSourceError::QueryCancelled);
            }
            if let Some(cursor) = request.cursor.as_ref() {
                let stored = state.take_library_continuation(cursor)?;
                let StoredLibraryContinuation {
                    continuation,
                    local_records,
                    cloud_records,
                    registry,
                    ..
                } = stored;
                let pre_registry = registry.clone();
                let pre_locals = local_records.clone();
                let pre_clouds = cloud_records.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    validate_current_library_memberships(&pre_registry, &pre_locals, &pre_clouds)
                })
                .await
                .map_err(|_| CloudSourceError::PersistFailed)??;
                let page = crate::library_query::continue_registered_library_query(
                    request,
                    continuation,
                    cancellation.clone(),
                )
                .await
                .map_err(map_registered_library_query_error)?;
                let post_registry = registry.clone();
                let post_locals = local_records.clone();
                let post_clouds = cloud_records.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    validate_current_library_memberships(&post_registry, &post_locals, &post_clouds)
                })
                .await
                .map_err(|_| CloudSourceError::PersistFailed)??;
                if let Some(continuation) = page.continuation {
                    state.store_library_continuation(StoredLibraryContinuation {
                        continuation,
                        local_records,
                        cloud_records,
                        registry,
                    })?;
                }
                return Ok(page.response);
            }
            let selected = request.source_ids.clone();
            let active_root = crate::library::active_root(&workspace);
            let admitted = blocking_cloud_operation(app.clone(), move |registry, vault| {
                admit_library_query(
                    registry,
                    Arc::new(vault),
                    selected.as_deref(),
                    active_root.as_ref(),
                )
            })
            .await?;
            let _catalog = ensure_cloud_catalog(&app, &state).await?;
            let materialized_store = state.cloud_sync.coordinator().await?.store();
            let AdmittedLibraryQuery {
                mut members,
                local_records,
                clouds,
                registry,
                vault: _,
            } = admitted;
            let expected_clouds = clouds
                .iter()
                .map(|(record, _)| record.clone())
                .collect::<Vec<_>>();
            let materialized_members = tauri::async_runtime::spawn_blocking(move || {
                clouds
                    .into_iter()
                    .map(|(record, _credential)| {
                        bind_materialized_library_cloud_source(
                            record,
                            Arc::clone(&materialized_store),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .await
            .map_err(|_| CloudSourceError::PersistFailed)??;
            members.extend(materialized_members);
            let page = crate::library_query::begin_registered_library_query(
                request,
                members,
                cancellation.clone(),
            )
            .await
            .map_err(map_registered_library_query_error)?;
            let validation_registry = registry.clone();
            let validation_locals = local_records.clone();
            let validation_clouds = expected_clouds.clone();
            tauri::async_runtime::spawn_blocking(move || {
                validate_current_library_memberships(
                    &validation_registry,
                    &validation_locals,
                    &validation_clouds,
                )
            })
            .await
            .map_err(|_| CloudSourceError::PersistFailed)??;
            if let Some(continuation) = page.continuation {
                state.store_library_continuation(StoredLibraryContinuation {
                    continuation,
                    local_records,
                    cloud_records: expected_clouds,
                    registry,
                })?;
            }
            Ok(page.response)
        }
        .await;
        owner.finish()?;
        result
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn query_cloud_documents(
    app: AppHandle,
    request_id: String,
    provider: CloudProvider,
    filters: Vec<CloudFilterInput>,
    fields: Vec<String>,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudDocumentQueryResponse, CloudSourceError> {
    query_cloud_documents_inner(app, request_id, provider, filters, fields, None, &state).await
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn validate_requested_catalog_pins(
    authority: &CloudExecutionAuthority,
    requested: Option<RequestedCatalogPins>,
) -> Result<(), CloudSourceError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    if requested.workspace_id != authority.workspace_id
        || requested
            .catalog_revision_id
            .is_some_and(|revision| revision.to_string() != authority.pins.catalog_version)
        || requested
            .schema_version_id
            .is_some_and(|version| version.to_string() != authority.pins.schema_version)
    {
        return Err(CloudSourceError::QueryCatalogInvalid);
    }
    Ok(())
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn parse_pin(value: &str) -> Result<uuid::Uuid, CloudSourceError> {
    uuid::Uuid::parse_str(value).map_err(|_| CloudSourceError::QueryCatalogInvalid)
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn shared_entity_response(
    response: CloudDocumentQueryResponse,
) -> Result<catdb_api_types::EntityQueryResponse, CloudSourceError> {
    use catdb_api_types::{
        Completeness, EntityQueryResponse, EntityRow, PinnedVersions, SourceExecution,
        SourceOutcome,
    };
    use catdb_provenance::{FieldObservation, FieldProvenance};

    let source_report = response
        .sources
        .first()
        .ok_or(CloudSourceError::QueryInvalidResponse)?;
    let source_version_id = parse_pin(&response.version_pins.source_version)?;
    let mapping_version_id = parse_pin(&response.version_pins.mapping_version)?;
    let provenance_rule_version_id = parse_pin(&response.version_pins.provenance_rule_version)?;
    let rows = response
        .rows
        .into_iter()
        .map(|row| {
            let fields: BTreeMap<String, serde_json::Value> = row
                .fields
                .as_object()
                .ok_or(CloudSourceError::QueryInvalidResponse)?
                .iter()
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect();
            let provenance = row
                .provenance
                .into_iter()
                .map(|field| {
                    let considered = field
                        .providers
                        .into_iter()
                        .map(|provider| match provider.observation {
                            CloudObservation::ObservedValue { value } => {
                                if fields
                                    .get(&field.field)
                                    .filter(|selected| **selected == value)
                                    .is_none()
                                {
                                    return Err(CloudSourceError::QueryInvalidResponse);
                                }
                                let physical_attribute = provider
                                    .physical_attribute
                                    .ok_or(CloudSourceError::QueryInvalidResponse)?;
                                let observed_at_ms = provider
                                    .observed_at_ms
                                    .ok_or(CloudSourceError::QueryInvalidResponse)?;
                                Ok(FieldObservation::observed(
                                    source_version_id,
                                    provider.record_key,
                                    mapping_version_id,
                                    observed_at_ms,
                                    value,
                                )
                                .with_physical_attribute(physical_attribute))
                            }
                            CloudObservation::ObservedNull => {
                                if !matches!(fields.get(&field.field), Some(value) if value.is_null())
                                {
                                    return Err(CloudSourceError::QueryInvalidResponse);
                                }
                                let physical_attribute = provider
                                    .physical_attribute
                                    .ok_or(CloudSourceError::QueryInvalidResponse)?;
                                let observed_at_ms = provider
                                    .observed_at_ms
                                    .ok_or(CloudSourceError::QueryInvalidResponse)?;
                                Ok(FieldObservation::observed(
                                    source_version_id,
                                    provider.record_key,
                                    mapping_version_id,
                                    observed_at_ms,
                                    serde_json::Value::Null,
                                )
                                .with_physical_attribute(physical_attribute))
                            }
                            CloudObservation::ObservedNothing { .. } => {
                                let physical_attribute = provider
                                    .physical_attribute
                                    .ok_or(CloudSourceError::QueryInvalidResponse)?;
                                let observed_at_ms = provider
                                    .observed_at_ms
                                    .ok_or(CloudSourceError::QueryInvalidResponse)?;
                                Ok(FieldObservation::field_unseen(
                                    source_version_id,
                                    provider.record_key,
                                    mapping_version_id,
                                    observed_at_ms,
                                )
                                .with_physical_attribute(physical_attribute))
                            }
                            CloudObservation::Unobservable { reason } => {
                                Ok(FieldObservation::not_asked(
                                    source_version_id,
                                    provider.record_key,
                                    mapping_version_id,
                                    reason,
                                ))
                            }
                        })
                        .collect::<Result<Vec<_>, CloudSourceError>>()?;
                    let selected = field
                        .selected
                        .map(usize::try_from)
                        .transpose()
                        .map_err(|_| CloudSourceError::QueryInvalidResponse)?;
                    let evidence = FieldProvenance::new(
                        provenance_rule_version_id,
                        field.field.clone(),
                        considered,
                        selected,
                    )
                    .map_err(|_| CloudSourceError::QueryInvalidResponse)?;
                    Ok((field.field, evidence))
                })
                .collect::<Result<BTreeMap<_, _>, CloudSourceError>>()?;
            Ok(EntityRow {
                record_key: row.record_key,
                fields,
                provenance,
            })
        })
        .collect::<Result<Vec<_>, CloudSourceError>>()?;
    let (scanned, returned) = match &source_report.outcome {
        CloudSourceOutcome::Ran {
            scanned, returned, ..
        } => (u64::from(*scanned), u64::from(*returned)),
    };
    Ok(EntityQueryResponse {
        query_id: parse_pin(&response.query_id)?,
        pins: PinnedVersions {
            catalog_resolver_id: CLOUD_CATALOG_RESOLVER_ID,
            catalog_revision_id: parse_pin(&response.version_pins.catalog_version)?,
            schema_version_id: parse_pin(&response.version_pins.schema_version)?,
            physical_schema_version_ids: vec![parse_pin(
                &response.version_pins.source_schema_version,
            )?],
            mapping_version_ids: vec![mapping_version_id],
            source_version_ids: vec![source_version_id],
            policy_version_ids: vec![parse_pin(&response.version_pins.policy_version)?],
            provenance_rule_version_id,
        },
        rows,
        sources: vec![SourceExecution {
            source_id: uuid::Uuid::new_v5(
                &CLOUD_SOURCE_VERSION_NAMESPACE,
                source_report.source_id.as_bytes(),
            ),
            source_version_id,
            outcome: SourceOutcome::Ran,
            retry_after_ms: None,
            freshness_ms: Some(source_report.freshness_ms),
            scanned,
            returned,
        }],
        completeness: Completeness::Complete,
        next_cursor: None,
    })
}

/// Run a cloud Library read through CatDB's shared typed entity-query contract.
#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn entity_query(
    app: AppHandle,
    request_id: String,
    provider: CloudProvider,
    request: catdb_api_types::EntityQueryRequest,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<catdb_api_types::EntityQueryResponse, CloudSourceError> {
    #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
    {
        let _ = (app, request_id, provider, request, state);
        Err(CloudSourceError::ConnectorUnavailable)
    }
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    {
        request
            .validate()
            .map_err(|error| query_invalid(error.to_string()))?;
        if request.entity != "document.Document"
            || request.cursor.is_some()
            || request.source_ids.is_some()
            || request.max_staleness_ms.is_some()
            || !request.order_by.is_empty()
        {
            return Err(query_invalid(
                "cloud Library supports document.Document without cursor, source selection, staleness constraint, or ordering",
            ));
        }
        let requested_pins = RequestedCatalogPins {
            workspace_id: request.workspace_id,
            catalog_revision_id: request.catalog_revision_id,
            schema_version_id: request.schema_version_id,
        };
        let filters = request
            .filters
            .into_iter()
            .map(|filter| CloudFilterInput {
                field: filter.field,
                operator: match filter.operator {
                    catdb_api_types::FilterOperator::Eq => CloudFilterOperator::Eq,
                    catdb_api_types::FilterOperator::Lt => CloudFilterOperator::Lt,
                    catdb_api_types::FilterOperator::Le => CloudFilterOperator::Le,
                    catdb_api_types::FilterOperator::Gt => CloudFilterOperator::Gt,
                    catdb_api_types::FilterOperator::Ge => CloudFilterOperator::Ge,
                    catdb_api_types::FilterOperator::Glob => CloudFilterOperator::Glob,
                },
                value: filter.value,
            })
            .collect();
        let response = query_cloud_documents_inner(
            app,
            request_id,
            provider,
            filters,
            request.select,
            Some(requested_pins),
            &state,
        )
        .await?;
        let answer = shared_entity_response(response)?;
        let maximum = usize::try_from(request.page_size.min(request.limit))
            .map_err(|_| CloudSourceError::QueryLimitExceeded)?;
        if answer.rows.len() > maximum {
            return Err(CloudSourceError::QueryLimitExceeded);
        }
        Ok(answer)
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) fn cancel_cloud_document_query(
    request_id: String,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<bool, CloudSourceError> {
    if request_id.is_empty()
        || request_id.len() > MAX_CLOUD_REQUEST_ID_BYTES
        || request_id.chars().any(char::is_control)
    {
        return Err(CloudSourceError::QueryInvalid {
            detail: "cloud query request ID is invalid".to_string(),
        });
    }
    Ok(state.cancel_query(&request_id))
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn query_invalid(detail: impl Into<String>) -> CloudSourceError {
    CloudSourceError::QueryInvalid {
        detail: bounded_error_detail(detail.into()),
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn query_failed(detail: impl Into<String>) -> CloudSourceError {
    CloudSourceError::QueryFailed {
        detail: bounded_error_detail(detail.into()),
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn map_query_source_error(error: catdb_connectors::SourceError) -> CloudSourceError {
    use catdb_connectors::SourceError;

    let source = |source_id: catdb_connectors::SourceId| source_id.to_string();
    match error {
        SourceError::Cancelled => CloudSourceError::QueryCancelled,
        SourceError::DeadlineExceeded => CloudSourceError::QueryDeadlineExceeded,
        SourceError::StatementTimeout { source_id } => CloudSourceError::QueryTimeout {
            source_id: source(source_id),
        },
        SourceError::Authentication { source_id } => CloudSourceError::QueryAuthentication {
            source_id: source(source_id),
        },
        SourceError::RateLimited {
            source_id,
            retry_after_ms,
        } => CloudSourceError::QueryRateLimited {
            source_id: source(source_id),
            retry_after_ms: retry_after_ms.map(|value| u32::try_from(value).unwrap_or(u32::MAX)),
        },
        SourceError::IncompleteSearch { source_id } => CloudSourceError::QueryIncomplete {
            source_id: source(source_id),
        },
        SourceError::Transport { source_id } => CloudSourceError::QueryTransport {
            source_id: source(source_id),
        },
        SourceError::SourceCancelled { source_id } => CloudSourceError::QuerySourceCancelled {
            source_id: source(source_id),
        },
        SourceError::UnsupportedRowLevelSecurity { source_id } => CloudSourceError::QueryRefused {
            source_id: source(source_id),
            reason: CloudQueryRefusal::UnsupportedPolicy,
        },
        SourceError::InvalidReadContext
        | SourceError::RowBudgetExceeded
        | SourceError::ByteBudgetExceeded
        | SourceError::CursorTooLarge { .. }
        | SourceError::InvalidCursor
        | SourceError::ResourceLimit { .. } => CloudSourceError::QueryLimitExceeded,
        SourceError::Unavailable { source_id, .. } => CloudSourceError::QueryRefused {
            source_id: source(source_id),
            reason: CloudQueryRefusal::Unavailable,
        },
        SourceError::UnknownEntity { source_id, .. } => CloudSourceError::QueryRefused {
            source_id: source(source_id),
            reason: CloudQueryRefusal::UnknownEntity,
        },
        SourceError::UnobservableAttribute { source_id, .. } => CloudSourceError::QueryRefused {
            source_id: source(source_id),
            reason: CloudQueryRefusal::UnobservableAttribute,
        },
        SourceError::UnsupportedFilter { source_id, .. } => CloudSourceError::QueryRefused {
            source_id: source(source_id),
            reason: CloudQueryRefusal::UnsupportedFilter,
        },
        SourceError::UnsupportedOrdering { source_id, .. } => CloudSourceError::QueryRefused {
            source_id: source(source_id),
            reason: CloudQueryRefusal::UnsupportedOrdering,
        },
        SourceError::UnsupportedLimit { source_id } => CloudSourceError::QueryRefused {
            source_id: source(source_id),
            reason: CloudQueryRefusal::UnsupportedLimit,
        },
        SourceError::SchemaDrift {
            source_id,
            expected,
            observed,
        } => CloudSourceError::QuerySchemaDrift {
            source_id: source(source_id),
            expected,
            observed,
        },
        SourceError::UnrepresentablePath { source_id, .. }
        | SourceError::Containment { source_id, .. }
        | SourceError::Read { source_id, .. }
        | SourceError::Malformed { source_id, .. } => CloudSourceError::QueryReadFailed {
            source_id: source(source_id),
        },
        SourceError::WorkerFailed
        | SourceError::InvalidPlan { .. }
        | SourceError::ForeignPlan { .. }
        | SourceError::AttributeConflict { .. }
        | SourceError::EmptyFederation => CloudSourceError::QueryInvalidResponse,
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn bounded_error_detail(detail: String) -> String {
    let detail: String = detail
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(2_048)
        .collect();
    if detail.is_empty() {
        "source operation failed".to_string()
    } else {
        detail
    }
}

#[cfg(feature = "google-drive-connector")]
fn drive_selections(
    selections: Vec<DriveItemSelection>,
) -> Result<Vec<catdb_connector_google_drive::DriveSelection>, CloudSourceError> {
    selections
        .into_iter()
        .map(
            |selection| match (selection.root, selection.folder, selection.resource_key) {
                (true, true, Some(key)) => {
                    catdb_connector_google_drive::DriveSelection::root_with_resource_key(
                        selection.id,
                        key,
                    )
                }
                (true, true, None) => Ok(catdb_connector_google_drive::DriveSelection::root(
                    selection.id,
                )),
                (false, true, Some(key)) => {
                    catdb_connector_google_drive::DriveSelection::folder_with_resource_key(
                        selection.id,
                        key,
                    )
                }
                (false, true, None) => Ok(catdb_connector_google_drive::DriveSelection::folder(
                    selection.id,
                )),
                (false, false, Some(key)) => {
                    catdb_connector_google_drive::DriveSelection::file_with_resource_key(
                        selection.id,
                        key,
                    )
                }
                (false, false, None) => Ok(catdb_connector_google_drive::DriveSelection::file(
                    selection.id,
                )),
                (true, false, _) => Err(
                    catdb_connector_google_drive::DriveConnectorError::InvalidConfiguration {
                        field: "selections",
                    },
                ),
            },
        )
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| query_invalid(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloudCatalogAuthority {
    catalog_path: Option<PathBuf>,
    pub(crate) workspace_id: uuid::Uuid,
    pub(crate) catalog_version: String,
    pub(crate) schema_version: String,
    pub(crate) schema_checksum: String,
}

impl CloudCatalogAuthority {
    fn ready(&self) -> CloudCatalogReady {
        CloudCatalogReady {
            workspace_id: self.workspace_id.to_string(),
            catalog_revision_id: self.catalog_version.clone(),
            schema_version_id: self.schema_version.clone(),
        }
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Debug, Clone)]
struct CloudReadAuthority {
    source_id: String,
    connector_version: String,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
impl CloudReadAuthority {
    fn from_record(record: &CloudSourceRecord) -> Result<Self, CloudSourceError> {
        let contract = record
            .contract
            .as_ref()
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        if !record.enabled
            || !contract.permits_read_only()
            || contract.mapping_version != CLOUD_DOCUMENT_MAPPING_VERSION
        {
            return Err(CloudSourceError::QueryRefused {
                source_id: record.id.clone(),
                reason: CloudQueryRefusal::UnsupportedPolicy,
            });
        }
        Ok(Self {
            source_id: record.id.clone(),
            connector_version: contract.connector_version().to_string(),
        })
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Debug, Clone)]
struct CloudExecutionAuthority {
    mapping: catdb_core::ValidMapping,
    workspace_id: uuid::Uuid,
    pins: CloudVersionPins,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn domain_provider(provider: CloudProvider) -> mdreader_domain::library::CloudDocumentSource {
    match provider {
        CloudProvider::GoogleDrive => mdreader_domain::library::CloudDocumentSource::GoogleDrive,
        CloudProvider::Dropbox => mdreader_domain::library::CloudDocumentSource::Dropbox,
    }
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn provider_field<'a>(
    mapping: &'a catdb_core::ValidMapping,
    document_field: &str,
) -> Option<&'a str> {
    mapping
        .mapping()
        .attr_map
        .iter()
        .find(|(_, target)| target.attr.as_str() == document_field)
        .map(|(physical, _)| physical.as_str())
}

fn cloud_workspace_id(app_identifier: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(&CLOUD_WORKSPACE_NAMESPACE, app_identifier.as_bytes())
}

fn cloud_catalog_timestamp_ms() -> Result<i64, CloudSourceError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
        .as_millis();
    i64::try_from(millis).map_err(|_| CloudSourceError::QueryCatalogInvalid)
}

pub(crate) fn open_cloud_catalog(
    path: &Path,
    workspace_id: uuid::Uuid,
) -> Result<CloudCatalogAuthority, CloudSourceError> {
    use catdb_storage::{CatalogDraft, Compatibility, SchemaContent, SchemaVersion};

    let store = catdb_storage_sqlite::SqliteStore::open(path)
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let active_content = SchemaContent::new(
        serde_json::to_value(mdreader_domain::library::document_schema())
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?,
    );
    let schema_version_id = catdb_engine::schema_catalog::operational_schema_version_id(
        &mdreader_domain::library::document_schema(),
    );
    let active_version = SchemaVersion {
        schema_version_id,
        name: mdreader_domain::library::SCHEMA.to_string(),
        content_id: active_content.content_id,
        compatibility: Compatibility::Backward,
    };
    let head = store
        .latest_catalog_revision_now()
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let revision = match head {
        Some(head) => {
            let current = head
                .schema_version_ids
                .iter()
                .filter_map(|version_id| {
                    store
                        .get_schema_version_now(*version_id)
                        .ok()
                        .flatten()
                        .filter(|version| version.name == mdreader_domain::library::SCHEMA)
                })
                .collect::<Vec<_>>();
            if current.len() == 1 && current[0] == active_version {
                let stored = store
                    .get_schema_content_now(active_content.content_id)
                    .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
                    .ok_or(CloudSourceError::QueryCatalogInvalid)?;
                stored
                    .verify()
                    .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
                if stored != active_content {
                    return Err(CloudSourceError::QueryCatalogInvalid);
                }
                head
            } else {
                let revision_id = uuid::Uuid::new_v5(
                    &CLOUD_CATALOG_REVISION_NAMESPACE,
                    format!("{}:{schema_version_id}", head.revision_id).as_bytes(),
                );
                store
                    .publish_catalog_now(
                        Some(head.revision_id),
                        CatalogDraft {
                            revision_id,
                            actor: "mdreader-cloud-library".to_string(),
                            timestamp: cloud_catalog_timestamp_ms()?,
                            schema_contents: vec![active_content.clone()],
                            schema_versions: vec![active_version.clone()],
                            mapping_versions: Vec::new(),
                            source_versions: Vec::new(),
                            policy_versions: Vec::new(),
                            provenance_rule_versions: Vec::new(),
                        },
                    )
                    .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            }
        }
        None => {
            let revision_id = uuid::Uuid::new_v5(
                &CLOUD_CATALOG_REVISION_NAMESPACE,
                schema_version_id.as_bytes(),
            );
            store
                .publish_catalog_now(
                    None,
                    CatalogDraft {
                        revision_id,
                        actor: "mdreader-cloud-library".to_string(),
                        timestamp: cloud_catalog_timestamp_ms()?,
                        schema_contents: vec![active_content.clone()],
                        schema_versions: vec![active_version],
                        mapping_versions: Vec::new(),
                        source_versions: Vec::new(),
                        policy_versions: Vec::new(),
                        provenance_rule_versions: Vec::new(),
                    },
                )
                .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
        }
    };
    Ok(CloudCatalogAuthority {
        catalog_path: Some(path.to_path_buf()),
        workspace_id,
        catalog_version: revision.revision_id.to_string(),
        schema_version: schema_version_id.to_string(),
        schema_checksum: active_content.checksum,
    })
}

pub(crate) async fn ensure_cloud_catalog(
    app: &AppHandle,
    state: &CloudAccountsState,
) -> Result<CloudCatalogAuthority, CloudSourceError> {
    state.configure_catalog(app)?;
    ensure_configured_cloud_catalog(state).await
}

pub(crate) async fn ensure_configured_cloud_catalog(
    state: &CloudAccountsState,
) -> Result<CloudCatalogAuthority, CloudSourceError> {
    let mut cached = state.catalog.lock().await;
    if let Some(catalog) = cached.as_ref() {
        return Ok(catalog.clone());
    }
    let (path, workspace_id) = state
        .catalog_location
        .lock()
        .map_err(|_| CloudSourceError::PersistFailed)?
        .clone()
        .ok_or(CloudSourceError::QueryCatalogUnavailable)?;
    let catalog = tauri::async_runtime::spawn_blocking(move || {
        let parent = path.parent().ok_or(CloudSourceError::PersistFailed)?;
        std::fs::create_dir_all(parent).map_err(|_| CloudSourceError::PersistFailed)?;
        open_cloud_catalog(&path, workspace_id)
    })
    .await
    .map_err(|_| CloudSourceError::PersistFailed)??;
    *cached = Some(catalog.clone());
    Ok(catalog)
}

/// Establish and verify the cloud Library catalog before renderer reads begin.
#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn prepare_cloud_catalog(
    app: AppHandle,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudCatalogReady, CloudSourceError> {
    Ok(ensure_cloud_catalog(&app, &state).await?.ready())
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn content_version(namespace: &uuid::Uuid, spec: &serde_json::Value) -> (String, String) {
    let checksum = catdb_storage::catalog_content_checksum(spec);
    (
        uuid::Uuid::new_v5(namespace, checksum.as_bytes()).to_string(),
        checksum,
    )
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn cloud_provenance_rule_spec(
    provider: CloudProvider,
    mapping: &catdb_core::Mapping,
    source_version_id: uuid::Uuid,
    mapping_version_id: uuid::Uuid,
) -> Result<serde_json::Value, CloudSourceError> {
    let declared_source = match provider {
        CloudProvider::GoogleDrive => "google-drive",
        CloudProvider::Dropbox => "dropbox",
    };
    let precedence = mdreader_domain::federated::precedence();
    let mut fields = BTreeMap::new();
    for (physical, target) in &mapping.attr_map {
        if !target.path.arrows.is_empty()
            || target.path.start.as_str() != mdreader_domain::federated::ENTITY
        {
            continue;
        }
        let declaration = precedence
            .of(&target.attr)
            .filter(|declaration| {
                declaration.order.iter().any(|candidate| {
                    candidate.source.as_str() == declared_source && candidate.attr == *physical
                })
            })
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        if fields
            .insert(
                target.attr.as_str().to_string(),
                serde_json::json!({
                    "entity": "document.Document",
                    "field": target.attr.as_str(),
                    "rationale": declaration.rationale,
                    "providers": [{
                        "source_version_id": source_version_id,
                        "mapping_version_id": mapping_version_id,
                    }],
                }),
            )
            .is_some()
        {
            return Err(CloudSourceError::QueryCatalogInvalid);
        }
    }
    if fields.is_empty() {
        return Err(CloudSourceError::QueryCatalogInvalid);
    }
    Ok(serde_json::json!({
        "fields": fields.into_values().collect::<Vec<_>>(),
    }))
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct CloudExecutionSpecs {
    mapping: serde_json::Value,
    physical_schema: serde_json::Value,
    source: serde_json::Value,
    policy: serde_json::Value,
    provenance_rule: serde_json::Value,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn persist_cloud_execution_pins(
    path: &Path,
    catalog: &CloudCatalogAuthority,
    source: &dyn catdb_connectors::DataSource,
    specs: CloudExecutionSpecs,
    pins: &CloudVersionPins,
    requested_revision_id: Option<uuid::Uuid>,
) -> Result<String, CloudSourceError> {
    use catdb_storage::{
        CatalogDraft, Compatibility, MappingCapability, MappingVersion, PolicyVersion,
        ProvenanceRuleVersion, SchemaContent, SchemaVersion, SourceVersion,
    };

    let store = catdb_storage_sqlite::SqliteStore::open(path)
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let head = store
        .latest_catalog_revision_now()
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
        .ok_or(CloudSourceError::QueryCatalogInvalid)?;
    let canonical_content = SchemaContent::new(
        serde_json::to_value(mdreader_domain::library::document_schema())
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?,
    );
    if canonical_content.checksum != catalog.schema_checksum {
        return Err(CloudSourceError::QueryCatalogInvalid);
    }
    let canonical_version = SchemaVersion {
        schema_version_id: parse_pin(&catalog.schema_version)?,
        name: mdreader_domain::library::SCHEMA.to_string(),
        content_id: canonical_content.content_id,
        compatibility: Compatibility::Backward,
    };
    let physical_content = SchemaContent::new(specs.physical_schema);
    if physical_content.checksum != pins.source_schema_checksum {
        return Err(CloudSourceError::QueryCatalogInvalid);
    }
    let physical_version = SchemaVersion {
        schema_version_id: parse_pin(&pins.source_schema_version)?,
        name: source.schema().name.to_string(),
        content_id: physical_content.content_id,
        compatibility: Compatibility::Backward,
    };
    let mapping_version = MappingVersion {
        mapping_version_id: parse_pin(&pins.mapping_version)?,
        name: CLOUD_DOCUMENT_MAPPING_VERSION.to_string(),
        source_schema_version_id: physical_version.schema_version_id,
        target_schema_version_id: canonical_version.schema_version_id,
        spec: specs.mapping,
        checksum: pins.mapping_checksum.clone(),
        capability: MappingCapability::ReadOnly,
    };
    let source_version = SourceVersion {
        source_version_id: parse_pin(&pins.source_version)?,
        source_id: uuid::Uuid::new_v5(
            &CLOUD_SOURCE_VERSION_NAMESPACE,
            source.id().as_str().as_bytes(),
        ),
        spec: specs.source,
        checksum: pins.source_checksum.clone(),
    };
    let policy_version = PolicyVersion {
        policy_version_id: parse_pin(&pins.policy_version)?,
        name: CLOUD_READ_POLICY_VERSION.to_string(),
        spec: specs.policy,
        checksum: pins.policy_checksum.clone(),
    };
    let provenance_rule_version = ProvenanceRuleVersion {
        provenance_rule_version_id: parse_pin(&pins.provenance_rule_version)?,
        spec: specs.provenance_rule,
        checksum: pins.provenance_rule_checksum.clone(),
    };
    mapping_version
        .verify()
        .and_then(|()| source_version.verify())
        .and_then(|()| policy_version.verify())
        .and_then(|()| provenance_rule_version.verify())
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;

    let schema_ids = vec![
        canonical_version.schema_version_id,
        physical_version.schema_version_id,
    ];
    let selects_authority = |revision: &catdb_storage::CatalogRevision| {
        revision.schema_version_ids == schema_ids
            && revision.mapping_version_ids == [mapping_version.mapping_version_id]
            && revision.source_version_ids == [source_version.source_version_id]
            && revision.policy_version_ids == [policy_version.policy_version_id]
            && revision.provenance_rule_version_ids
                == [provenance_rule_version.provenance_rule_version_id]
    };
    let verify_authority = |revision: &catdb_storage::CatalogRevision| {
        let stored_canonical = store
            .get_schema_version_now(canonical_version.schema_version_id)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        let stored_physical = store
            .get_schema_version_now(physical_version.schema_version_id)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        let stored_mapping = store
            .get_mapping_version_now(mapping_version.mapping_version_id)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        let stored_source = store
            .get_source_version_now(source_version.source_version_id)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        let stored_policy = store
            .get_policy_version_now(policy_version.policy_version_id)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        let stored_provenance_rule = store
            .get_provenance_rule_version_now(provenance_rule_version.provenance_rule_version_id)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        if stored_canonical != canonical_version
            || stored_physical != physical_version
            || stored_mapping != mapping_version
            || stored_source != source_version
            || stored_policy != policy_version
            || stored_provenance_rule != provenance_rule_version
        {
            return Err(CloudSourceError::QueryCatalogInvalid);
        }
        Ok(revision.revision_id.to_string())
    };
    if let Some(requested_revision_id) = requested_revision_id {
        let requested = store
            .get_catalog_revision_now(requested_revision_id)
            .map_err(|_| CloudSourceError::QueryCatalogInvalid)?
            .ok_or(CloudSourceError::QueryCatalogInvalid)?;
        if selects_authority(&requested) {
            return verify_authority(&requested);
        }
    }
    if selects_authority(&head) {
        return verify_authority(&head);
    }

    let revision_spec = serde_json::json!({
        "parent": head.revision_id,
        "schemas": schema_ids,
        "mapping": mapping_version.mapping_version_id,
        "source": source_version.source_version_id,
        "policy": policy_version.policy_version_id,
        "provenance_rule": provenance_rule_version.provenance_rule_version_id,
    });
    let revision_id = uuid::Uuid::new_v5(
        &CLOUD_CATALOG_REVISION_NAMESPACE,
        catdb_storage::catalog_content_checksum(&revision_spec).as_bytes(),
    );
    let revision = store
        .publish_catalog_now(
            Some(head.revision_id),
            CatalogDraft {
                revision_id,
                actor: "mdreader-cloud-library".to_string(),
                timestamp: cloud_catalog_timestamp_ms()?,
                schema_contents: vec![canonical_content, physical_content],
                schema_versions: vec![canonical_version, physical_version],
                mapping_versions: vec![mapping_version],
                source_versions: vec![source_version],
                policy_versions: vec![policy_version],
                provenance_rule_versions: vec![provenance_rule_version],
            },
        )
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    Ok(revision.revision_id.to_string())
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn resolve_cloud_execution_authority(
    provider: CloudProvider,
    source: &dyn catdb_connectors::DataSource,
    catalog: &CloudCatalogAuthority,
    read: &CloudReadAuthority,
    requested_revision_id: Option<uuid::Uuid>,
) -> Result<CloudExecutionAuthority, CloudSourceError> {
    if source.id().as_str() != read.source_id {
        return Err(CloudSourceError::QueryInvalidResponse);
    }
    let mapping = mdreader_domain::library::validated_cloud_document_mapping(
        source,
        domain_provider(provider),
        &catdb_validation::EggProver::default(),
    )
    .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let mapping_spec = serde_json::to_value(mapping.mapping())
        .map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let (mapping_version, mapping_checksum) =
        content_version(&CLOUD_MAPPING_VERSION_NAMESPACE, &mapping_spec);

    let source_schema_spec =
        serde_json::to_value(source.schema()).map_err(|_| CloudSourceError::QueryCatalogInvalid)?;
    let (source_schema_version, source_schema_checksum) = content_version(
        &CLOUD_PHYSICAL_SCHEMA_VERSION_NAMESPACE,
        &source_schema_spec,
    );
    let source_spec = source.source_spec();
    let (source_version, source_checksum) =
        content_version(&CLOUD_SOURCE_VERSION_NAMESPACE, &source_spec);
    let provenance_rule_spec = cloud_provenance_rule_spec(
        provider,
        mapping.mapping(),
        parse_pin(&source_version)?,
        parse_pin(&mapping_version)?,
    )?;
    let (provenance_rule_version, provenance_rule_checksum) = content_version(
        &CLOUD_PROVENANCE_RULE_VERSION_NAMESPACE,
        &provenance_rule_spec,
    );
    let policy_spec = serde_json::json!({
        "kind": "mdreader-cloud-read",
        "provider": provider,
        "source_id": read.source_id,
        "source_version": source_version,
        "source_schema_version": source_schema_version,
        "mapping_version": mapping_version,
        "connector_version": read.connector_version,
        "capability": "read-only",
    });
    let (policy_version, policy_checksum) =
        content_version(&CLOUD_POLICY_VERSION_NAMESPACE, &policy_spec);

    let mut pins = CloudVersionPins {
        catalog_version: catalog.catalog_version.clone(),
        schema_version: catalog.schema_version.clone(),
        schema_checksum: catalog.schema_checksum.clone(),
        source_schema_version,
        source_schema_checksum,
        mapping_version,
        mapping_checksum,
        connector_version: read.connector_version.clone(),
        source_version,
        source_checksum,
        policy_version,
        policy_checksum,
        provenance_rule_version,
        provenance_rule_checksum,
    };
    if let Some(path) = catalog.catalog_path.as_deref() {
        pins.catalog_version = persist_cloud_execution_pins(
            path,
            catalog,
            source,
            CloudExecutionSpecs {
                mapping: mapping_spec,
                physical_schema: source_schema_spec,
                source: source_spec,
                policy: policy_spec,
                provenance_rule: provenance_rule_spec,
            },
            &pins,
            requested_revision_id,
        )?;
    }
    Ok(CloudExecutionAuthority {
        mapping,
        workspace_id: catalog.workspace_id,
        pins,
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn execute_cloud_query(
    provider: CloudProvider,
    source: &dyn catdb_connectors::DataSource,
    authority: &CloudExecutionAuthority,
    filters: Vec<CloudFilterInput>,
    fields: Vec<String>,
    cancellation: catdb_connectors::CancellationToken,
) -> Result<CloudDocumentQueryResponse, CloudSourceError> {
    if filters.len() > MAX_CLOUD_QUERY_FILTERS
        || fields.is_empty()
        || fields.len() > MAX_CLOUD_QUERY_FIELDS
    {
        return Err(query_invalid("cloud query bounds are invalid"));
    }
    let point_record_key = match filters.as_slice() {
        [filter]
            if filter.field == "path" && matches!(filter.operator, CloudFilterOperator::Eq) =>
        {
            Some(filter.value.clone())
        }
        _ => None,
    };
    let valid_text = |value: &str| {
        !value.is_empty()
            && value.len() <= 4_096
            && !value.chars().any(|character| character.is_control())
    };
    if filters
        .iter()
        .any(|filter| !valid_text(&filter.field) || !valid_text(&filter.value))
        || fields.iter().any(|field| !valid_text(field))
    {
        return Err(query_invalid(
            "cloud query contains an invalid field or value",
        ));
    }
    let unique_fields: HashSet<&str> = fields.iter().map(String::as_str).collect();
    if unique_fields.len() != fields.len() {
        return Err(query_invalid("cloud query projects a field more than once"));
    }
    let document = mdreader_domain::library::document_schema();
    if fields
        .iter()
        .any(|field| !document.attributes.contains_key(&field.as_str().into()))
    {
        return Err(query_invalid(
            "cloud query projects a field outside the pinned Document schema",
        ));
    }

    let entities = source
        .schema()
        .entities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let [entity] = entities.as_slice() else {
        return Err(query_failed(
            "cloud connector does not expose exactly one Document entity",
        ));
    };
    let target = format!("{}.{}", source.schema().name, entity);
    let vocabulary = catdb_query::EntityVocabulary::derive(&[source.schema().clone()]);
    let filter_inputs = filters
        .into_iter()
        .map(|filter| {
            let field = provider_field(&authority.mapping, &filter.field).ok_or_else(|| {
                query_invalid(format!(
                    "cloud Document field `{}` cannot be filtered by this provider",
                    filter.field
                ))
            })?;
            Ok(catdb_query::EntityFilterInput {
                field: field.to_string(),
                op: match filter.operator {
                    CloudFilterOperator::Eq => catdb_query::AttrOp::Eq,
                    CloudFilterOperator::Lt => catdb_query::AttrOp::Lt,
                    CloudFilterOperator::Le => catdb_query::AttrOp::Le,
                    CloudFilterOperator::Gt => catdb_query::AttrOp::Gt,
                    CloudFilterOperator::Ge => catdb_query::AttrOp::Ge,
                    CloudFilterOperator::Glob => catdb_query::AttrOp::Glob,
                },
                value: filter.value,
            })
        })
        .collect::<Result<Vec<_>, CloudSourceError>>()?;
    let mut provider_fields = fields
        .iter()
        .filter_map(|field| provider_field(&authority.mapping, field))
        .map(str::to_string)
        .collect::<Vec<_>>();
    provider_fields.sort();
    provider_fields.dedup();
    if provider_fields.is_empty() {
        let identity = provider_field(&authority.mapping, "path")
            .ok_or_else(|| query_failed("cloud mapping has no provider identity"))?;
        provider_fields.push(identity.to_string());
    }
    let query = catdb_query::validate_entity_request(
        catdb_query::EntityTarget::Entity(target.clone()),
        &filter_inputs,
        &vocabulary,
    )
    .map_err(|error| query_invalid(error.to_string()))?;
    let query_id = uuid::Uuid::new_v4();
    let context = catdb_connectors::ReadContext::new(
        query_id,
        Instant::now()
            .checked_add(CLOUD_QUERY_DEADLINE)
            .ok_or_else(|| query_failed("cloud query deadline overflow"))?,
        CLOUD_QUERY_WORK_BUDGET,
        CLOUD_QUERY_BYTE_BUDGET,
        CLOUD_QUERY_PAGE_SIZE,
        cancellation,
    )
    .map_err(|error| query_failed(error.to_string()))?;
    let freshness = source
        .freshness(&context)
        .await
        .map_err(map_query_source_error)?;
    let plan = source
        .compile(&query, &provider_fields)
        .map_err(map_query_source_error)?;
    let pushed = plan
        .pushed_attrs()
        .into_iter()
        .map(|field| {
            fields
                .iter()
                .find(|document_field| {
                    provider_field(&authority.mapping, document_field) == Some(field)
                })
                .cloned()
                .unwrap_or_else(|| field.to_string())
        })
        .collect::<Vec<_>>();
    let residual = plan
        .residual_attrs()
        .into_iter()
        .map(|field| {
            fields
                .iter()
                .find(|document_field| {
                    provider_field(&authority.mapping, document_field) == Some(field)
                })
                .cloned()
                .unwrap_or_else(|| field.to_string())
        })
        .collect::<Vec<_>>();
    let answer = catdb_connectors::read(source, &query, &provider_fields, &context)
        .await
        .map_err(map_query_source_error)?;
    if answer.source != *source.id() {
        return Err(query_failed(
            "cloud connector returned an answer under a different source identity",
        ));
    }
    let scanned = u32::try_from(answer.scanned)
        .map_err(|_| query_failed("cloud query scan count exceeds the application contract"))?;
    let source_id = answer.source.to_string();
    let rows = answer
        .rows
        .into_iter()
        .map(|row| {
            if row.source != answer.source || row.entity != target || !row.value.is_object() {
                return Err(query_failed(
                    "cloud connector returned a row with an invalid source, entity, or value",
                ));
            }
            let row_source = row.source.to_string();
            let raw_fields = row
                .value
                .as_object()
                .ok_or_else(|| query_failed("cloud connector returned a non-object row"))?;
            let mut document_fields = serde_json::Map::new();
            let provenance = fields
                .iter()
                .map(|field| {
                    let physical_attribute =
                        provider_field(&authority.mapping, field).map(str::to_string);
                    let observed = physical_attribute
                        .as_ref()
                        .and_then(|attribute| raw_fields.get(attribute))
                        .cloned();
                    if let Some(value) = observed.as_ref() {
                        document_fields.insert(field.clone(), value.clone());
                    }
                    let (observation, selected, observed_at_ms) =
                        match (physical_attribute.as_deref(), observed.as_ref()) {
                            (Some(_), Some(value)) if value.is_null() => (
                                CloudObservation::ObservedNull,
                                Some(0),
                                Some(freshness.observed_at_ms),
                            ),
                            (Some(_), Some(value)) => (
                                CloudObservation::ObservedValue {
                                    value: value.clone(),
                                },
                                Some(0),
                                Some(freshness.observed_at_ms),
                            ),
                            (Some(attribute), None) => (
                                CloudObservation::ObservedNothing {
                                    reason: format!(
                                    "source record did not contain mapped attribute `{attribute}`"
                                ),
                                },
                                None,
                                Some(freshness.observed_at_ms),
                            ),
                            (None, _) => (
                                CloudObservation::Unobservable {
                                    reason: format!(
                                    "pinned mapping does not expose `Document.{field}` for `{}`",
                                    provider.id()
                                ),
                                },
                                None,
                                None,
                            ),
                        };
                    Ok(CloudFieldProvenance {
                        field: field.clone(),
                        selected,
                        providers: vec![CloudConsideredProvider {
                            provider,
                            source_id: row_source.clone(),
                            record_key: row.key.clone(),
                            mapping_version: authority.pins.mapping_version.clone(),
                            physical_attribute,
                            observation,
                            observed_at_ms,
                        }],
                    })
                })
                .collect::<Result<Vec<_>, CloudSourceError>>()?;
            Ok(CloudDocumentRow {
                source_id: row_source,
                record_key: row.key,
                fields: serde_json::Value::Object(document_fields),
                provenance,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let returned = u32::try_from(rows.len())
        .map_err(|_| query_failed("cloud query row count exceeds the application contract"))?;
    if rows.is_empty() {
        if let Some(record_key) = point_record_key {
            return Err(CloudSourceError::QueryNotFound {
                source_id,
                record_key,
            });
        }
    }
    let response = CloudDocumentQueryResponse {
        query_id: query_id.to_string(),
        provider,
        workspace_id: authority.workspace_id.to_string(),
        schema_version: authority.pins.schema_version.clone(),
        version_pins: authority.pins.clone(),
        completeness: CloudCompleteness::Complete,
        sources: vec![CloudSourceReport {
            source_id,
            freshness_ms: freshness.freshness_ms,
            outcome: CloudSourceOutcome::Ran {
                mode: CloudExecutionMode::Virtual,
                scanned,
                returned,
                pushed,
                residual,
            },
        }],
        scanned,
        rows,
    };
    let encoded = serde_json::to_vec(&response)
        .map_err(|_| query_failed("cloud query response is invalid"))?;
    if encoded.len() > CLOUD_QUERY_RESPONSE_BYTE_BUDGET {
        return Err(query_failed("cloud query response byte budget exceeded"));
    }
    Ok(response)
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn list_cloud_sources(
    app: AppHandle,
    _state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    blocking_cloud_operation(app, |registry, vault| {
        CloudAccountService::new(registry, &vault).snapshot()
    })
    .await
}

enum CloudSyncLifecycleAction {
    Activate,
    Replace {
        retired_source_id: Option<catdb_connectors::SourceId>,
        activated_source_id: catdb_connectors::SourceId,
    },
    Pause(catdb_connectors::SourceId),
    Retire(catdb_connectors::SourceId),
}

fn snapshot_source_id(
    snapshot: &CloudSourcesSnapshot,
    provider: CloudProvider,
) -> Option<catdb_connectors::SourceId> {
    snapshot
        .sources
        .iter()
        .find(|source| source.provider == provider && !source.id.is_empty())
        .map(|source| catdb_connectors::SourceId::new(source.id.clone()))
}

/// Applies an already-committed account lifecycle transition to the local
/// synchronizer. Its result is intentionally non-authoritative: a scheduling
/// failure cannot report a committed OAuth or account publication as rolled
/// back.
async fn reconcile_cloud_sync_after_commit(
    app: AppHandle,
    provider: CloudProvider,
    state: &CloudAccountsState,
    action: CloudSyncLifecycleAction,
) -> Result<(), CloudSourceError> {
    #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
    {
        let _ = (app, provider, state, action);
        return Ok(());
    }
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    match action {
        CloudSyncLifecycleAction::Activate => {
            let admitted = open_admitted_cloud_markdown_sync_source(app, provider, state).await?;
            let source_id = admitted.source.source_id().clone();
            state.cloud_sync.cancel_background_refresh(&source_id)?;
            let coordinator = state.cloud_sync.coordinator().await?;
            coordinator
                .activate_with_pins(admitted.source, admitted.pins)
                .await
                .map_err(|_| CloudSourceError::PersistFailed)?;
            state
                .cloud_sync
                .schedule_background_refresh(coordinator, source_id)
        }
        CloudSyncLifecycleAction::Replace {
            retired_source_id,
            activated_source_id,
        } => {
            if let Some(retired_source_id) = retired_source_id {
                reconcile_inactive_cloud_sync_after_commit(
                    state,
                    CloudSyncLifecycleAction::Retire(retired_source_id),
                )
                .await?;
            }
            let admitted = open_admitted_cloud_markdown_sync_source(app, provider, state).await?;
            let source_id = admitted.source.source_id().clone();
            if source_id != activated_source_id {
                return Err(CloudSourceError::PersistFailed);
            }
            state.cloud_sync.cancel_background_refresh(&source_id)?;
            let coordinator = state.cloud_sync.coordinator().await?;
            coordinator
                .activate_with_pins(admitted.source, admitted.pins)
                .await
                .map_err(|_| CloudSourceError::PersistFailed)?;
            state
                .cloud_sync
                .schedule_background_refresh(coordinator, source_id)
        }
        action @ (CloudSyncLifecycleAction::Pause(_) | CloudSyncLifecycleAction::Retire(_)) => {
            reconcile_inactive_cloud_sync_after_commit(state, action).await
        }
    }
}

/// Applies the non-activating half of a committed account transition. Keeping
/// this separate makes the post-commit pause/retire ordering explicit and
/// testable without constructing provider credentials.
async fn reconcile_inactive_cloud_sync_after_commit(
    state: &CloudAccountsState,
    action: CloudSyncLifecycleAction,
) -> Result<(), CloudSourceError> {
    let source_id = match &action {
        CloudSyncLifecycleAction::Pause(source_id) => source_id.clone(),
        CloudSyncLifecycleAction::Retire(source_id) => source_id.clone(),
        CloudSyncLifecycleAction::Activate | CloudSyncLifecycleAction::Replace { .. } => {
            return Err(CloudSourceError::PersistFailed)
        }
    };
    state.cloud_sync.cancel_background_refresh(&source_id)?;
    let result = match action {
        CloudSyncLifecycleAction::Pause(_) => {
            let Some(coordinator) = state.cloud_sync.existing_coordinator().await else {
                return Ok(());
            };
            coordinator.pause(&source_id).await
        }
        CloudSyncLifecycleAction::Retire(_) => {
            state
                .cloud_sync
                .coordinator()
                .await?
                .retire(&source_id)
                .await
        }
        CloudSyncLifecycleAction::Activate | CloudSyncLifecycleAction::Replace { .. } => {
            return Err(CloudSourceError::PersistFailed)
        }
    };
    match result {
        Ok(()) | Err(crate::cloud_documents::CloudSyncError::UnknownSource) => Ok(()),
        Err(_) => Err(CloudSourceError::PersistFailed),
    }
}

/// Schedules startup repair after window creation. All store opening, vault
/// admission, and provider work remain on the background runtime.
pub(crate) fn schedule_cloud_sync_startup(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let startup_app = app.clone();
        let state = app.state::<CloudAccountsState>();
        if let Err(error) = startup_cloud_sync(startup_app, &state).await {
            let kind = serde_json::to_value(error)
                .ok()
                .and_then(|value| {
                    value
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "unknown".to_owned());
            eprintln!("MDReader cloud startup failed: {kind}");
        }
    });
}

async fn startup_cloud_sync(
    app: AppHandle,
    state: &CloudAccountsState,
) -> Result<(), CloudSourceError> {
    #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
    {
        let _ = (app, state);
        return Ok(());
    }
    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    {
        let sources = blocking_cloud_operation(app.clone(), |registry, _vault| {
            registry.read_cloud(|sources| {
                Ok(sources
                    .iter()
                    .filter(|source| source.enabled)
                    .map(|source| {
                        (
                            source.provider,
                            catdb_connectors::SourceId::new(source.id.clone()),
                        )
                    })
                    .collect::<Vec<_>>())
            })
        })
        .await?;
        let coordinator = state.cloud_sync.coordinator().await?;
        for (provider, expected_source_id) in sources {
            let admitted = match open_admitted_cloud_markdown_sync_source(
                app.clone(),
                provider,
                state,
            )
            .await
            {
                Ok(admitted) => admitted,
                Err(error) => {
                    let kind = serde_json::to_value(error)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("kind")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_else(|| "unknown".to_owned());
                    eprintln!("MDReader cloud source admission failed for {provider:?}: {kind}");
                    continue;
                }
            };
            let source_id = admitted.source.source_id().clone();
            if source_id != expected_source_id {
                continue;
            }
            let repair_executor = crate::cloud_documents::service::CloudDocumentLocalPublisher::new(
                coordinator.store(),
                admitted.pins.clone(),
            );
            coordinator
                .startup_repair([source_id.clone()], &repair_executor)
                .await
                .map_err(|_| CloudSourceError::PersistFailed)?;
            state.cloud_sync.cancel_background_refresh(&source_id)?;
            if coordinator
                .activate_with_pins(admitted.source, admitted.pins)
                .await
                .is_ok()
            {
                state
                    .cloud_sync
                    .schedule_background_refresh(coordinator.clone(), source_id)?;
            }
        }
        Ok(())
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn set_cloud_source_enabled(
    app: AppHandle,
    provider: CloudProvider,
    enabled: bool,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    state.cancel_provider_activity(provider)?;
    let owner = CloudConnectOwner::begin(&state, provider)?;
    let commit_app = app.clone();
    let post_app = app.clone();
    let post_state: &CloudAccountsState = &state;
    commit_cloud_connect_then(
        &state,
        owner,
        move || {
            cloud_operation(commit_app, move |registry, vault| {
                let snapshot =
                    CloudAccountService::new(registry, &vault).set_enabled(provider, enabled)?;
                let action = if enabled {
                    Some(CloudSyncLifecycleAction::Activate)
                } else {
                    snapshot_source_id(&snapshot, provider).map(CloudSyncLifecycleAction::Pause)
                };
                Ok((snapshot, action))
            })
        },
        move |action| async move {
            if let Some(action) = action {
                let _ =
                    reconcile_cloud_sync_after_commit(post_app, provider, post_state, action).await;
            }
        },
    )
    .await
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn disconnect_cloud_source(
    app: AppHandle,
    provider: CloudProvider,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    state.cancel_provider_activity(provider)?;
    let owner = CloudConnectOwner::begin(&state, provider)?;
    let commit_app = app.clone();
    let post_app = app.clone();
    let post_state: &CloudAccountsState = &state;
    commit_cloud_connect_then(
        &state,
        owner,
        move || {
            cloud_operation(commit_app, move |registry, vault| {
                let publication =
                    CloudAccountService::new(registry, &vault).disconnect_with_id(provider)?;
                let action = publication
                    .retired_source_id
                    .clone()
                    .map(CloudSyncLifecycleAction::Retire);
                Ok((publication.snapshot, action))
            })
        },
        move |action| async move {
            if let Some(action) = action {
                let _ =
                    reconcile_cloud_sync_after_commit(post_app, provider, post_state, action).await;
            }
        },
    )
    .await
}

async fn commit_cloud_connect<T, Install>(
    state: &CloudAccountsState,
    owner: CloudConnectOwner<'_>,
    install: Install,
) -> Result<T, CloudSourceError>
where
    T: Send + 'static,
    Install: FnOnce() -> Result<T, CloudSourceError> + Send + 'static,
{
    commit_cloud_connect_observed(state, owner, || {}, install).await
}

/// Publishes one account lifecycle mutation, then performs its non-authoritative
/// local sync effect. `install` derives the effect under the same commit lease
/// as the registry mutation, so disconnect cleanup cannot target a stale
/// source identity.
async fn commit_cloud_connect_then<T, Action, Install, PostCommit, PostFuture>(
    state: &CloudAccountsState,
    owner: CloudConnectOwner<'_>,
    install: Install,
    post_commit: PostCommit,
) -> Result<T, CloudSourceError>
where
    T: Send + 'static,
    Action: Send + 'static,
    Install: FnOnce() -> Result<(T, Action), CloudSourceError> + Send + 'static,
    PostCommit: FnOnce(Action) -> PostFuture,
    PostFuture: std::future::Future<Output = ()>,
{
    let (result, action) = commit_cloud_connect(state, owner, install).await?;
    post_commit(action).await;
    Ok(result)
}

async fn commit_cloud_connect_observed<T, Observe, Observation, Install>(
    state: &CloudAccountsState,
    owner: CloudConnectOwner<'_>,
    observe_worker: Observe,
    install: Install,
) -> Result<T, CloudSourceError>
where
    T: Send + 'static,
    Observe: FnOnce() -> Observation + Send + 'static,
    Observation: Send + 'static,
    Install: FnOnce() -> Result<T, CloudSourceError> + Send + 'static,
{
    let lifecycle_commits = Arc::clone(&state.lifecycle_commits);
    let authority = owner.commit_authority();
    let worker_authority = authority.clone();
    let installed = tauri::async_runtime::spawn_blocking(move || {
        let _observation = observe_worker();
        let _commit = lifecycle_commits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        worker_authority.begin_commit()?;
        let result = install();
        worker_authority.finish_commit();
        result
    })
    .await
    .map_err(|_| CloudSourceError::PersistFailed)??;
    owner.finish_committed();
    Ok(installed)
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn connect_cloud_source(
    app: AppHandle,
    provider: CloudProvider,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    let owner = CloudConnectOwner::begin(&state, provider)?;
    let requested_profile = CloudAuthorizationProfile::MarkdownReadWriteV2;
    let cancellation = owner.cancellation();
    let oauth_client = oauth_client_configuration(provider)?;
    let callback_configuration = oauth_callback_configuration(
        provider,
        match provider {
            CloudProvider::GoogleDrive => None,
            CloudProvider::Dropbox => option_env!("MDREADER_DROPBOX_REDIRECT_URI"),
        },
    )?;
    let (listener, redirect_uri) = bind_oauth_callback_listener(callback_configuration).await?;
    let (verifier, oauth_state) = new_pkce_material();
    let attempt = OAuthAttempt::with_profile(
        provider,
        requested_profile,
        verifier,
        oauth_state,
        redirect_uri.clone(),
    )?;
    let authorization_url = attempt.authorization_url(oauth_client.client_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        tauri_plugin_opener::open_url(authorization_url, None::<&str>)
            .map_err(|_| CloudSourceError::OauthTransport)
    })
    .await
    .map_err(|_| CloudSourceError::OauthTransport)??;

    let callback = tokio::select! {
        _ = cancellation.cancelled() => return Err(CloudSourceError::OauthCancelled),
        callback = receive_oauth_callback(listener, &redirect_uri) => callback?,
    };
    let authorization = attempt.verify_callback(&callback)?;
    let token = tokio::select! {
        _ = cancellation.cancelled() => return Err(CloudSourceError::OauthCancelled),
        token = exchange_authorization_code(provider, &oauth_client, &attempt, &authorization) => token?,
    };
    let config = tokio::select! {
        _ = cancellation.cancelled() => return Err(CloudSourceError::OauthCancelled),
        config = cloud_source_config(provider, &token, &authorization) => config?,
    };
    let secret = SecretBundle::new(
        token.access_token.as_str().to_owned(),
        token.refresh_token.as_str().to_owned(),
        expires_at_unix_seconds(token.expires_in_seconds)?,
        new_cursor_key(),
    )?;
    let commit_app = app.clone();
    let post_app = app.clone();
    let post_state: &CloudAccountsState = &state;
    commit_cloud_connect_then(
        &state,
        owner,
        move || {
            cloud_operation(commit_app, move |registry, vault| {
                let publication = CloudAccountService::new(registry, &vault)
                    .install_with_profile_publication(
                        provider,
                        config,
                        secret,
                        requested_profile,
                    )?;
                Ok((
                    publication.snapshot,
                    CloudSyncLifecycleAction::Replace {
                        retired_source_id: publication.retired_source_id,
                        activated_source_id: publication.activated_source_id,
                    },
                ))
            })
        },
        move |action| async move {
            let _ = reconcile_cloud_sync_after_commit(post_app, provider, post_state, action).await;
        },
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OAuthClientConfiguration<'a> {
    client_id: &'a str,
    client_secret: Option<&'a str>,
}

fn oauth_client_configuration(
    provider: CloudProvider,
) -> Result<OAuthClientConfiguration<'static>, CloudSourceError> {
    let client_id = match provider {
        CloudProvider::GoogleDrive => option_env!("MDREADER_GOOGLE_DRIVE_CLIENT_ID"),
        CloudProvider::Dropbox => option_env!("MDREADER_DROPBOX_CLIENT_ID"),
    };
    oauth_client_configuration_from(
        provider,
        client_id,
        option_env!("MDREADER_GOOGLE_DRIVE_CLIENT_SECRET"),
    )
}

fn oauth_client_configuration_from<'a>(
    provider: CloudProvider,
    client_id: Option<&'a str>,
    google_client_secret: Option<&'a str>,
) -> Result<OAuthClientConfiguration<'a>, CloudSourceError> {
    let client_id = client_id.ok_or(CloudSourceError::ClientConfigurationMissing)?;
    validate_oauth_client_id(provider, client_id)?;
    let client_secret = match provider {
        CloudProvider::GoogleDrive => {
            let client_secret =
                google_client_secret.ok_or(CloudSourceError::ClientConfigurationMissing)?;
            validate_google_oauth_client_secret(client_secret)?;
            Some(client_secret)
        }
        CloudProvider::Dropbox => None,
    };
    Ok(OAuthClientConfiguration {
        client_id,
        client_secret,
    })
}

fn validate_google_oauth_client_secret(client_secret: &str) -> Result<(), CloudSourceError> {
    let valid = client_secret.strip_prefix("GOCSPX-").is_some_and(|suffix| {
        !suffix.is_empty()
            && client_secret.len() <= 512
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });
    if !valid {
        return Err(CloudSourceError::ClientConfigurationMissing);
    }
    Ok(())
}

fn validate_oauth_client_id(
    provider: CloudProvider,
    client_id: &str,
) -> Result<(), CloudSourceError> {
    let valid = match provider {
        CloudProvider::GoogleDrive => {
            let Some((project_number, provider_name)) = client_id.split_once('-') else {
                return Err(CloudSourceError::ClientConfigurationMissing);
            };
            let Some(provider_name) = provider_name.strip_suffix(".apps.googleusercontent.com")
            else {
                return Err(CloudSourceError::ClientConfigurationMissing);
            };
            client_id.len() <= 512
                && !project_number.is_empty()
                && project_number.bytes().all(|byte| byte.is_ascii_digit())
                && !provider_name.is_empty()
                && provider_name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }
        CloudProvider::Dropbox => {
            (8..=128).contains(&client_id.len())
                && client_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }
    };
    if !valid {
        return Err(CloudSourceError::ClientConfigurationMissing);
    }
    Ok(())
}

fn expires_at_unix_seconds(expires_in_seconds: u64) -> Result<u64, CloudSourceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CloudSourceError::InvalidConfiguration)?
        .as_secs()
        .checked_add(expires_in_seconds)
        .ok_or(CloudSourceError::InvalidConfiguration)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DropboxRedirectConfiguration {
    redirect_uri: String,
    socket_addr: SocketAddr,
}

impl DropboxRedirectConfiguration {
    fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    fn socket_addr(&self) -> SocketAddr {
        self.socket_addr
    }
}

fn dropbox_redirect_configuration(
    configured: Option<&str>,
) -> Result<DropboxRedirectConfiguration, CloudSourceError> {
    let configured = configured
        .filter(|value| !value.trim().is_empty())
        .ok_or(CloudSourceError::ClientConfigurationMissing)?;
    let redirect = Url::parse(configured).map_err(|_| CloudSourceError::InvalidConfiguration)?;
    if redirect.scheme() != "http"
        || !redirect.username().is_empty()
        || redirect.password().is_some()
        || redirect.query().is_some()
        || redirect.fragment().is_some()
        || redirect.path() != "/oauth/dropbox"
    {
        return Err(CloudSourceError::InvalidConfiguration);
    }
    let authority = configured
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix("/oauth/dropbox"))
        .ok_or(CloudSourceError::InvalidConfiguration)?;
    let port_text = authority
        .strip_prefix("127.0.0.1:")
        .ok_or(CloudSourceError::InvalidConfiguration)?;
    let port = port_text
        .parse::<u16>()
        .map_err(|_| CloudSourceError::InvalidConfiguration)?;
    if port < 1_024 || port_text != port.to_string() {
        return Err(CloudSourceError::InvalidConfiguration);
    }
    let socket_addr = SocketAddr::from(([127, 0, 0, 1], port));
    Ok(DropboxRedirectConfiguration {
        redirect_uri: configured.to_string(),
        socket_addr,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OAuthCallbackConfiguration {
    GoogleDrive,
    Dropbox(DropboxRedirectConfiguration),
}

fn oauth_callback_configuration(
    provider: CloudProvider,
    dropbox_redirect_uri: Option<&str>,
) -> Result<OAuthCallbackConfiguration, CloudSourceError> {
    match provider {
        CloudProvider::GoogleDrive => Ok(OAuthCallbackConfiguration::GoogleDrive),
        CloudProvider::Dropbox => dropbox_redirect_configuration(dropbox_redirect_uri)
            .map(OAuthCallbackConfiguration::Dropbox),
    }
}

async fn bind_oauth_callback_listener(
    configuration: OAuthCallbackConfiguration,
) -> Result<(TcpListener, String), CloudSourceError> {
    match configuration {
        OAuthCallbackConfiguration::GoogleDrive => {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|_| CloudSourceError::OauthCallbackUnavailable)?;
            let port = listener
                .local_addr()
                .map_err(|_| CloudSourceError::OauthCallbackUnavailable)?
                .port();
            Ok((
                listener,
                format!("http://127.0.0.1:{port}/oauth/google-drive"),
            ))
        }
        OAuthCallbackConfiguration::Dropbox(configuration) => {
            let listener = TcpListener::bind(configuration.socket_addr())
                .await
                .map_err(|_| CloudSourceError::OauthCallbackUnavailable)?;
            Ok((listener, configuration.redirect_uri().to_string()))
        }
    }
}

fn new_pkce_material() -> (String, String) {
    let verifier = [
        uuid::Uuid::new_v4().simple().to_string(),
        uuid::Uuid::new_v4().simple().to_string(),
        uuid::Uuid::new_v4().simple().to_string(),
    ]
    .concat();
    let state = [
        uuid::Uuid::new_v4().simple().to_string(),
        uuid::Uuid::new_v4().simple().to_string(),
    ]
    .concat();
    (verifier, state)
}

fn new_cursor_key() -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-cloud-cursor-key-v1\0");
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    *hasher.finalize().as_bytes()
}

async fn receive_oauth_callback(
    listener: TcpListener,
    redirect_uri: &str,
) -> Result<String, CloudSourceError> {
    let accepted = tokio::time::timeout(OAUTH_CALLBACK_TIMEOUT, listener.accept())
        .await
        .map_err(|_| CloudSourceError::OauthTimeout)?
        .map_err(|_| CloudSourceError::OauthTransport)?;
    let (mut stream, peer) = accepted;
    if !peer.ip().is_loopback() {
        return Err(CloudSourceError::OauthInvalidCallback);
    }
    let mut request = Vec::with_capacity(2 * 1024);
    let mut buffer = [0_u8; 1024];
    loop {
        let read = tokio::time::timeout(OAUTH_CALLBACK_TIMEOUT, stream.read(&mut buffer))
            .await
            .map_err(|_| CloudSourceError::OauthTimeout)?
            .map_err(|_| CloudSourceError::OauthTransport)?;
        if read == 0 {
            return Err(CloudSourceError::OauthInvalidCallback);
        }
        request.extend_from_slice(&buffer[..read]);
        if request.len() > MAX_CALLBACK_BYTES {
            return Err(CloudSourceError::OauthInvalidCallback);
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let request =
        std::str::from_utf8(&request).map_err(|_| CloudSourceError::OauthInvalidCallback)?;
    let request_line = request
        .split("\r\n")
        .next()
        .ok_or(CloudSourceError::OauthInvalidCallback)?;
    let mut parts = request_line.split(' ');
    let method = parts.next();
    let target = parts.next();
    let version = parts.next();
    if method != Some("GET")
        || !matches!(version, Some("HTTP/1.0" | "HTTP/1.1"))
        || parts.next().is_some()
    {
        return Err(CloudSourceError::OauthInvalidCallback);
    }
    let target = target
        .filter(|target| target.starts_with('/') && !target.starts_with("//"))
        .ok_or(CloudSourceError::OauthInvalidCallback)?;
    let redirect = Url::parse(redirect_uri).map_err(|_| CloudSourceError::OauthInvalidCallback)?;
    let callback = format!("{}{}", &redirect[..Position::BeforePath], target);
    let body = b"MDReader received the authorization response. You can return to the app.";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?;
    stream
        .write_all(body)
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?;
    Ok(callback)
}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: String,
    scope: Option<String>,
}

struct OAuthToken {
    access_token: Zeroizing<String>,
    refresh_token: Zeroizing<String>,
    expires_in_seconds: u64,
}

fn validate_authorization_token_response(
    provider: CloudProvider,
    authorization_profile: CloudAuthorizationProfile,
    response: OAuthTokenResponse,
) -> Result<OAuthToken, CloudSourceError> {
    let refresh_token = response
        .refresh_token
        .filter(|token| !token.is_empty())
        .ok_or(CloudSourceError::OauthOfflineGrantMissing)?;
    let Some(expires_in_seconds) = response
        .expires_in
        .filter(|seconds| *seconds > 0 && *seconds <= 365 * 24 * 60 * 60)
    else {
        eprintln!(
            "MDReader OAuth token validation rejected provider={} phase=authorization-code reason=invalid-expiry",
            provider.id()
        );
        return Err(CloudSourceError::OauthTokenRejected);
    };
    if !response.token_type.eq_ignore_ascii_case("bearer") {
        eprintln!(
            "MDReader OAuth token validation rejected provider={} phase=authorization-code reason=invalid-token-type",
            provider.id()
        );
        return Err(CloudSourceError::OauthTokenRejected);
    }
    if validated_authorization_profile(provider, authorization_profile, response.scope.as_deref())
        .is_err()
    {
        eprintln!(
            "MDReader OAuth token validation rejected provider={} phase=authorization-code reason=scope-mismatch",
            provider.id()
        );
        return Err(CloudSourceError::OauthTokenRejected);
    }
    SecretBundle::new(
        response.access_token.clone(),
        refresh_token.clone(),
        expires_at_unix_seconds(expires_in_seconds)?,
        [1; 32],
    )
    .map_err(|_| {
        eprintln!(
            "MDReader OAuth token validation rejected provider={} phase=authorization-code reason=invalid-token-shape",
            provider.id()
        );
        CloudSourceError::OauthTokenRejected
    })?;
    Ok(OAuthToken {
        access_token: Zeroizing::new(response.access_token),
        refresh_token: Zeroizing::new(refresh_token),
        expires_in_seconds,
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct RefreshedOAuthToken {
    access_token: Zeroizing<String>,
    expires_at_unix_seconds: u64,
}

async fn exchange_authorization_code(
    provider: CloudProvider,
    oauth_client: &OAuthClientConfiguration<'_>,
    attempt: &OAuthAttempt,
    authorization: &VerifiedAuthorization,
) -> Result<OAuthToken, CloudSourceError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| CloudSourceError::OauthTransport)?;
    let endpoint = match provider {
        CloudProvider::GoogleDrive => GOOGLE_TOKEN_ENDPOINT,
        CloudProvider::Dropbox => DROPBOX_TOKEN_ENDPOINT,
    };
    let verifier = attempt.verifier()?;
    let form = authorization_code_token_form(
        oauth_client,
        authorization.code(),
        verifier,
        attempt.redirect_uri(),
    );
    let response = client
        .post(endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(CloudSourceError::OauthRateLimited {
            retry_after_ms: retry_after_ms(response.headers()),
        });
    }
    let bytes = bounded_oauth_token_response(provider, "authorization-code", response).await?;
    let response: OAuthTokenResponse =
        serde_json::from_slice(&bytes).map_err(|_| CloudSourceError::OauthTokenRejected)?;
    validate_authorization_token_response(provider, attempt.authorization_profile(), response)
        .inspect_err(|error| {
            let reason = match error {
                CloudSourceError::OauthOfflineGrantMissing => "missing-refresh-token",
                CloudSourceError::OauthTokenRejected => "invalid-token-response",
                _ => "other",
            };
            eprintln!(
                "MDReader OAuth token validation rejected provider={} phase=authorization-code reason={}",
                provider.id(),
                reason
            );
        })
}

fn authorization_code_token_form<'a>(
    oauth_client: &'a OAuthClientConfiguration<'a>,
    code: &'a str,
    verifier: &'a str,
    redirect_uri: &'a str,
) -> Vec<(&'static str, &'a str)> {
    let mut form = vec![
        ("client_id", oauth_client.client_id),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(client_secret) = oauth_client.client_secret {
        form.push(("client_secret", client_secret));
    }
    form
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn exchange_refresh_token(
    provider: CloudProvider,
    authorization_profile: CloudAuthorizationProfile,
    refresh_token: &str,
) -> Result<RefreshedOAuthToken, CloudSourceError> {
    let oauth_client = oauth_client_configuration(provider)?;
    if refresh_token.is_empty()
        || refresh_token.len() > 16 * 1024
        || refresh_token.chars().any(char::is_control)
    {
        return Err(CloudSourceError::CredentialMissing);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| CloudSourceError::OauthTransport)?;
    let endpoint = match provider {
        CloudProvider::GoogleDrive => GOOGLE_TOKEN_ENDPOINT,
        CloudProvider::Dropbox => DROPBOX_TOKEN_ENDPOINT,
    };
    let form = refresh_token_form(&oauth_client, refresh_token);
    let response = client
        .post(endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(CloudSourceError::OauthRateLimited {
            retry_after_ms: retry_after_ms(response.headers()),
        });
    }
    let bytes = bounded_oauth_token_response(provider, "refresh", response).await?;
    let response: OAuthTokenResponse =
        serde_json::from_slice(&bytes).map_err(|_| CloudSourceError::OauthTokenRejected)?;
    let expires_in_seconds = response
        .expires_in
        .filter(|seconds| *seconds > 0 && *seconds <= 365 * 24 * 60 * 60)
        .ok_or(CloudSourceError::OauthTokenRejected)?;
    if response.access_token.is_empty()
        || response.access_token.len() > 16 * 1024
        || response.access_token.chars().any(char::is_control)
        || !response.token_type.eq_ignore_ascii_case("bearer")
        || validated_authorization_profile(
            provider,
            authorization_profile,
            response.scope.as_deref(),
        )
        .is_err()
    {
        return Err(CloudSourceError::OauthTokenRejected);
    }
    Ok(RefreshedOAuthToken {
        access_token: Zeroizing::new(response.access_token),
        expires_at_unix_seconds: expires_at_unix_seconds(expires_in_seconds)?,
    })
}

fn refresh_token_form<'a>(
    oauth_client: &'a OAuthClientConfiguration<'a>,
    refresh_token: &'a str,
) -> Vec<(&'static str, &'a str)> {
    let mut form = vec![
        ("client_id", oauth_client.client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    if let Some(client_secret) = oauth_client.client_secret {
        form.push(("client_secret", client_secret));
    }
    form
}

fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u32> {
    let milliseconds = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()?
        .checked_mul(1_000)?;
    u32::try_from(milliseconds).ok()
}

async fn bounded_provider_response(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, CloudSourceError> {
    if !response.status().is_success() {
        return Err(CloudSourceError::ProviderRejected);
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(CloudSourceError::ProviderRejected);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CloudSourceError::ProviderRejected);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn safe_oauth_error_class(bytes: &[u8]) -> &'static str {
    let error = serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_owned));
    match error.as_deref() {
        Some("invalid_request") => "invalid_request",
        Some("invalid_grant") => "invalid_grant",
        Some("invalid_client") => "invalid_client",
        Some("unsupported_grant_type") => "unsupported_grant_type",
        _ => "other",
    }
}

async fn bounded_oauth_token_response(
    provider: CloudProvider,
    phase: &'static str,
    response: reqwest::Response,
) -> Result<Vec<u8>, CloudSourceError> {
    let status = response.status();
    let bytes =
        bounded_provider_response_allowing_error(response, MAX_TOKEN_RESPONSE_BYTES).await?;
    if !status.is_success() {
        eprintln!(
            "MDReader OAuth token exchange rejected provider={} phase={} status={} error={}",
            provider.id(),
            phase,
            status.as_u16(),
            safe_oauth_error_class(&bytes)
        );
        return Err(CloudSourceError::OauthTokenRejected);
    }
    Ok(bytes)
}

async fn bounded_provider_response_allowing_error(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, CloudSourceError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(CloudSourceError::OauthTokenRejected);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CloudSourceError::OauthTokenRejected);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFileMetadata {
    id: String,
    mime_type: String,
    resource_key: Option<String>,
}

async fn cloud_source_config(
    provider: CloudProvider,
    token: &OAuthToken,
    authorization: &VerifiedAuthorization,
) -> Result<CloudSourceConfig, CloudSourceError> {
    match provider {
        CloudProvider::GoogleDrive => {
            let selections = fetch_drive_selections(
                token.access_token.as_str(),
                authorization.picked_file_ids(),
            )
            .await?;
            validate_drive_config(&selections)?;
            Ok(CloudSourceConfig::GoogleDrive { selections })
        }
        CloudProvider::Dropbox => {
            validate_dropbox_config()?;
            Ok(CloudSourceConfig::Dropbox)
        }
    }
}

async fn fetch_drive_selections(
    access_token: &str,
    picked_file_ids: &[String],
) -> Result<Vec<DriveItemSelection>, CloudSourceError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| CloudSourceError::OauthTransport)?;
    let mut selections = Vec::with_capacity(picked_file_ids.len());
    let mut seen = HashSet::new();
    for picked_id in picked_file_ids {
        if !seen.insert(picked_id.as_str()) {
            return Err(CloudSourceError::ProviderRejected);
        }
        let mut url = Url::parse(GOOGLE_DRIVE_FILES_ENDPOINT)
            .map_err(|_| CloudSourceError::OauthTransport)?;
        url.path_segments_mut()
            .map_err(|_| CloudSourceError::OauthTransport)?
            .pop_if_empty()
            .push(picked_id);
        url.query_pairs_mut()
            .append_pair("fields", "id,mimeType,resourceKey")
            .append_pair("supportsAllDrives", "true");
        let response = client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| CloudSourceError::OauthTransport)?;
        let bytes = bounded_provider_response(response, MAX_DRIVE_METADATA_BYTES).await?;
        let metadata: DriveFileMetadata =
            serde_json::from_slice(&bytes).map_err(|_| CloudSourceError::ProviderRejected)?;
        selections.push(drive_selection_from_metadata(picked_id, metadata)?);
    }
    Ok(selections)
}

fn drive_selection_from_metadata(
    requested_id: &str,
    metadata: DriveFileMetadata,
) -> Result<DriveItemSelection, CloudSourceError> {
    let folder = metadata.mime_type == GOOGLE_DRIVE_FOLDER_MIME;
    if metadata.id != requested_id && !(requested_id == "root" && folder) {
        return Err(CloudSourceError::ProviderRejected);
    }
    Ok(DriveItemSelection {
        id: metadata.id,
        resource_key: metadata.resource_key,
        folder,
        root: requested_id == "root",
    })
}

fn validate_drive_config(selections: &[DriveItemSelection]) -> Result<(), CloudSourceError> {
    #[cfg(feature = "google-drive-connector")]
    {
        let mut connector_selections = Vec::with_capacity(selections.len());
        for selection in selections {
            let connector_selection =
                match (selection.root, selection.folder, &selection.resource_key) {
                    (true, true, Some(key)) => {
                        catdb_connector_google_drive::DriveSelection::root_with_resource_key(
                            selection.id.clone(),
                            key.clone(),
                        )
                    }
                    (true, true, None) => Ok(catdb_connector_google_drive::DriveSelection::root(
                        selection.id.clone(),
                    )),
                    (false, true, Some(key)) => {
                        catdb_connector_google_drive::DriveSelection::folder_with_resource_key(
                            selection.id.clone(),
                            key.clone(),
                        )
                    }
                    (false, true, None) => Ok(
                        catdb_connector_google_drive::DriveSelection::folder(selection.id.clone()),
                    ),
                    (false, false, Some(key)) => {
                        catdb_connector_google_drive::DriveSelection::file_with_resource_key(
                            selection.id.clone(),
                            key.clone(),
                        )
                    }
                    (false, false, None) => Ok(catdb_connector_google_drive::DriveSelection::file(
                        selection.id.clone(),
                    )),
                    (true, false, _) => Err(
                        catdb_connector_google_drive::DriveConnectorError::InvalidConfiguration {
                            field: "selections",
                        },
                    ),
                }
                .map_err(|_| CloudSourceError::InvalidConfiguration)?;
            connector_selections.push(connector_selection);
        }
        let reference = catdb_connector_google_drive::DriveSecretReference::new("mdreader-keyring")
            .map_err(|_| CloudSourceError::InvalidConfiguration)?;
        catdb_connector_google_drive::DriveSourceConfig::new(
            "mdreader-google-drive",
            "mdreader",
            "Document",
            connector_selections,
            reference,
        )
        .map(|config| {
            config.with_content_scope(catdb_connector_google_drive::DriveContentScope::Markdown)
        })
        .map(|_| ())
        .map_err(|_| CloudSourceError::InvalidConfiguration)
    }
    #[cfg(not(feature = "google-drive-connector"))]
    {
        let _ = selections;
        Err(CloudSourceError::ConnectorUnavailable)
    }
}

fn validate_dropbox_config() -> Result<(), CloudSourceError> {
    #[cfg(feature = "dropbox-connector")]
    {
        let reference = catdb_connector_dropbox::DropboxSecretReference::new("mdreader-keyring")
            .map_err(|_| CloudSourceError::InvalidConfiguration)?;
        catdb_connector_dropbox::DropboxSourceConfig::new(
            "mdreader-dropbox",
            "mdreader",
            "Document",
            "",
            catdb_connector_dropbox::DropboxAuthorizationMode::FullDropbox,
            reference,
        )
        .map(|config| {
            config.with_content_scope(catdb_connector_dropbox::DropboxContentScope::Markdown)
        })
        .map(|_| ())
        .map_err(|_| CloudSourceError::InvalidConfiguration)
    }
    #[cfg(not(feature = "dropbox-connector"))]
    {
        Err(CloudSourceError::ConnectorUnavailable)
    }
}

#[cfg(test)]
#[derive(Default)]
struct MemoryCredentialVault {
    entries: Mutex<HashMap<String, SecretBundle>>,
    cleanup_references: Mutex<HashMap<CloudProvider, Vec<String>>>,
}

#[cfg(test)]
impl MemoryCredentialVault {
    fn entry_count(&self) -> usize {
        self.entries.lock().expect("credential vault").len()
    }

    fn clear(&self) {
        self.entries.lock().expect("credential vault").clear();
    }
}

#[cfg(test)]
impl CredentialVault for MemoryCredentialVault {
    fn get(&self, reference: &str) -> Result<Option<SecretBundle>, CloudSourceError> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| CloudSourceError::CredentialUnavailable)?
            .get(reference)
            .cloned())
    }

    fn set(&self, reference: &str, secret: &SecretBundle) -> Result<(), CloudSourceError> {
        self.entries
            .lock()
            .map_err(|_| CloudSourceError::CredentialUnavailable)?
            .insert(reference.to_string(), secret.clone());
        Ok(())
    }

    fn delete(&self, reference: &str) -> Result<(), CloudSourceError> {
        self.entries
            .lock()
            .map_err(|_| CloudSourceError::CredentialUnavailable)?
            .remove(reference);
        Ok(())
    }

    fn cleanup_references(&self, provider: CloudProvider) -> Result<Vec<String>, CloudSourceError> {
        Ok(self
            .cleanup_references
            .lock()
            .map_err(|_| CloudSourceError::CredentialUnavailable)?
            .get(&provider)
            .cloned()
            .unwrap_or_default())
    }

    fn set_cleanup_references(
        &self,
        provider: CloudProvider,
        references: &[String],
    ) -> Result<(), CloudSourceError> {
        validate_cleanup_references(references)?;
        let mut cleanup = self
            .cleanup_references
            .lock()
            .map_err(|_| CloudSourceError::CredentialUnavailable)?;
        if references.is_empty() {
            cleanup.remove(&provider);
        } else {
            cleanup.insert(provider, references.to_vec());
        }
        Ok(())
    }
}

#[cfg(test)]
struct PanicCredentialVault;

#[cfg(test)]
impl CredentialVault for PanicCredentialVault {
    fn get(&self, _reference: &str) -> Result<Option<SecretBundle>, CloudSourceError> {
        panic!("foreground Library admission must not resolve a cloud credential")
    }

    fn set(&self, _reference: &str, _secret: &SecretBundle) -> Result<(), CloudSourceError> {
        panic!("foreground Library admission must not write a cloud credential")
    }

    fn delete(&self, _reference: &str) -> Result<(), CloudSourceError> {
        panic!("foreground Library admission must not delete a cloud credential")
    }

    fn cleanup_references(
        &self,
        _provider: CloudProvider,
    ) -> Result<Vec<String>, CloudSourceError> {
        panic!("foreground Library admission must not read credential cleanup state")
    }

    fn set_cleanup_references(
        &self,
        _provider: CloudProvider,
        _references: &[String],
    ) -> Result<(), CloudSourceError> {
        panic!("foreground Library admission must not write credential cleanup state")
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Barrier,
        },
        time::{Duration, Instant},
    };

    use catdb_connectors::{
        DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor, SourceError,
        SourceFreshness, SourceId, SourceModel, SourcePage, SourcePlan, SourceRow,
    };
    use tempfile::tempdir;
    use tokio::{
        io::AsyncWriteExt as _,
        net::{TcpListener, TcpStream},
    };
    #[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
    use url::Url;

    use super::{
        admit_library_query, bind_identity_bound_local_sources, bind_library_cloud_source,
        bind_oauth_callback_listener, commit_cloud_connect, commit_cloud_connect_observed,
        commit_cloud_connect_then, dropbox_redirect_configuration, execute_cloud_query,
        oauth_callback_configuration, oauth_client_configuration_from, receive_oauth_callback,
        requested_scopes, validate_authorization_token_response,
        validate_google_oauth_client_secret, validate_oauth_client_id,
        validated_authorization_profile, AdmittedLibraryQuery, CloudAccountService,
        CloudAccountsState, CloudAuthorizationProfile, CloudCompleteness, CloudConnectOwner,
        CloudConnectionState, CloudCredentialResolver, CloudDocumentQueryResponse,
        CloudEditingCapability, CloudExecutionMode, CloudProvider, CloudQueryRefusal,
        CloudSourceConfig, CloudSourceError, CloudSourceOutcome, CloudSourceRecord,
        CloudSyncRuntime, CredentialVault, IdentityBoundLocalSource, KeyringCredentialVault,
        MemoryCredentialVault, OAuthAttempt, OAuthTokenResponse, SecretBundle, MAX_CALLBACK_BYTES,
    };
    #[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
    use super::{authorization_code_token_form, refresh_token_form};
    use crate::{
        file_lock::AdvisoryFileLock,
        library::{LibraryRegistry, LibraryStore},
    };

    // These tests deliberately release an ephemeral loopback port and then
    // rebind its exact address. Without one in-process lease, another callback
    // test can win that port in the gap and make the exact-bind assertion fail
    // nondeterministically even though the production listener is correct.
    static OAUTH_CALLBACK_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct LifecycleBlockingSyncSource {
        source_id: SourceId,
        entered: tokio::sync::Notify,
    }

    impl LifecycleBlockingSyncSource {
        fn new(source_id: &str) -> Self {
            Self {
                source_id: SourceId::new(source_id),
                entered: tokio::sync::Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::cloud_documents::MarkdownDocumentSource for LifecycleBlockingSyncSource {
        fn source_id(&self) -> &SourceId {
            &self.source_id
        }

        async fn start_change_tracking(
            &self,
            context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::ProviderCursor,
            crate::cloud_documents::CloudDocumentError,
        > {
            self.entered.notify_waiters();
            context.cancellation.cancelled().await;
            Err(crate::cloud_documents::CloudDocumentError::Cancelled)
        }

        async fn list_page(
            &self,
            _cursor: Option<&crate::cloud_documents::ProviderCursor>,
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownMetadataPage,
            crate::cloud_documents::CloudDocumentError,
        > {
            Err(crate::cloud_documents::CloudDocumentError::ProtocolFailed)
        }

        async fn read_document(
            &self,
            _key: &crate::cloud_documents::SourceRecordKey,
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownDocumentRevision,
            crate::cloud_documents::CloudDocumentError,
        > {
            Err(crate::cloud_documents::CloudDocumentError::ProtocolFailed)
        }

        async fn write_document(
            &self,
            _key: &crate::cloud_documents::SourceRecordKey,
            _expected: &crate::cloud_documents::ProviderRevision,
            _body: &[u8],
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownWriteReceipt,
            crate::cloud_documents::CloudDocumentError,
        > {
            Err(crate::cloud_documents::CloudDocumentError::ProtocolFailed)
        }

        async fn read_changes(
            &self,
            _cursor: &crate::cloud_documents::ProviderCursor,
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownChangePage,
            crate::cloud_documents::CloudDocumentError,
        > {
            Err(crate::cloud_documents::CloudDocumentError::ProtocolFailed)
        }

        async fn materialize_changes(
            &self,
            _changes: &[crate::cloud_documents::MarkdownChangeEvent],
            _known: &[crate::cloud_documents::KnownMarkdownMetadata],
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            Vec<crate::cloud_documents::MarkdownMetadata>,
            crate::cloud_documents::CloudDocumentError,
        > {
            Err(crate::cloud_documents::CloudDocumentError::ProtocolFailed)
        }
    }

    struct SchedulerSyncSource {
        source_id: SourceId,
        change_calls: AtomicUsize,
    }

    impl SchedulerSyncSource {
        fn new(source_id: &str) -> Self {
            Self {
                source_id: SourceId::new(source_id),
                change_calls: AtomicUsize::new(0),
            }
        }

        fn change_calls(&self) -> usize {
            self.change_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl crate::cloud_documents::MarkdownDocumentSource for SchedulerSyncSource {
        fn source_id(&self) -> &SourceId {
            &self.source_id
        }

        async fn start_change_tracking(
            &self,
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::ProviderCursor,
            crate::cloud_documents::CloudDocumentError,
        > {
            Ok(crate::cloud_documents::ProviderCursor::new(
                b"scheduler-c0".to_vec(),
            ))
        }

        async fn list_page(
            &self,
            _cursor: Option<&crate::cloud_documents::ProviderCursor>,
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownMetadataPage,
            crate::cloud_documents::CloudDocumentError,
        > {
            Ok(crate::cloud_documents::MarkdownMetadataPage {
                rows: Vec::new(),
                next_cursor: None,
                observed_at_ms: 1,
            })
        }

        async fn read_document(
            &self,
            _key: &crate::cloud_documents::SourceRecordKey,
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownDocumentRevision,
            crate::cloud_documents::CloudDocumentError,
        > {
            Err(crate::cloud_documents::CloudDocumentError::ProtocolFailed)
        }

        async fn write_document(
            &self,
            _key: &crate::cloud_documents::SourceRecordKey,
            _expected: &crate::cloud_documents::ProviderRevision,
            _body: &[u8],
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownWriteReceipt,
            crate::cloud_documents::CloudDocumentError,
        > {
            Err(crate::cloud_documents::CloudDocumentError::ProtocolFailed)
        }

        async fn read_changes(
            &self,
            cursor: &crate::cloud_documents::ProviderCursor,
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            crate::cloud_documents::MarkdownChangePage,
            crate::cloud_documents::CloudDocumentError,
        > {
            self.change_calls.fetch_add(1, Ordering::SeqCst);
            Ok(crate::cloud_documents::MarkdownChangePage {
                changes: Vec::new(),
                next_cursor: cursor.clone(),
                has_more: false,
                observed_at_ms: 2,
            })
        }

        async fn materialize_changes(
            &self,
            _changes: &[crate::cloud_documents::MarkdownChangeEvent],
            _known: &[crate::cloud_documents::KnownMarkdownMetadata],
            _context: &crate::cloud_documents::CloudIoContext,
        ) -> Result<
            Vec<crate::cloud_documents::MarkdownMetadata>,
            crate::cloud_documents::CloudDocumentError,
        > {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn sync_materialization_uses_the_configured_catalog_file_without_replacing_its_version() {
        let root = tempdir().expect("cloud catalog directory");
        let catalog_path = root.path().join("cloud-library.catdb");
        super::open_cloud_catalog(&catalog_path, uuid::Uuid::from_u128(0xc10d))
            .expect("open CatDB catalog");
        let before: i64 = rusqlite::Connection::open(&catalog_path)
            .expect("inspect catalog version")
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("catalog user version");
        let runtime = CloudSyncRuntime::default();
        runtime
            .configure(catalog_path.clone())
            .expect("configure shared catalog path");

        runtime
            .coordinator()
            .await
            .expect("open shared materialization");

        assert_eq!(
            runtime
                .location
                .lock()
                .expect("configured path lock")
                .as_deref(),
            Some(catalog_path.as_path())
        );
        assert!(catalog_path.exists(), "the shared catalog remains present");
        assert!(
            !root.path().join("cloud-markdown.sqlite").exists(),
            "startup must not create a parallel cloud materialization database"
        );
        let connection = rusqlite::Connection::open(&catalog_path).expect("inspect shared file");
        let after: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("catalog user version after cloud tables");
        let cloud_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name GLOB 'mdreader_cloud_*'",
                [],
                |row| row.get(0),
            )
            .expect("cloud table count");
        assert_eq!(
            after, before,
            "cloud tables do not own CatDB's schema version"
        );
        assert_eq!(
            cloud_tables, 5,
            "all cloud materialization tables share the catalog file"
        );
    }

    #[tokio::test]
    async fn periodic_refresh_is_cancelled_before_pause_and_cannot_reclaim_the_owner() {
        let root = tempdir().expect("cloud catalog directory");
        let runtime = CloudSyncRuntime::default();
        runtime
            .configure(root.path().join("cloud-library.catdb"))
            .expect("configure shared catalog path");
        let coordinator = runtime.coordinator().await.expect("open coordinator");
        let source = Arc::new(SchedulerSyncSource::new("scheduled-sync-source"));
        let source_id = source.source_id.clone();
        let pins = crate::cloud_documents::store::CloudDocumentVersionPins::new(
            b"schema-v1",
            b"mapping-v1",
            b"source-v1",
        )
        .expect("fixed test pins");
        coordinator
            .activate_with_pins(
                Arc::clone(&source) as Arc<dyn crate::cloud_documents::MarkdownDocumentSource>,
                pins,
            )
            .await
            .expect("activate source");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("initial sync settles");
        let initial_calls = source.change_calls();

        runtime
            .schedule_background_refresh_for_test(
                coordinator.clone(),
                source_id.clone(),
                Duration::from_millis(5),
            )
            .expect("schedule periodic refresh");
        tokio::time::timeout(Duration::from_secs(1), async {
            while source.change_calls() == initial_calls {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("periodic refresh runs");

        runtime
            .cancel_background_refresh(&source_id)
            .expect("cancel periodic refresh before pause");
        coordinator.pause(&source_id).await.expect("pause source");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("pause settles");
        let calls_after_pause = source.change_calls();
        tokio::time::sleep(Duration::from_millis(30)).await;

        assert_eq!(source.change_calls(), calls_after_pause);
        assert!(matches!(
            coordinator.status(&source_id).expect("pause status"),
            Some(status) if matches!(status.phase(), crate::cloud_documents::CloudSyncPhase::Paused)
        ));
    }

    #[tokio::test]
    async fn replacement_cancels_and_retires_the_old_scheduler_before_activating_the_new_source() {
        let root = tempdir().expect("cloud catalog directory");
        let runtime = CloudSyncRuntime::default();
        runtime
            .configure(root.path().join("cloud-library.catdb"))
            .expect("configure shared catalog path");
        let coordinator = runtime.coordinator().await.expect("open coordinator");
        let pins = crate::cloud_documents::store::CloudDocumentVersionPins::new(
            b"schema-v1",
            b"mapping-v1",
            b"source-v1",
        )
        .expect("fixed test pins");
        let old_source = Arc::new(SchedulerSyncSource::new("replacement-old-source"));
        let old_source_id = old_source.source_id.clone();
        coordinator
            .activate_with_pins(
                Arc::clone(&old_source) as Arc<dyn crate::cloud_documents::MarkdownDocumentSource>,
                pins.clone(),
            )
            .await
            .expect("activate old source");
        coordinator
            .wait_until_idle(&old_source_id)
            .await
            .expect("old initial sync settles");
        let old_initial_calls = old_source.change_calls();
        runtime
            .schedule_background_refresh_for_test(
                coordinator.clone(),
                old_source_id.clone(),
                Duration::from_millis(5),
            )
            .expect("schedule old refresh");
        tokio::time::timeout(Duration::from_secs(1), async {
            while old_source.change_calls() == old_initial_calls {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("old refresh runs");

        runtime
            .cancel_background_refresh(&old_source_id)
            .expect("cancel old scheduler before retirement");
        coordinator
            .retire(&old_source_id)
            .await
            .expect("retire old source before replacement");
        coordinator
            .wait_until_idle(&old_source_id)
            .await
            .expect("old source settles retired");
        let old_calls_after_retire = old_source.change_calls();

        let new_source = Arc::new(SchedulerSyncSource::new("replacement-new-source"));
        let new_source_id = new_source.source_id.clone();
        coordinator
            .activate_with_pins(
                Arc::clone(&new_source) as Arc<dyn crate::cloud_documents::MarkdownDocumentSource>,
                pins,
            )
            .await
            .expect("activate new source only after old retirement");
        coordinator
            .wait_until_idle(&new_source_id)
            .await
            .expect("new initial sync settles");
        let new_initial_calls = new_source.change_calls();
        runtime
            .schedule_background_refresh_for_test(
                coordinator.clone(),
                new_source_id.clone(),
                Duration::from_millis(5),
            )
            .expect("schedule replacement refresh");
        tokio::time::timeout(Duration::from_secs(1), async {
            while new_source.change_calls() == new_initial_calls {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("replacement refresh runs");
        tokio::time::sleep(Duration::from_millis(30)).await;

        assert_eq!(old_source.change_calls(), old_calls_after_retire);
        assert!(matches!(
            coordinator.status(&old_source_id).expect("old source status"),
            Some(status) if matches!(status.phase(), crate::cloud_documents::CloudSyncPhase::Retired)
        ));
        runtime
            .cancel_background_refresh(&new_source_id)
            .expect("stop replacement scheduler");
    }

    #[tokio::test]
    async fn pause_and_retire_reconcile_only_after_their_lifecycle_commits() {
        let root = tempdir().expect("cloud catalog directory");
        let state = CloudAccountsState::default();
        state
            .cloud_sync
            .configure(root.path().join("cloud-library.catdb"))
            .expect("configure shared catalog path");
        let coordinator = state
            .cloud_sync
            .coordinator()
            .await
            .expect("open coordinator");
        let source = Arc::new(LifecycleBlockingSyncSource::new("lifecycle-sync-source"));
        let source_id = source.source_id.clone();
        let entered = source.entered.notified();
        let pins = crate::cloud_documents::store::CloudDocumentVersionPins::new(
            b"schema-v1",
            b"mapping-v1",
            b"source-v1",
        )
        .expect("fixed test pins");
        coordinator
            .activate_with_pins(
                Arc::clone(&source) as Arc<dyn crate::cloud_documents::MarkdownDocumentSource>,
                pins,
            )
            .await
            .expect("activate source");
        entered.await;

        let pause_committed = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::clone(&pause_committed);
        let owner = CloudConnectOwner::begin(&state, CloudProvider::GoogleDrive)
            .expect("begin pause transition");
        commit_cloud_connect(&state, owner, move || {
            pause_flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("commit disabled lifecycle");
        assert!(pause_committed.load(Ordering::SeqCst));
        super::reconcile_inactive_cloud_sync_after_commit(
            &state,
            super::CloudSyncLifecycleAction::Pause(source_id.clone()),
        )
        .await
        .expect("pause only after lifecycle commit");
        coordinator
            .wait_until_idle(&source_id)
            .await
            .expect("pause settles");
        assert!(matches!(
            coordinator.status(&source_id).expect("pause status"),
            Some(status) if matches!(status.phase(), crate::cloud_documents::CloudSyncPhase::Paused)
        ));

        let retire_committed = Arc::new(AtomicBool::new(false));
        let retire_flag = Arc::clone(&retire_committed);
        let owner = CloudConnectOwner::begin(&state, CloudProvider::GoogleDrive)
            .expect("begin disconnect transition");
        commit_cloud_connect(&state, owner, move || {
            retire_flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("commit disconnected lifecycle");
        assert!(retire_committed.load(Ordering::SeqCst));
        super::reconcile_inactive_cloud_sync_after_commit(
            &state,
            super::CloudSyncLifecycleAction::Retire(source_id.clone()),
        )
        .await
        .expect("retire only after lifecycle commit");
        assert!(matches!(
            coordinator.status(&source_id).expect("retire status"),
            Some(status) if matches!(status.phase(), crate::cloud_documents::CloudSyncPhase::Retired)
        ));
    }

    #[tokio::test]
    async fn post_commit_sync_action_cannot_run_before_the_lifecycle_publication() {
        let state = CloudAccountsState::default();
        let owner = CloudConnectOwner::begin(&state, CloudProvider::Dropbox)
            .expect("begin lifecycle transition");
        let committed = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicBool::new(false));
        let committed_by_install = Arc::clone(&committed);
        let committed_by_post = Arc::clone(&committed);
        let observed_by_post = Arc::clone(&observed);

        let result = commit_cloud_connect_then(
            &state,
            owner,
            move || {
                committed_by_install.store(true, Ordering::SeqCst);
                Ok(("published", "post-commit-sync"))
            },
            move |_action| async move {
                assert!(
                    committed_by_post.load(Ordering::SeqCst),
                    "a sync action must observe the completed lifecycle publication"
                );
                observed_by_post.store(true, Ordering::SeqCst);
            },
        )
        .await;

        assert_eq!(result, Ok("published"));
        assert!(observed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cold_start_retire_removes_exact_disconnected_source_state() {
        let root = tempdir().expect("cloud catalog directory");
        let catalog_path = root.path().join("cloud-library.catdb");
        crate::cloud_documents::store::CloudDocumentStore::open(&catalog_path)
            .expect("create shared cloud tables");
        let source_id = SourceId::new("cold-start-disconnect-source");
        let generation = [4_u8; 16];
        let connection = rusqlite::Connection::open(&catalog_path).expect("seed cloud state");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_snapshot
                 (source_id, generation, state, observed_at_ms, schema_version_id, mapping_version_id, source_version_id)
                 VALUES (?1, ?2, 'published', 1, ?3, ?4, ?5)",
                rusqlite::params![
                    source_id.as_str().as_bytes(),
                    generation,
                    b"schema-v1",
                    b"mapping-v1",
                    b"source-v1",
                ],
            )
            .expect("seed snapshot");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_document
                 (source_id, generation, record_key, name, display_path, media_type, modified_at_ms, size_bytes,
                  revision, content_hash, deleted, schema_version_id, mapping_version_id, source_version_id, observed_at_ms)
                 VALUES (?1, ?2, ?3, 'note.md', '/note.md', 'text/markdown', 1, 1, ?4, NULL, 0, ?5, ?6, ?7, 1)",
                rusqlite::params![
                    source_id.as_str().as_bytes(),
                    generation,
                    b"note.md",
                    b"revision-1",
                    b"schema-v1",
                    b"mapping-v1",
                    b"source-v1",
                ],
            )
            .expect("seed metadata");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_sync_state
                 (source_id, change_cursor, sync_state, last_success_ms, last_error_kind, retry_after_ms)
                 VALUES (?1, ?2, 'published', 1, NULL, NULL)",
                rusqlite::params![source_id.as_str().as_bytes(), b"cursor-1"],
            )
            .expect("seed cursor");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_body_cache
                 (source_id, source_version_id, record_key, revision, body, body_hash, size_bytes, last_access_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 4, 1)",
                rusqlite::params![
                    source_id.as_str().as_bytes(),
                    b"source-v1",
                    b"note.md",
                    b"revision-1",
                    b"body",
                    [8_u8; 32],
                ],
            )
            .expect("seed cache");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_write_repair
                 (operation_id, source_id, record_key, accepted_revision, body_hash, receipt_json, state, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'provider-accepted', 1)",
                rusqlite::params![
                    b"operation-1",
                    source_id.as_str().as_bytes(),
                    b"note.md",
                    b"revision-1",
                    [9_u8; 32],
                    b"{}",
                ],
            )
            .expect("seed repair");
        let survivor = SourceId::new("cold-start-survivor-source");
        let survivor_generation = [5_u8; 16];
        connection
            .execute(
                "INSERT INTO mdreader_cloud_snapshot
                 (source_id, generation, state, observed_at_ms, schema_version_id, mapping_version_id, source_version_id)
                 VALUES (?1, ?2, 'published', 1, ?3, ?4, ?5)",
                rusqlite::params![
                    survivor.as_str().as_bytes(),
                    survivor_generation,
                    b"schema-v1",
                    b"mapping-v1",
                    b"source-v1",
                ],
            )
            .expect("seed survivor snapshot");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_document
                 (source_id, generation, record_key, name, display_path, media_type, modified_at_ms, size_bytes,
                  revision, content_hash, deleted, schema_version_id, mapping_version_id, source_version_id, observed_at_ms)
                 VALUES (?1, ?2, ?3, 'survivor.md', '/survivor.md', 'text/markdown', 1, 1, ?4, NULL, 0, ?5, ?6, ?7, 1)",
                rusqlite::params![
                    survivor.as_str().as_bytes(),
                    survivor_generation,
                    b"survivor.md",
                    b"survivor-revision",
                    b"schema-v1",
                    b"mapping-v1",
                    b"source-v1",
                ],
            )
            .expect("seed survivor metadata");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_sync_state
                 (source_id, change_cursor, sync_state, last_success_ms, last_error_kind, retry_after_ms)
                 VALUES (?1, ?2, 'published', 1, NULL, NULL)",
                rusqlite::params![survivor.as_str().as_bytes(), b"survivor-cursor"],
            )
            .expect("seed survivor cursor");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_body_cache
                 (source_id, source_version_id, record_key, revision, body, body_hash, size_bytes, last_access_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 4, 1)",
                rusqlite::params![
                    survivor.as_str().as_bytes(),
                    b"source-v1",
                    b"survivor.md",
                    b"survivor-revision",
                    b"body",
                    [10_u8; 32],
                ],
            )
            .expect("seed survivor cache");
        connection
            .execute(
                "INSERT INTO mdreader_cloud_write_repair
                 (operation_id, source_id, record_key, accepted_revision, body_hash, receipt_json, state, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'provider-accepted', 2)",
                rusqlite::params![
                    b"survivor-operation",
                    survivor.as_str().as_bytes(),
                    b"survivor.md",
                    b"survivor-revision",
                    [11_u8; 32],
                    b"{}",
                ],
            )
            .expect("seed survivor repair");
        drop(connection);

        let state = CloudAccountsState::default();
        state
            .cloud_sync
            .configure(catalog_path.clone())
            .expect("configure shared catalog path");
        super::reconcile_inactive_cloud_sync_after_commit(
            &state,
            super::CloudSyncLifecycleAction::Retire(source_id.clone()),
        )
        .await
        .expect("cold-start retirement");

        let connection = rusqlite::Connection::open(&catalog_path).expect("inspect cleaned state");
        for table in [
            "mdreader_cloud_snapshot",
            "mdreader_cloud_document",
            "mdreader_cloud_sync_state",
            "mdreader_cloud_body_cache",
            "mdreader_cloud_write_repair",
        ] {
            let count: i64 = connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE source_id = ?1"),
                    [source_id.as_str().as_bytes()],
                    |row| row.get(0),
                )
                .expect("source-scoped cleanup count");
            assert_eq!(count, 0, "cold-start retirement clears {table}");
            let survivor_count: i64 = connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE source_id = ?1"),
                    [survivor.as_str().as_bytes()],
                    |row| row.get(0),
                )
                .expect("survivor count");
            assert_eq!(
                survivor_count, 1,
                "retirement preserves other-source {table}"
            );
        }
    }

    #[tokio::test]
    async fn library_application_continuation_expires_at_native_authority_boundary() {
        let temp = tempdir().expect("temporary directory");
        let local_root = temp.path().join("notes");
        fs::create_dir(&local_root).expect("local source");
        fs::write(local_root.join("one.md"), "# One").expect("first document");
        fs::write(local_root.join("two.md"), "# Two").expect("second document");
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let local = registry.add_root(&local_root).expect("admit local source");
        let membership_id = local.engine_source_id();
        registry
            .save_to(&registry_path)
            .expect("persist local source");
        let admitted = admit_library_query(
            LibraryStore::new(registry_path),
            Arc::new(MemoryCredentialVault::default()),
            Some(&[membership_id]),
            None,
        )
        .expect("admit local Library");
        let AdmittedLibraryQuery {
            members,
            local_records,
            clouds,
            registry,
            vault: _,
        } = admitted;
        assert!(clouds.is_empty());
        let page = crate::library_query::begin_registered_library_query(
            catdb_api_types::EntityQueryRequest {
                workspace_id: uuid::Uuid::new_v4(),
                catalog_revision_id: None,
                schema_version_id: None,
                entity: "document.Document".to_string(),
                select: vec!["path".to_string()],
                filters: Vec::new(),
                order_by: Vec::new(),
                source_ids: Some(vec![membership_id]),
                max_staleness_ms: None,
                require_complete: Some(false),
                page_size: 1,
                limit: 10,
                query_id: None,
                cursor: None,
            },
            members,
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("first bounded Library page");
        let continuation = page.continuation.expect("native continuation");
        let cursor = continuation.token().clone();
        let expires_at = continuation.expires_at();
        let state = CloudAccountsState::default();
        state
            .store_library_continuation(super::StoredLibraryContinuation {
                continuation,
                local_records,
                cloud_records: Vec::new(),
                registry,
            })
            .expect("store application continuation");

        assert!(matches!(
            state.take_library_continuation_at(&cursor, expires_at),
            Err(CloudSourceError::QueryInvalid { .. })
        ));
    }

    #[cfg(feature = "e2e-build")]
    #[test]
    fn e2e_cloud_membership_record_satisfies_the_registry_contract() {
        super::validate_cloud_source_records(&[super::e2e_drive_record()])
            .expect("the exact E2E Drive seed must be admissible by the production Library");
    }

    #[test]
    fn legacy_records_deserialize_as_read_only_v1() {
        let record: CloudSourceRecord = serde_json::from_value(serde_json::json!({
            "id": "a6d8390b9c71bf145c8d8c3999f0e1b6a1ddcb834c7a7c7523f235e3178af620",
            "provider": "dropbox",
            "label": "Dropbox",
            "enabled": true,
            "credential_reference": "cloud-credential-legacy",
            "config": { "kind": "dropbox" }
        }))
        .expect("legacy source record");

        assert_eq!(
            record.authorization_profile,
            CloudAuthorizationProfile::ReadOnlyV1
        );
    }

    #[test]
    fn markdown_write_v2_requires_the_exact_provider_scopes() {
        assert_eq!(
            requested_scopes(
                CloudProvider::Dropbox,
                CloudAuthorizationProfile::MarkdownReadWriteV2,
            ),
            [
                "files.metadata.read",
                "files.content.read",
                "files.content.write",
            ]
        );
        assert_eq!(
            requested_scopes(
                CloudProvider::GoogleDrive,
                CloudAuthorizationProfile::MarkdownReadWriteV2,
            ),
            ["https://www.googleapis.com/auth/drive"]
        );
    }

    #[test]
    fn omitted_token_scope_does_not_upgrade_read_only_authority() {
        assert_eq!(
            validated_authorization_profile(
                CloudProvider::GoogleDrive,
                CloudAuthorizationProfile::ReadOnlyV1,
                None,
            ),
            Ok(CloudAuthorizationProfile::ReadOnlyV1)
        );
    }

    #[test]
    fn a_connected_read_only_account_requires_reconnect_for_editing() {
        let temp = tempdir().expect("temporary directory");
        let vault = MemoryCredentialVault::default();
        let registry = LibraryStore::new(temp.path().join("library.json"));
        let service = CloudAccountService::new(registry.clone(), &vault);
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("install legacy read-only source");

        let snapshot = service.snapshot().expect("connected for reads");
        let dropbox = snapshot
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::Dropbox)
            .expect("Dropbox source");
        assert_eq!(dropbox.connection, CloudConnectionState::Connected);

        let record = registry
            .read_cloud(|sources| Ok(sources[0].clone()))
            .expect("stored source");
        assert_eq!(
            record.editing_capability(),
            CloudEditingCapability::ReconnectRequired
        );
    }

    fn park_lifecycle_commit(
        commit: Arc<std::sync::Mutex<()>>,
    ) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let gate = std::thread::spawn(move || {
            let _lease = commit.lock().expect("hold lifecycle commit point");
            held_tx.send(()).expect("signal held commit point");
            release_rx.recv().expect("release lifecycle commit point");
        });
        held_rx.recv().expect("lifecycle commit point was held");
        (release_tx, gate)
    }

    fn initialize_committed_git_fixture(root: &Path) {
        for args in [
            vec!["init", "-q"],
            vec!["add", "--all"],
            vec![
                "-c",
                "user.name=MDReader Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
        ] {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .expect("git fixture command");
            assert!(
                output.status.success(),
                "git fixture command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[derive(Default)]
    struct SharedCredentialBackend {
        inner: MemoryCredentialVault,
        fail_next_delete: AtomicBool,
        fail_next_journal_write: AtomicBool,
        panic_after_next_set: AtomicBool,
        block_after_next_set: std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, Arc<Barrier>)>>,
    }

    #[derive(Clone)]
    struct SharedCredentialVault {
        backend: Arc<SharedCredentialBackend>,
    }

    impl SharedCredentialVault {
        fn new(backend: Arc<SharedCredentialBackend>) -> Self {
            Self { backend }
        }
    }

    impl CredentialVault for SharedCredentialVault {
        fn get(&self, reference: &str) -> Result<Option<SecretBundle>, CloudSourceError> {
            self.backend.inner.get(reference)
        }

        fn set(&self, reference: &str, secret: &SecretBundle) -> Result<(), CloudSourceError> {
            self.backend.inner.set(reference, secret)?;
            if self
                .backend
                .panic_after_next_set
                .swap(false, Ordering::SeqCst)
            {
                panic!("simulated process termination after credential staging");
            }
            let blocked = self
                .backend
                .block_after_next_set
                .lock()
                .map_err(|_| CloudSourceError::CredentialUnavailable)?
                .take();
            if let Some((staged, release)) = blocked {
                let _ = staged.send(());
                release.wait();
            }
            Ok(())
        }

        fn delete(&self, reference: &str) -> Result<(), CloudSourceError> {
            if self.backend.fail_next_delete.swap(false, Ordering::SeqCst) {
                return Err(CloudSourceError::CredentialUnavailable);
            }
            self.backend.inner.delete(reference)
        }

        fn cleanup_references(
            &self,
            provider: CloudProvider,
        ) -> Result<Vec<String>, CloudSourceError> {
            self.backend.inner.cleanup_references(provider)
        }

        fn set_cleanup_references(
            &self,
            provider: CloudProvider,
            references: &[String],
        ) -> Result<(), CloudSourceError> {
            if self
                .backend
                .fail_next_journal_write
                .swap(false, Ordering::SeqCst)
            {
                return Err(CloudSourceError::CredentialUnavailable);
            }
            self.backend
                .inner
                .set_cleanup_references(provider, references)
        }
    }

    struct FakeCloudSource {
        model: SourceModel,
        result: Result<(usize, Vec<SourceRow>), SourceError>,
        executions: Arc<AtomicUsize>,
    }

    impl FakeCloudSource {
        fn model(attrs: Vec<SourceAttr>) -> SourceModel {
            SourceModel::new(SourceId::new("cloud-test"), "mdreader", "Document", attrs)
        }

        fn dropbox_attrs() -> Vec<SourceAttr> {
            vec![
                SourceAttr::text("id"),
                SourceAttr::text("name"),
                SourceAttr::text("path"),
                SourceAttr::parsed("modifiedAt", catdb_core::BaseType::Timestamp),
                SourceAttr::parsed("size", catdb_core::BaseType::Int),
                SourceAttr::text("rev"),
                SourceAttr::text("contentHash"),
                SourceAttr::text("body"),
            ]
        }

        fn drive_attrs() -> Vec<SourceAttr> {
            vec![
                SourceAttr::text("id"),
                SourceAttr::text("name"),
                SourceAttr::text("mimeType"),
                SourceAttr::parsed("modifiedAt", catdb_core::BaseType::Timestamp),
                SourceAttr::parsed("size", catdb_core::BaseType::Int),
                SourceAttr::text("checksum"),
                SourceAttr::text("body"),
            ]
        }

        fn empty(scanned: usize) -> Self {
            Self {
                model: Self::model(Self::dropbox_attrs()),
                result: Ok((scanned, Vec::new())),
                executions: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn refusing() -> Self {
            Self {
                model: Self::model(Self::dropbox_attrs()),
                result: Err(SourceError::Transport {
                    source_id: SourceId::new("cloud-test"),
                }),
                executions: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn one_row() -> Self {
            Self {
                model: Self::model(Self::dropbox_attrs()),
                result: Ok((
                    1,
                    vec![SourceRow {
                        source: SourceId::new("cloud-test"),
                        entity: "mdreader.Document".to_string(),
                        key: "/research.md".to_string(),
                        value: serde_json::json!({
                            "id": "provider-record-1",
                            "path": "/research.md",
                            "name": "Research.md",
                            "modifiedAt": 1,
                            "size": 1,
                            "rev": "1",
                            "contentHash": "abc",
                            "body": "# Research",
                        }),
                    }],
                )),
                executions: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_unmappable_attribute() -> Self {
            let mut attrs = Self::dropbox_attrs();
            attrs.retain(|attr| attr.name != "body");
            attrs.push(SourceAttr::parsed("body", catdb_core::BaseType::Int));
            Self {
                model: Self::model(attrs),
                result: Ok((0, Vec::new())),
                executions: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    #[test]
    fn numeric_pushdown_refusals_keep_their_source_and_reason() {
        let source_id = SourceId::new("cloud-test");

        assert_eq!(
            super::map_query_source_error(SourceError::UnsupportedOrdering {
                source_id: source_id.clone(),
                attr: "size".to_string(),
            }),
            CloudSourceError::QueryRefused {
                source_id: source_id.to_string(),
                reason: CloudQueryRefusal::UnsupportedOrdering,
            }
        );
        assert_eq!(
            super::map_query_source_error(SourceError::UnsupportedLimit {
                source_id: source_id.clone(),
            }),
            CloudSourceError::QueryRefused {
                source_id: source_id.to_string(),
                reason: CloudQueryRefusal::UnsupportedLimit,
            }
        );
    }

    #[async_trait::async_trait]
    impl DataSource for FakeCloudSource {
        fn id(&self) -> &SourceId {
            self.model.id()
        }

        fn schema(&self) -> &catdb_core::Schema {
            self.model.schema()
        }

        fn capabilities(&self) -> &SourceCapabilities {
            self.model.capabilities()
        }

        fn source_spec(&self) -> serde_json::Value {
            serde_json::json!({"connector": "fake-cloud-source"})
        }

        async fn freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
            context.check()?;
            Ok(SourceFreshness {
                observed_at_ms: 1,
                freshness_ms: 0,
            })
        }

        fn compile(
            &self,
            query: &catdb_query::EntityQuery,
            authorized_fields: &[String],
        ) -> Result<SourcePlan, SourceError> {
            self.model.compile(query, authorized_fields)
        }

        async fn execute_page(
            &self,
            _plan: &SourcePlan,
            cursor: Option<&SourceCursor>,
            context: &ReadContext,
        ) -> Result<SourcePage, SourceError> {
            context.check()?;
            self.executions.fetch_add(1, Ordering::SeqCst);
            if cursor.is_some() {
                return Err(SourceError::InvalidCursor);
            }
            let (scanned, rows) = self.result.clone()?;
            Ok(SourcePage {
                source: self.model.id().clone(),
                scanned,
                rows,
                next_cursor: None,
                observed_at_ms: 1,
                freshness_ms: 0,
            })
        }
    }

    #[test]
    fn cloud_only_library_establishes_a_persistent_catdb_catalog_without_a_workspace() {
        let root = tempdir().expect("cloud catalog directory");
        let path = root.path().join("cloud-library.catdb");
        let workspace_id = uuid::Uuid::from_u128(0xc10d);

        let first =
            super::open_cloud_catalog(&path, workspace_id).expect("establish cloud catalog");
        let restored =
            super::open_cloud_catalog(&path, workspace_id).expect("restore cloud catalog");

        assert_eq!(first.workspace_id, workspace_id);
        assert_eq!(
            restored, first,
            "restoration must retain exact catalog pins"
        );
        assert!(path.exists(), "the cloud catalog is CatDB-managed state");
    }

    #[test]
    fn cloud_catalog_persists_the_exact_mapping_source_and_policy_pins_it_returns() {
        let root = tempdir().expect("cloud catalog directory");
        let path = root.path().join("cloud-library.catdb");
        let catalog = super::open_cloud_catalog(&path, uuid::Uuid::from_u128(0xc10d))
            .expect("establish cloud catalog");
        let source = FakeCloudSource::one_row();
        let authority = super::resolve_cloud_execution_authority(
            CloudProvider::Dropbox,
            &source,
            &catalog,
            &super::CloudReadAuthority {
                source_id: source.id().to_string(),
                connector_version: "dropbox/v2".to_string(),
            },
            None,
        )
        .expect("persist execution authority");
        let store = catdb_storage_sqlite::SqliteStore::open(&path).expect("reopen cloud catalog");
        let head = store
            .latest_catalog_revision_now()
            .expect("catalog head")
            .expect("catalog revision");

        assert_eq!(
            head.revision_id.to_string(),
            authority.pins.catalog_version,
            "the response must pin the revision that owns its execution versions"
        );
        let mapping_id =
            uuid::Uuid::parse_str(&authority.pins.mapping_version).expect("mapping pin");
        let source_id = uuid::Uuid::parse_str(&authority.pins.source_version).expect("source pin");
        let policy_id = uuid::Uuid::parse_str(&authority.pins.policy_version).expect("policy pin");
        assert!(head.mapping_version_ids.contains(&mapping_id));
        assert!(head.source_version_ids.contains(&source_id));
        assert!(head.policy_version_ids.contains(&policy_id));

        let mapping = store
            .get_mapping_version_now(mapping_id)
            .expect("mapping lookup")
            .expect("persisted mapping");
        assert_eq!(mapping.checksum, authority.pins.mapping_checksum);
        assert_eq!(
            mapping.source_schema_version_id.to_string(),
            authority.pins.source_schema_version
        );
        assert_eq!(
            mapping.target_schema_version_id.to_string(),
            authority.pins.schema_version
        );
        assert_eq!(
            store
                .get_source_version_now(source_id)
                .expect("source lookup")
                .expect("persisted source")
                .checksum,
            authority.pins.source_checksum
        );
        assert_eq!(
            store
                .get_policy_version_now(policy_id)
                .expect("policy lookup")
                .expect("persisted policy")
                .checksum,
            authority.pins.policy_checksum
        );
    }

    #[test]
    fn request_pin_is_checked_only_after_required_execution_authority_is_persisted() {
        let root = tempdir().expect("cloud catalog directory");
        let path = root.path().join("cloud-library.catdb");
        let catalog = super::open_cloud_catalog(&path, uuid::Uuid::from_u128(0xc10d))
            .expect("schema authority");
        let requested_schema_only_revision =
            uuid::Uuid::parse_str(&catalog.catalog_version).expect("schema-only revision");
        let source = FakeCloudSource::one_row();
        let authority = super::resolve_cloud_execution_authority(
            CloudProvider::Dropbox,
            &source,
            &catalog,
            &super::CloudReadAuthority {
                source_id: source.id().to_string(),
                connector_version: "dropbox/v2".to_string(),
            },
            Some(requested_schema_only_revision),
        )
        .expect("bootstrap execution authority before checking the caller pin");

        assert_ne!(
            authority.pins.catalog_version, catalog.catalog_version,
            "source, mapping, and policy authority require a new immutable revision"
        );
        assert_eq!(
            super::validate_requested_catalog_pins(
                &authority,
                Some(super::RequestedCatalogPins {
                    workspace_id: catalog.workspace_id,
                    catalog_revision_id: Some(requested_schema_only_revision),
                    schema_version_id: Some(
                        uuid::Uuid::parse_str(&catalog.schema_version).expect("schema pin"),
                    ),
                }),
            ),
            Err(CloudSourceError::QueryCatalogInvalid),
            "the stale schema-only pin is rejected only after the executable authority exists"
        );
        let head = catdb_storage_sqlite::SqliteStore::open(&path)
            .expect("catalog")
            .latest_catalog_revision_now()
            .expect("head")
            .expect("revision");
        assert_eq!(head.revision_id.to_string(), authority.pins.catalog_version);
        assert_eq!(
            source.executions.load(Ordering::SeqCst),
            0,
            "authority bootstrap must not read the provider"
        );
    }

    #[tokio::test]
    async fn an_exact_older_caller_pin_executes_without_advancing_to_the_new_head() {
        let root = tempdir().expect("cloud catalog directory");
        let path = root.path().join("cloud-library.catdb");
        let catalog = super::open_cloud_catalog(&path, uuid::Uuid::from_u128(0xc10d))
            .expect("schema authority");
        let dropbox = FakeCloudSource::one_row();
        let dropbox_authority = super::resolve_cloud_execution_authority(
            CloudProvider::Dropbox,
            &dropbox,
            &catalog,
            &super::CloudReadAuthority {
                source_id: dropbox.id().to_string(),
                connector_version: "dropbox/v2".to_string(),
            },
            None,
        )
        .expect("dropbox authority");
        let dropbox_revision =
            uuid::Uuid::parse_str(&dropbox_authority.pins.catalog_version).expect("dropbox pin");

        let drive = FakeCloudSource {
            model: FakeCloudSource::model(FakeCloudSource::drive_attrs()),
            result: Ok((0, Vec::new())),
            executions: Arc::new(AtomicUsize::new(0)),
        };
        let drive_authority = super::resolve_cloud_execution_authority(
            CloudProvider::GoogleDrive,
            &drive,
            &catalog,
            &super::CloudReadAuthority {
                source_id: drive.id().to_string(),
                connector_version: "google-drive/v2".to_string(),
            },
            None,
        )
        .expect("newer drive authority");
        let head_before = catdb_storage_sqlite::SqliteStore::open(&path)
            .expect("catalog")
            .latest_catalog_revision_now()
            .expect("head")
            .expect("revision");
        assert_eq!(
            head_before.revision_id.to_string(),
            drive_authority.pins.catalog_version
        );

        let pinned = super::resolve_cloud_execution_authority(
            CloudProvider::Dropbox,
            &dropbox,
            &catalog,
            &super::CloudReadAuthority {
                source_id: dropbox.id().to_string(),
                connector_version: "dropbox/v2".to_string(),
            },
            Some(dropbox_revision),
        )
        .expect("immutable older authority remains executable");
        let state = CloudAccountsState::default();
        let mut cached_head = catalog.clone();
        cached_head.catalog_version = drive_authority.pins.catalog_version.clone();
        *state.catalog.lock().await = Some(cached_head);
        state.advance_catalog(&pinned, Some(dropbox_revision)).await;
        assert_eq!(
            state
                .catalog
                .lock()
                .await
                .as_ref()
                .expect("cached head")
                .catalog_version,
            drive_authority.pins.catalog_version,
            "executing an older immutable pin must not rewind the authoritative head cache"
        );
        super::validate_requested_catalog_pins(
            &pinned,
            Some(super::RequestedCatalogPins {
                workspace_id: catalog.workspace_id,
                catalog_revision_id: Some(dropbox_revision),
                schema_version_id: Some(
                    uuid::Uuid::parse_str(&catalog.schema_version).expect("schema pin"),
                ),
            }),
        )
        .expect("exact pin");
        let answer = execute_cloud_query(
            CloudProvider::Dropbox,
            &dropbox,
            &pinned,
            Vec::new(),
            vec!["path".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("execute older immutable revision");
        let head_after = catdb_storage_sqlite::SqliteStore::open(&path)
            .expect("catalog")
            .latest_catalog_revision_now()
            .expect("head")
            .expect("revision");

        assert_eq!(
            answer.version_pins.catalog_version,
            dropbox_revision.to_string()
        );
        assert_eq!(
            head_after.revision_id, head_before.revision_id,
            "validation and execution must never move the caller's immutable revision to head"
        );
    }

    fn test_execution_authority(
        provider: CloudProvider,
        source: &dyn DataSource,
    ) -> super::CloudExecutionAuthority {
        let root = tempdir().expect("cloud catalog");
        let catalog = super::open_cloud_catalog(
            &root.path().join("cloud-library.catdb"),
            uuid::Uuid::from_u128(1),
        )
        .expect("persisted catalog authority");
        super::resolve_cloud_execution_authority(
            provider,
            source,
            &catalog,
            &super::CloudReadAuthority {
                source_id: source.id().to_string(),
                connector_version: format!("{provider:?}/test"),
            },
            None,
        )
        .expect("execution authority")
    }

    fn google_config() -> CloudSourceConfig {
        CloudSourceConfig::GoogleDrive {
            selections: vec![super::DriveItemSelection {
                id: "picked_file_123".to_string(),
                resource_key: None,
                folder: false,
                root: false,
            }],
        }
    }

    fn credentials() -> SecretBundle {
        SecretBundle::new(
            "access-token-value",
            "refresh-token-value",
            4_000_000_000,
            [7; 32],
        )
        .expect("valid test credentials")
    }

    fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn visit(base: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            let mut entries = fs::read_dir(current)
                .expect("read snapshot directory")
                .collect::<Result<Vec<_>, _>>()
                .expect("snapshot entries");
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                let relative = path.strip_prefix(base).expect("relative snapshot path");
                let file_type = entry.file_type().expect("snapshot file type");
                if file_type.is_dir() {
                    snapshot.insert(relative.to_path_buf(), None);
                    visit(base, &path, snapshot);
                } else {
                    snapshot.insert(
                        relative.to_path_buf(),
                        Some(fs::read(&path).expect("snapshot file bytes")),
                    );
                }
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    #[tokio::test]
    async fn rejected_cloud_read_admission_has_no_catalog_or_cached_authority_effect() {
        for case in ["invalid", "missing", "paused", "missing-credential"] {
            let temp = tempdir().expect("temporary app data");
            let app_data = temp.path().join("app-data");
            fs::create_dir(&app_data).expect("app data");
            let registry_path = app_data.join("library.json");
            let vault = MemoryCredentialVault::default();
            let service =
                CloudAccountService::new(LibraryStore::new(registry_path.clone()), &vault);

            let expected = match case {
                "missing" => {
                    crate::library::LibraryRegistry::empty()
                        .save_to(&registry_path)
                        .expect("empty Library registry");
                    service.snapshot().expect("prime Library lock");
                    CloudSourceError::SourceNotFound
                }
                "paused" => {
                    service
                        .install(CloudProvider::GoogleDrive, google_config(), credentials())
                        .expect("install Drive");
                    service
                        .set_enabled(CloudProvider::GoogleDrive, false)
                        .expect("pause Drive");
                    CloudSourceError::SourcePaused
                }
                "missing-credential" => {
                    service
                        .install(CloudProvider::GoogleDrive, google_config(), credentials())
                        .expect("install Drive");
                    vault.clear();
                    CloudSourceError::CredentialMissing
                }
                "invalid" => {
                    service
                        .install(CloudProvider::GoogleDrive, google_config(), credentials())
                        .expect("install Drive");
                    let mut stored: serde_json::Value = serde_json::from_slice(
                        &fs::read(&registry_path).expect("read Library registry"),
                    )
                    .expect("parse Library registry");
                    stored["sources"][0]["contract"]["mapping_version"] =
                        serde_json::json!("unreviewed-mapping");
                    fs::write(
                        &registry_path,
                        serde_json::to_vec_pretty(&stored).expect("encode invalid registry"),
                    )
                    .expect("persist invalid registry");
                    CloudSourceError::CorruptRegistry
                }
                _ => unreachable!(),
            };
            drop(service);

            let before = tree_snapshot(&app_data);
            let state = CloudAccountsState::default();
            let catalog_path = app_data.join("cloud-library.catdb");
            let result = super::open_admitted_cloud_query(
                async {
                    super::admit_cloud_read(
                        LibraryStore::new(registry_path),
                        vault,
                        CloudProvider::GoogleDrive,
                    )
                },
                || async {
                    fs::create_dir_all(&app_data).map_err(|_| CloudSourceError::PersistFailed)?;
                    let catalog =
                        super::open_cloud_catalog(&catalog_path, uuid::Uuid::from_u128(0xc10d))?;
                    *state.catalog.lock().await = Some(catalog.clone());
                    Ok(catalog)
                },
            )
            .await;

            assert_eq!(result.map(|_| ()), Err(expected), "{case}");
            assert!(
                tree_snapshot(&app_data) == before,
                "{case} must not change any app-data catalog entry or byte"
            );
            assert!(
                state.catalog.lock().await.is_none(),
                "{case} must not install cached catalog authority"
            );
        }
    }

    #[test]
    fn production_cloud_query_uses_the_admitted_context_boundary() {
        let source = include_str!("cloud_accounts.rs");
        let entry = source
            .split_once("async fn query_cloud_documents_inner(")
            .expect("production cloud query entry")
            .1
            .split_once("struct AdmittedLibraryQuery")
            .expect("unified Library query boundary")
            .0;

        assert_eq!(
            entry.matches("open_admitted_cloud_query(").count(),
            1,
            "the production entry must acquire catalog and resolver authority through one boundary"
        );
        assert_eq!(
            entry.matches("ensure_cloud_catalog(").count(),
            1,
            "the catalog opener must only be supplied to the admitted boundary"
        );
        assert!(
            !entry.contains("CloudCredentialResolver::from_admitted"),
            "credential snapshot construction belongs to the admitted boundary, not a bypass in the command"
        );
    }

    #[cfg(all(feature = "google-drive-connector", not(feature = "dropbox-connector")))]
    #[tokio::test]
    async fn disabled_dropbox_document_connector_refuses_before_native_authorities() {
        let admission = Arc::new(AtomicUsize::new(0));
        let vault = Arc::new(AtomicUsize::new(0));
        let resolver = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AtomicUsize::new(0));

        let admission_in_remainder = Arc::clone(&admission);
        let vault_in_remainder = Arc::clone(&vault);
        let resolver_in_remainder = Arc::clone(&resolver);
        let provider_in_remainder = Arc::clone(&provider);
        assert_eq!(
            super::with_available_cloud_document_connector(
                CloudProvider::Dropbox,
                move || async move {
                    admission_in_remainder.fetch_add(1, Ordering::SeqCst);
                    vault_in_remainder.fetch_add(1, Ordering::SeqCst);
                    resolver_in_remainder.fetch_add(1, Ordering::SeqCst);
                    provider_in_remainder.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), crate::cloud_documents::CloudDocumentError>(())
                }
            )
            .await,
            Err(crate::cloud_documents::CloudDocumentError::ConnectorUnavailable)
        );
        assert_eq!(admission.load(Ordering::SeqCst), 0);
        assert_eq!(vault.load(Ordering::SeqCst), 0);
        assert_eq!(resolver.load(Ordering::SeqCst), 0);
        assert_eq!(provider.load(Ordering::SeqCst), 0);
    }

    #[cfg(all(feature = "dropbox-connector", not(feature = "google-drive-connector")))]
    #[tokio::test]
    async fn disabled_drive_document_connector_refuses_before_native_authorities() {
        let admission = Arc::new(AtomicUsize::new(0));
        let vault = Arc::new(AtomicUsize::new(0));
        let resolver = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AtomicUsize::new(0));

        let admission_in_remainder = Arc::clone(&admission);
        let vault_in_remainder = Arc::clone(&vault);
        let resolver_in_remainder = Arc::clone(&resolver);
        let provider_in_remainder = Arc::clone(&provider);
        assert_eq!(
            super::with_available_cloud_document_connector(
                CloudProvider::GoogleDrive,
                move || async move {
                    admission_in_remainder.fetch_add(1, Ordering::SeqCst);
                    vault_in_remainder.fetch_add(1, Ordering::SeqCst);
                    resolver_in_remainder.fetch_add(1, Ordering::SeqCst);
                    provider_in_remainder.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), crate::cloud_documents::CloudDocumentError>(())
                }
            )
            .await,
            Err(crate::cloud_documents::CloudDocumentError::ConnectorUnavailable)
        );
        assert_eq!(admission.load(Ordering::SeqCst), 0);
        assert_eq!(vault.load(Ordering::SeqCst), 0);
        assert_eq!(resolver.load(Ordering::SeqCst), 0);
        assert_eq!(provider.load(Ordering::SeqCst), 0);
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    #[tokio::test]
    async fn available_cloud_document_connector_runs_the_entire_remainder() {
        #[cfg(feature = "google-drive-connector")]
        let provider = CloudProvider::GoogleDrive;
        #[cfg(all(not(feature = "google-drive-connector"), feature = "dropbox-connector"))]
        let provider = CloudProvider::Dropbox;

        let remainder_calls = Arc::new(AtomicUsize::new(0));
        let remainder_calls_in_remainder = Arc::clone(&remainder_calls);
        assert_eq!(
            super::with_available_cloud_document_connector(provider, move || async move {
                remainder_calls_in_remainder.fetch_add(1, Ordering::SeqCst);
                Ok::<(), crate::cloud_documents::CloudDocumentError>(())
            })
            .await,
            Ok(())
        );
        assert_eq!(remainder_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
    #[tokio::test]
    async fn admitted_credential_is_a_stable_snapshot_until_explicit_store() {
        let temp = tempdir().expect("temporary directory");
        let registry = LibraryStore::new(temp.path().join("library.json"));
        let vault = Arc::new(MemoryCredentialVault::default());
        let admitted = credentials();
        CloudAccountService::new(registry.clone(), vault.as_ref())
            .install(
                CloudProvider::GoogleDrive,
                google_config(),
                admitted.clone(),
            )
            .expect("seed admitted credential");
        let record = registry
            .read_cloud(|sources| {
                sources
                    .iter()
                    .find(|source| source.provider == CloudProvider::GoogleDrive)
                    .cloned()
                    .ok_or(CloudSourceError::SourceNotFound)
            })
            .expect("admitted record");
        let reference = record.credential_reference.clone();
        let resolver =
            CloudCredentialResolver::from_admitted(record, admitted, vault.clone(), registry);

        let replacement = SecretBundle::new(
            "replacement-access-token",
            "replacement-refresh-token",
            4_100_000_000,
            [8; 32],
        )
        .expect("replacement credential");
        vault
            .set(&reference, &replacement)
            .expect("mutate backing vault");
        let loaded_after_mutation = resolver.load().await.expect("admitted snapshot");
        assert_eq!(loaded_after_mutation.access_token, "access-token-value");
        assert_eq!(loaded_after_mutation.refresh_token, "refresh-token-value");

        vault.clear();
        let loaded_after_removal = resolver.load().await.expect("admitted snapshot");
        assert_eq!(loaded_after_removal.access_token, "access-token-value");
        assert_eq!(loaded_after_removal.refresh_token, "refresh-token-value");

        resolver
            .store(replacement)
            .await
            .expect("explicit refresh persistence");
        let loaded_after_store = resolver.load().await.expect("updated snapshot");
        assert_eq!(loaded_after_store.access_token, "replacement-access-token");
        assert_eq!(
            vault
                .get(&reference)
                .expect("vault read")
                .expect("stored refresh")
                .access_token,
            "replacement-access-token"
        );
    }

    #[cfg(feature = "google-drive-connector")]
    #[tokio::test]
    async fn drive_runtime_resolves_only_the_registered_vault_reference() {
        use catdb_connector_google_drive::{
            DriveConnectorError, DriveSecretReference, DriveSecretResolver,
        };

        let vault = Arc::new(MemoryCredentialVault::default());
        vault
            .set("keyring://drive", &credentials())
            .expect("seed credential");
        let resolver = CloudCredentialResolver::new(
            CloudProvider::GoogleDrive,
            "keyring://drive".to_string(),
            vault,
        );

        assert!(resolver
            .resolve(&DriveSecretReference::new("keyring://drive").expect("reference"))
            .await
            .is_ok());
        assert!(matches!(
            resolver
                .resolve(&DriveSecretReference::new("keyring://other").expect("reference"))
                .await,
            Err(DriveConnectorError::InvalidSecret)
        ));
    }

    #[cfg(feature = "dropbox-connector")]
    #[tokio::test]
    async fn dropbox_runtime_resolves_only_the_registered_vault_reference() {
        use catdb_connector_dropbox::{
            DropboxConnectorError, DropboxSecretReference, DropboxSecretResolver,
        };

        let vault = Arc::new(MemoryCredentialVault::default());
        vault
            .set("keyring://dropbox", &credentials())
            .expect("seed credential");
        let resolver = CloudCredentialResolver::new(
            CloudProvider::Dropbox,
            "keyring://dropbox".to_string(),
            vault,
        );

        assert!(resolver
            .resolve(&DropboxSecretReference::new("keyring://dropbox").expect("reference"))
            .await
            .is_ok());
        assert!(matches!(
            resolver
                .resolve(&DropboxSecretReference::new("keyring://other").expect("reference"))
                .await,
            Err(DropboxConnectorError::InvalidSecret)
        ));
    }

    #[cfg(feature = "dropbox-connector")]
    #[tokio::test]
    async fn mdreader_library_pins_dropbox_to_markdown_even_without_a_live_credential() {
        let temp = tempdir().expect("temporary directory");
        let record = super::CloudSourceRecord {
            id: "d".repeat(64),
            provider: CloudProvider::Dropbox,
            label: CloudProvider::Dropbox.label().to_string(),
            enabled: true,
            credential_reference: "mdreader-dropbox-test".to_string(),
            config: CloudSourceConfig::Dropbox,
            authorization_profile: CloudAuthorizationProfile::ReadOnlyV1,
            contract: Some(crate::library::LibrarySourceContract::cloud(
                CloudProvider::Dropbox,
            )),
        };
        let vault: Arc<dyn CredentialVault> = Arc::new(MemoryCredentialVault::default());
        let member = bind_library_cloud_source(
            record,
            None,
            vault,
            LibraryStore::new(temp.path().join("library.json")),
            "mdreader-library-schema-v1",
        )
        .await
        .expect("declared MDReader Dropbox source");

        assert_eq!(
            member.sources[0]
                .source
                .source_spec()
                .pointer("/source/config/content_scope"),
            Some(&serde_json::json!("markdown"))
        );
    }

    #[tokio::test]
    async fn cloud_query_keeps_an_empty_answer_distinct_from_a_source_failure() {
        let empty_source = FakeCloudSource::empty(7);
        let empty_authority = test_execution_authority(CloudProvider::Dropbox, &empty_source);
        let empty = execute_cloud_query(
            CloudProvider::Dropbox,
            &empty_source,
            &empty_authority,
            Vec::new(),
            vec!["path".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("successful empty query");
        assert_eq!(
            empty,
            CloudDocumentQueryResponse {
                query_id: empty.query_id.clone(),
                provider: CloudProvider::Dropbox,
                workspace_id: empty.workspace_id.clone(),
                schema_version: empty.version_pins.schema_version.clone(),
                version_pins: empty.version_pins.clone(),
                completeness: CloudCompleteness::Complete,
                sources: vec![super::CloudSourceReport {
                    source_id: "cloud-test".to_string(),
                    freshness_ms: 0,
                    outcome: CloudSourceOutcome::Ran {
                        mode: CloudExecutionMode::Virtual,
                        scanned: 7,
                        returned: 0,
                        pushed: Vec::new(),
                        residual: Vec::new(),
                    },
                }],
                scanned: 7,
                rows: Vec::new(),
            }
        );

        let refusing_source = FakeCloudSource::refusing();
        let refusing_authority = test_execution_authority(CloudProvider::Dropbox, &refusing_source);
        let failure = execute_cloud_query(
            CloudProvider::Dropbox,
            &refusing_source,
            &refusing_authority,
            Vec::new(),
            vec!["path".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            failure,
            Err(CloudSourceError::QueryTransport { source_id })
                if source_id == "cloud-test"
        ));
    }

    #[tokio::test]
    async fn successful_point_read_with_no_row_is_a_typed_not_found() {
        let source = FakeCloudSource::empty(1);
        let authority = test_execution_authority(CloudProvider::Dropbox, &source);

        let error = execute_cloud_query(
            CloudProvider::Dropbox,
            &source,
            &authority,
            vec![super::CloudFilterInput {
                field: "path".to_string(),
                operator: super::CloudFilterOperator::Eq,
                value: "/missing.md".to_string(),
            }],
            vec!["path".to_string(), "body".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect_err("a successful point read cannot silently publish no row");

        assert_eq!(
            error,
            CloudSourceError::QueryNotFound {
                source_id: "cloud-test".to_string(),
                record_key: "/missing.md".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn cloud_query_uses_the_persisted_library_schema_identity() {
        let root = tempdir().expect("cloud catalog");
        let catalog_path = root.path().join("cloud-library.catdb");
        let catalog = super::open_cloud_catalog(&catalog_path, uuid::Uuid::from_u128(1))
            .expect("persisted catalog authority");
        let source = FakeCloudSource::empty(0);
        let authority = super::resolve_cloud_execution_authority(
            CloudProvider::Dropbox,
            &source,
            &catalog,
            &super::CloudReadAuthority {
                source_id: source.id().to_string(),
                connector_version: "dropbox/test".to_string(),
            },
            None,
        )
        .expect("execution authority");

        let answer = execute_cloud_query(
            CloudProvider::Dropbox,
            &source,
            &authority,
            Vec::new(),
            vec!["path".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("cloud query");

        assert_eq!(
            answer.version_pins.schema_version, catalog.schema_version,
            "cloud execution must use the schema version persisted by the shared Library catalog"
        );
        assert_eq!(answer.schema_version, catalog.schema_version);
        uuid::Uuid::parse_str(&answer.schema_version)
            .expect("schema identity is an immutable UUID");
    }

    #[tokio::test]
    async fn cloud_query_validates_the_live_mapping_before_any_source_page() {
        let source = FakeCloudSource::with_unmappable_attribute();
        let executions = Arc::clone(&source.executions);
        let root = tempdir().expect("cloud catalog");
        let catalog = super::open_cloud_catalog(
            &root.path().join("cloud-library.catdb"),
            uuid::Uuid::from_u128(1),
        )
        .expect("persisted catalog authority");

        let result = super::resolve_cloud_execution_authority(
            CloudProvider::Dropbox,
            &source,
            &catalog,
            &super::CloudReadAuthority {
                source_id: source.id().to_string(),
                connector_version: "dropbox/test".to_string(),
            },
            None,
        );

        assert!(
            result.is_err(),
            "an unmappable source schema must be refused"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "mapping and version authority must resolve before source execution"
        );
    }

    #[tokio::test]
    async fn every_cloud_field_records_the_considered_provider_and_observation_time() {
        let source = FakeCloudSource::one_row();
        let authority = test_execution_authority(CloudProvider::Dropbox, &source);
        let answer = execute_cloud_query(
            CloudProvider::Dropbox,
            &source,
            &authority,
            Vec::new(),
            vec!["path".to_string(), "title".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("cloud query");
        let wire = serde_json::to_value(answer).expect("serialize response");

        for field in wire["rows"][0]["provenance"]
            .as_array()
            .expect("field provenance")
        {
            let providers = field["providers"].as_array().expect("considered providers");
            assert_eq!(providers.len(), 1);
            assert_eq!(providers[0]["source_id"], "cloud-test");
            assert_eq!(providers[0]["observed_at_ms"], 1);
            let expected_attribute = match field["field"].as_str().expect("semantic field") {
                "path" => "path",
                "title" => "name",
                other => panic!("unexpected semantic field {other}"),
            };
            assert_eq!(
                providers[0]["physical_attribute"], expected_attribute,
                "provenance must name the mapped provider attribute, not fabricate the semantic field"
            );
        }
    }

    #[tokio::test]
    async fn cloud_wire_preserves_null_missing_and_unobservable_without_relabelling() {
        let mut source = FakeCloudSource::one_row();
        source.result = Ok((
            1,
            vec![SourceRow {
                source: SourceId::new("cloud-test"),
                entity: "mdreader.Document".to_string(),
                key: "/null-title.md".to_string(),
                value: serde_json::json!({
                    "id": "provider-record-1",
                    "path": "/null-title.md",
                    "name": null,
                    "modifiedAt": 1,
                    "size": 1,
                    "contentHash": "abc",
                    "body": "# Null title",
                }),
            }],
        ));
        let authority = test_execution_authority(CloudProvider::Dropbox, &source);
        let cloud = execute_cloud_query(
            CloudProvider::Dropbox,
            &source,
            &authority,
            Vec::new(),
            vec![
                "title".to_string(),
                "revision".to_string(),
                "author".to_string(),
            ],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("unmapped fields remain queryable as absence evidence");
        let wire = serde_json::to_value(cloud).expect("serialize exact cloud evidence");
        let provenance = wire["rows"][0]["provenance"]
            .as_array()
            .expect("field provenance");

        assert_eq!(provenance[0]["field"], "title");
        assert_eq!(provenance[0]["selected"], 0);
        assert_eq!(
            provenance[0]["providers"][0]["observation"],
            serde_json::json!({ "kind": "observed-null" })
        );
        assert_eq!(wire["rows"][0]["fields"]["title"], serde_json::Value::Null);

        assert_eq!(provenance[1]["field"], "revision");
        assert_eq!(provenance[1]["selected"], serde_json::Value::Null);
        assert_eq!(
            provenance[1]["providers"][0]["observation"],
            serde_json::json!({
                "kind": "observed-nothing",
                "reason": "source record did not contain mapped attribute `rev`",
            })
        );
        assert!(
            wire["rows"][0]["fields"].get("revision").is_none(),
            "missing must remain absent rather than becoming JSON null"
        );

        assert_eq!(provenance[2]["field"], "author");
        assert_eq!(provenance[2]["selected"], serde_json::Value::Null);
        assert_eq!(
            provenance[2]["providers"][0]["observation"],
            serde_json::json!({
                "kind": "unobservable",
                "reason": "pinned mapping does not expose `Document.author` for `dropbox`",
            })
        );
        assert_eq!(
            provenance[2]["providers"][0]["physical_attribute"],
            serde_json::Value::Null
        );
        assert_eq!(
            provenance[2]["providers"][0]["observed_at_ms"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn drive_and_dropbox_provenance_uses_their_real_renamed_attributes() {
        let drive = FakeCloudSource {
            model: FakeCloudSource::model(FakeCloudSource::drive_attrs()),
            result: Ok((0, Vec::new())),
            executions: Arc::new(AtomicUsize::new(0)),
        };
        let drive_authority = test_execution_authority(CloudProvider::GoogleDrive, &drive);
        for (semantic, physical) in [("path", "id"), ("title", "name"), ("mediaType", "mimeType")] {
            assert_eq!(
                super::provider_field(&drive_authority.mapping, semantic),
                Some(physical)
            );
        }

        let dropbox = FakeCloudSource::empty(0);
        let dropbox_authority = test_execution_authority(CloudProvider::Dropbox, &dropbox);
        for (semantic, physical) in [
            ("providerId", "id"),
            ("title", "name"),
            ("revision", "rev"),
            ("checksum", "contentHash"),
        ] {
            assert_eq!(
                super::provider_field(&dropbox_authority.mapping, semantic),
                Some(physical)
            );
        }
    }

    #[tokio::test]
    async fn shared_entity_response_preserves_cloud_pins_and_physical_provenance() {
        let source = FakeCloudSource::one_row();
        let authority = test_execution_authority(CloudProvider::Dropbox, &source);
        let cloud = execute_cloud_query(
            CloudProvider::Dropbox,
            &source,
            &authority,
            Vec::new(),
            vec!["title".to_string(), "checksum".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("cloud query");
        let expected_mapping =
            uuid::Uuid::parse_str(&cloud.version_pins.mapping_version).expect("mapping pin");
        let expected_source =
            uuid::Uuid::parse_str(&cloud.version_pins.source_version).expect("source pin");

        let shared = super::shared_entity_response(cloud).expect("shared entity response");
        assert_eq!(shared.pins.mapping_version_ids, vec![expected_mapping]);
        assert_eq!(shared.pins.source_version_ids, vec![expected_source]);
        let row = &shared.rows[0];
        assert_eq!(
            row.provenance["title"].considered()[0].physical_attribute(),
            Some("name")
        );
        assert_eq!(
            row.provenance["checksum"].considered()[0].physical_attribute(),
            Some("contentHash")
        );
    }

    #[test]
    fn cloud_query_request_ids_make_cancellation_reachable_before_completion() {
        let state = CloudAccountsState::default();
        let first = state
            .begin_query("browse:google-drive")
            .expect("first query");
        let second = state
            .begin_query("browse:google-drive")
            .expect("replacement query");

        assert!(
            first.cancellation.is_cancelled(),
            "a replacement request must cancel the retired connector read"
        );
        assert!(!second.cancellation.is_cancelled());
        assert!(state.cancel_query("browse:google-drive"));
        assert!(second.cancellation.is_cancelled());
        assert!(!state.cancel_query("browse:google-drive"));
    }

    #[test]
    fn disconnect_cancels_inflight_oauth_and_queries_before_local_cleanup() {
        let state = CloudAccountsState::default();
        let connect = state
            .begin_connect(CloudProvider::GoogleDrive)
            .expect("connect owner");
        let query = state
            .begin_query("library:disconnect")
            .expect("query owner");

        state
            .cancel_provider_activity(CloudProvider::GoogleDrive)
            .expect("cancel provider activity");

        assert!(connect.cancellation.is_cancelled());
        assert!(query.cancellation.is_cancelled());
        assert!(!state.cancel_query("library:disconnect"));
    }

    #[test]
    fn a_replaced_oauth_attempt_cannot_publish_after_its_successor() {
        let state = CloudAccountsState::default();
        let first = state
            .begin_connect(CloudProvider::Dropbox)
            .expect("first OAuth attempt");
        let replacement = state
            .begin_connect(CloudProvider::Dropbox)
            .expect("replacement OAuth attempt");

        assert!(first.cancellation.is_cancelled());
        assert_eq!(
            state.finish_connect(&first),
            Err(CloudSourceError::OauthCancelled)
        );
        assert_eq!(state.finish_connect(&replacement), Ok(()));
    }

    #[tokio::test]
    async fn a_connect_cannot_be_retired_after_its_publication_has_started() {
        let state = CloudAccountsState::default();
        let owner =
            CloudConnectOwner::begin(&state, CloudProvider::Dropbox).expect("first OAuth attempt");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let publication = commit_cloud_connect(&state, owner, move || {
            started_tx.send(()).expect("signal publication");
            release_rx.recv().expect("release publication");
            Ok("installed")
        });
        let replacement = async {
            tokio::task::spawn_blocking(move || started_rx.recv())
                .await
                .expect("start waiter did not panic")
                .expect("publication started");
            let replacement = CloudConnectOwner::begin(&state, CloudProvider::Dropbox)
                .expect("replacement OAuth attempt");
            release_tx.send(()).expect("release publication");
            replacement
        };

        let (published, replacement) = tokio::join!(publication, replacement);
        assert_eq!(published, Ok("installed"));
        assert_eq!(replacement.finish(), Ok(()));
    }

    #[tokio::test]
    async fn dropping_a_connect_before_the_worker_commit_point_prevents_publication() {
        struct Completion(std::sync::mpsc::Sender<()>);

        impl Drop for Completion {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let state = CloudAccountsState::default();
        let owner =
            CloudConnectOwner::begin(&state, CloudProvider::Dropbox).expect("OAuth attempt");
        let published = Arc::new(AtomicBool::new(false));
        let worker_published = Arc::clone(&published);
        let (worker_started_tx, worker_started_rx) = std::sync::mpsc::channel();
        let (worker_completed_tx, worker_completed_rx) = std::sync::mpsc::channel();
        let (release_commit, commit_gate) =
            park_lifecycle_commit(Arc::clone(&state.lifecycle_commits));
        let mut publication = Box::pin(commit_cloud_connect_observed(
            &state,
            owner,
            move || {
                worker_started_tx.send(()).expect("signal worker");
                Completion(worker_completed_tx)
            },
            move || {
                worker_published.store(true, Ordering::SeqCst);
                Ok("installed")
            },
        ));

        tokio::select! {
            result = &mut publication => panic!("publication completed while commit point was parked: {result:?}"),
            started = tokio::task::spawn_blocking(move || worker_started_rx.recv()) => {
                started.expect("start waiter did not panic").expect("worker started");
            }
        }
        drop(publication);
        release_commit.send(()).expect("release commit point");
        commit_gate.join().expect("commit gate did not panic");
        tokio::task::spawn_blocking(move || worker_completed_rx.recv())
            .await
            .expect("completion waiter did not panic")
            .expect("worker completed");
        assert!(
            !published.load(Ordering::SeqCst),
            "a detached blocking worker published after its owner future was dropped"
        );
    }

    #[tokio::test]
    async fn a_stale_connect_serial_cannot_publish_after_replacement() {
        let state = CloudAccountsState::default();
        let owner = CloudConnectOwner::begin(&state, CloudProvider::Dropbox).expect("first owner");
        let published = Arc::new(AtomicBool::new(false));
        let worker_published = Arc::clone(&published);
        let (worker_started_tx, worker_started_rx) = std::sync::mpsc::channel();
        let (release_commit, commit_gate) =
            park_lifecycle_commit(Arc::clone(&state.lifecycle_commits));
        let mut publication = Box::pin(commit_cloud_connect_observed(
            &state,
            owner,
            move || worker_started_tx.send(()).expect("signal worker"),
            move || {
                worker_published.store(true, Ordering::SeqCst);
                Ok("installed")
            },
        ));

        tokio::select! {
            result = &mut publication => panic!("publication completed while commit point was parked: {result:?}"),
            started = tokio::task::spawn_blocking(move || worker_started_rx.recv()) => {
                started.expect("start waiter did not panic").expect("worker started");
            }
        }
        let replacement =
            CloudConnectOwner::begin(&state, CloudProvider::Dropbox).expect("replacement owner");
        release_commit.send(()).expect("release commit point");
        commit_gate.join().expect("commit gate did not panic");
        assert_eq!(publication.await, Err(CloudSourceError::OauthCancelled));
        assert!(!published.load(Ordering::SeqCst));
        assert_eq!(replacement.finish(), Ok(()));
    }

    #[tokio::test]
    async fn a_panicking_connect_worker_releases_ownership_and_the_commit_lease() {
        let state = CloudAccountsState::default();
        let owner = CloudConnectOwner::begin(&state, CloudProvider::Dropbox).expect("first owner");
        let result = commit_cloud_connect(&state, owner, || -> Result<(), _> {
            panic!("deliberate worker panic")
        })
        .await;
        assert_eq!(result, Err(CloudSourceError::PersistFailed));

        let replacement =
            CloudConnectOwner::begin(&state, CloudProvider::Dropbox).expect("replacement owner");
        assert_eq!(
            commit_cloud_connect(&state, replacement, || Ok("installed")).await,
            Ok("installed")
        );
    }

    #[test]
    fn a_completed_cloud_query_replaced_before_publication_is_retired() {
        let state = CloudAccountsState::default();
        let completed = state
            .begin_query("library:google-drive")
            .expect("completed query");
        let replacement = state
            .begin_query("library:google-drive")
            .expect("replacement query");

        assert_eq!(
            state.finish_query(&completed),
            Err(CloudSourceError::QueryRetired),
            "an old success delayed at the IPC boundary must fail closed"
        );
        assert_eq!(
            state.finish_query(&replacement),
            Ok(()),
            "the current serial remains publishable"
        );
    }

    #[test]
    fn dropping_a_cloud_query_owner_cancels_and_settles_only_its_exact_serial() {
        let state = CloudAccountsState::default();
        let abandoned =
            super::CloudQueryOwner::begin(&state, "library:abandoned-owner").expect("query owner");
        let abandoned_cancellation = abandoned.cancellation();
        drop(abandoned);
        assert!(
            abandoned_cancellation.is_cancelled(),
            "dropping an unfinished owner must cancel its connector work"
        );
        assert!(
            !state.cancel_query("library:abandoned-owner"),
            "dropping an unfinished owner must settle its active registration"
        );

        let retired =
            super::CloudQueryOwner::begin(&state, "library:drop-owner").expect("query owner");
        let replacement = state
            .begin_query("library:drop-owner")
            .expect("replacement query");

        drop(retired);

        assert!(
            !replacement.cancellation.is_cancelled(),
            "an old owner must never cancel a replacement with the same request ID"
        );
        assert_eq!(
            state.finish_query(&replacement),
            Ok(()),
            "the replacement must remain the exact active owner"
        );
    }

    #[test]
    fn panicking_cloud_query_owner_cancels_and_settles_its_exact_serial() {
        let state = CloudAccountsState::default();
        let owner =
            super::CloudQueryOwner::begin(&state, "library:panic-owner").expect("query owner");
        let cancellation = owner.cancellation();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owner = owner;
            panic!("simulated task panic");
        }));

        assert!(panic.is_err());
        assert!(cancellation.is_cancelled());
        assert!(
            !state.cancel_query("library:panic-owner"),
            "panic cleanup must remove the exact active owner"
        );
    }

    #[test]
    fn snapshot_waits_for_the_sibling_cloud_lifecycle_lease() {
        let temp = tempdir().expect("temporary directory");
        let registry_path = temp.path().join("library.json");
        let vault = Arc::new(MemoryCredentialVault::default());
        let lifecycle_lease = AdvisoryFileLock::acquire(&temp.path().join("cloud-accounts.lock"))
            .expect("hold lifecycle lease");
        let worker_vault = Arc::clone(&vault);
        let (completed, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let snapshot =
                CloudAccountService::new(LibraryStore::new(registry_path), worker_vault.as_ref())
                    .snapshot();
            let _ = completed.send(snapshot);
        });

        assert!(
            matches!(
                result.recv_timeout(Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "snapshot and recovery must serialize behind the cross-process lifecycle lease"
        );
        drop(lifecycle_lease);
        result
            .recv_timeout(Duration::from_secs(1))
            .expect("snapshot resumes after lease release")
            .expect("snapshot result");
        worker.join().expect("snapshot worker");
    }

    #[test]
    fn reconnect_publishes_a_new_credential_before_retiring_the_old_one() {
        let temp = tempdir().expect("temporary directory");
        let registry_path = temp.path().join("library.json");
        let vault = MemoryCredentialVault::default();
        let service = CloudAccountService::new(LibraryStore::new(registry_path.clone()), &vault);
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("initial connection");
        let old_record = LibraryStore::new(registry_path.clone())
            .read_cloud(|sources| {
                sources
                    .iter()
                    .find(|source| source.provider == CloudProvider::Dropbox)
                    .cloned()
                    .ok_or(CloudSourceError::SourceNotFound)
            })
            .expect("old source");

        let publication = service
            .install_with_profile_publication(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                SecretBundle::new(
                    "replacement-access-token",
                    "replacement-refresh-token",
                    4_000_000_100,
                    [41; 32],
                )
                .expect("replacement credential"),
                CloudAuthorizationProfile::ReadOnlyV1,
            )
            .expect("reconnect publication");
        let new_record = LibraryStore::new(registry_path)
            .read_cloud(|sources| {
                sources
                    .iter()
                    .find(|source| source.provider == CloudProvider::Dropbox)
                    .cloned()
                    .ok_or(CloudSourceError::SourceNotFound)
            })
            .expect("new source");

        assert_ne!(new_record.id, old_record.id);
        assert_eq!(
            publication.retired_source_id,
            Some(SourceId::new(old_record.id.clone())),
            "the post-commit synchronizer retires exactly the superseded source"
        );
        assert_eq!(
            publication.activated_source_id,
            SourceId::new(new_record.id.clone()),
            "the post-commit synchronizer activates exactly the committed replacement"
        );
        assert_ne!(
            new_record.credential_reference,
            old_record.credential_reference
        );
        assert!(vault
            .get(&new_record.credential_reference)
            .expect("new credential")
            .is_some());
        assert!(vault
            .get(&old_record.credential_reference)
            .expect("old credential")
            .is_none());
        assert_eq!(vault.entry_count(), 1);
    }

    #[test]
    fn reconnect_holds_the_lifecycle_lease_while_its_credential_is_staged() {
        let temp = tempdir().expect("temporary directory");
        let registry_path = temp.path().join("library.json");
        let backend = Arc::new(SharedCredentialBackend::default());
        let vault = SharedCredentialVault::new(Arc::clone(&backend));
        CloudAccountService::new(LibraryStore::new(registry_path.clone()), &vault)
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("initial connection");
        let (staged, staged_signal) = std::sync::mpsc::channel();
        let release = Arc::new(Barrier::new(2));
        *backend.block_after_next_set.lock().expect("stage barrier") =
            Some((staged, Arc::clone(&release)));

        let reconnect_path = registry_path.clone();
        let reconnect_vault = SharedCredentialVault::new(Arc::clone(&backend));
        let reconnect = std::thread::spawn(move || {
            CloudAccountService::new(LibraryStore::new(reconnect_path), &reconnect_vault).install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                SecretBundle::new(
                    "staged-access-token",
                    "staged-refresh-token",
                    4_000_000_101,
                    [42; 32],
                )
                .expect("staged credential"),
            )
        });
        staged_signal
            .recv_timeout(Duration::from_secs(1))
            .expect("credential staged before publication");

        let snapshot_path = registry_path.clone();
        let snapshot_vault = SharedCredentialVault::new(Arc::clone(&backend));
        let (snapshot_done, snapshot_result) = std::sync::mpsc::channel();
        let snapshot = std::thread::spawn(move || {
            let result =
                CloudAccountService::new(LibraryStore::new(snapshot_path), &snapshot_vault)
                    .snapshot();
            let _ = snapshot_done.send(result);
        });
        assert!(
            matches!(
                snapshot_result.recv_timeout(Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "another process must not recover a staged credential before publication"
        );

        release.wait();
        let reconnected = reconnect
            .join()
            .expect("reconnect worker")
            .expect("reconnect result");
        let listed = snapshot_result
            .recv_timeout(Duration::from_secs(1))
            .expect("snapshot resumes after publication")
            .expect("snapshot result");
        snapshot.join().expect("snapshot worker");
        let reconnect_id = reconnected
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::Dropbox)
            .expect("reconnected Dropbox")
            .id
            .clone();
        assert_eq!(
            listed
                .sources
                .iter()
                .find(|source| source.provider == CloudProvider::Dropbox)
                .expect("listed Dropbox")
                .id,
            reconnect_id
        );
        assert_eq!(backend.inner.entry_count(), 1);
    }

    #[test]
    fn cleanup_journal_write_failure_prevents_credential_staging() {
        let temp = tempdir().expect("temporary directory");
        let backend = Arc::new(SharedCredentialBackend::default());
        backend
            .fail_next_journal_write
            .store(true, Ordering::SeqCst);
        let vault = SharedCredentialVault::new(Arc::clone(&backend));
        let registry = LibraryStore::new(temp.path().join("library.json"));
        let service = CloudAccountService::new(registry.clone(), &vault);

        assert_eq!(
            service.install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            ),
            Err(CloudSourceError::CredentialUnavailable)
        );
        assert_eq!(backend.inner.entry_count(), 0);
        assert!(registry
            .read_cloud(|sources| Ok(sources.is_empty()))
            .expect("unchanged registry"));
    }

    #[test]
    fn failed_registry_publication_recovers_the_staged_credential_and_keeps_the_active_one() {
        let temp = tempdir().expect("temporary directory");
        let registry = LibraryStore::new(temp.path().join("library.json"));
        let backend = Arc::new(SharedCredentialBackend::default());
        let vault = SharedCredentialVault::new(Arc::clone(&backend));
        let service = CloudAccountService::new(registry.clone(), &vault);
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("initial connection");
        let active = registry
            .read_cloud(|sources| {
                sources
                    .iter()
                    .find(|source| source.provider == CloudProvider::Dropbox)
                    .cloned()
                    .ok_or(CloudSourceError::SourceNotFound)
            })
            .expect("active source");
        registry.fail_next_cloud_persist();

        assert_eq!(
            service.install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                SecretBundle::new(
                    "failed-access-token",
                    "failed-refresh-token",
                    4_000_000_102,
                    [43; 32],
                )
                .expect("replacement credential"),
            ),
            Err(CloudSourceError::PersistFailed)
        );
        assert_eq!(backend.inner.entry_count(), 1);
        assert!(vault
            .get(&active.credential_reference)
            .expect("active credential")
            .is_some());
        assert_eq!(
            vault
                .cleanup_references(CloudProvider::Dropbox)
                .expect("cleanup journal"),
            vec![active.credential_reference]
        );
    }

    #[test]
    fn fresh_vault_instance_recovers_persisted_cleanup_after_restart() {
        let temp = tempdir().expect("temporary directory");
        let registry_path = temp.path().join("library.json");
        let backend = Arc::new(SharedCredentialBackend::default());
        let first_vault = SharedCredentialVault::new(Arc::clone(&backend));
        let first =
            CloudAccountService::new(LibraryStore::new(registry_path.clone()), &first_vault);
        first
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("initial connection");
        backend.fail_next_delete.store(true, Ordering::SeqCst);
        assert_eq!(
            first.install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                SecretBundle::new(
                    "restart-access-token",
                    "restart-refresh-token",
                    4_000_000_103,
                    [44; 32],
                )
                .expect("replacement credential"),
            ),
            Err(CloudSourceError::CredentialUnavailable)
        );
        assert_eq!(backend.inner.entry_count(), 2);

        let restarted_vault = SharedCredentialVault::new(Arc::clone(&backend));
        let snapshot =
            CloudAccountService::new(LibraryStore::new(registry_path.clone()), &restarted_vault)
                .snapshot()
                .expect("restart recovery");
        let active = LibraryStore::new(registry_path)
            .read_cloud(|sources| {
                sources
                    .iter()
                    .find(|source| source.provider == CloudProvider::Dropbox)
                    .cloned()
                    .ok_or(CloudSourceError::SourceNotFound)
            })
            .expect("active source");

        assert_eq!(backend.inner.entry_count(), 1);
        assert!(restarted_vault
            .get(&active.credential_reference)
            .expect("active credential")
            .is_some());
        assert_eq!(
            restarted_vault
                .cleanup_references(CloudProvider::Dropbox)
                .expect("durable journal"),
            vec![active.credential_reference]
        );
        assert_eq!(
            snapshot
                .sources
                .iter()
                .find(|source| source.provider == CloudProvider::Dropbox)
                .expect("Dropbox snapshot")
                .connection,
            CloudConnectionState::Connected
        );
    }

    #[tokio::test]
    async fn retired_query_cannot_resurrect_a_credential_after_reconnect() {
        let temp = tempdir().expect("temporary directory");
        let registry = LibraryStore::new(temp.path().join("library.json"));
        let vault = Arc::new(MemoryCredentialVault::default());
        let service = CloudAccountService::new(registry.clone(), vault.as_ref());
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("initial connection");
        let retired = registry
            .read_cloud(|sources| {
                sources
                    .iter()
                    .find(|source| source.provider == CloudProvider::Dropbox)
                    .cloned()
                    .ok_or(CloudSourceError::SourceNotFound)
            })
            .expect("retired record");
        let resolver = CloudCredentialResolver::from_admitted(
            retired.clone(),
            credentials(),
            vault.clone(),
            registry.clone(),
        );

        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                SecretBundle::new(
                    "replacement-access-token",
                    "replacement-refresh-token",
                    4_000_000_104,
                    [45; 32],
                )
                .expect("replacement credential"),
            )
            .expect("reconnect");
        assert!(vault
            .get(&retired.credential_reference)
            .expect("retired credential")
            .is_none());

        assert_eq!(
            resolver
                .store(
                    SecretBundle::new(
                        "late-access-token",
                        "late-refresh-token",
                        4_000_000_105,
                        [46; 32],
                    )
                    .expect("late credential"),
                )
                .await,
            Err(CloudSourceError::QueryCatalogInvalid)
        );
        assert!(
            vault
                .get(&retired.credential_reference)
                .expect("retired credential after late store")
                .is_none(),
            "a query admitted before reconnect must not recreate its retired credential"
        );
    }

    #[test]
    fn cleanup_journal_never_deletes_another_providers_active_credential() {
        let temp = tempdir().expect("temporary directory");
        let registry = LibraryStore::new(temp.path().join("library.json"));
        let vault = MemoryCredentialVault::default();
        let service = CloudAccountService::new(registry.clone(), &vault);
        service
            .install(CloudProvider::GoogleDrive, google_config(), credentials())
            .expect("install Drive");
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                SecretBundle::new(
                    "dropbox-access-token",
                    "dropbox-refresh-token",
                    4_000_000_106,
                    [47; 32],
                )
                .expect("Dropbox credential"),
            )
            .expect("install Dropbox");
        let dropbox_reference = registry
            .read_cloud(|sources| {
                sources
                    .iter()
                    .find(|source| source.provider == CloudProvider::Dropbox)
                    .map(|source| source.credential_reference.clone())
                    .ok_or(CloudSourceError::SourceNotFound)
            })
            .expect("Dropbox reference");
        vault
            .set_cleanup_references(
                CloudProvider::GoogleDrive,
                std::slice::from_ref(&dropbox_reference),
            )
            .expect("inject cross-provider cleanup reference");

        service.snapshot().expect("recover cleanup journals");

        assert!(
            vault
                .get(&dropbox_reference)
                .expect("Dropbox credential")
                .is_some(),
            "one provider's cleanup journal must never delete another provider's active credential"
        );
        assert!(vault
            .cleanup_references(CloudProvider::GoogleDrive)
            .expect("cleaned Drive journal")
            .is_empty());
    }

    #[test]
    fn panic_after_credential_staging_is_recovered_from_the_durable_journal() {
        let temp = tempdir().expect("temporary directory");
        let registry_path = temp.path().join("library.json");
        let backend = Arc::new(SharedCredentialBackend::default());
        backend.panic_after_next_set.store(true, Ordering::SeqCst);
        let faulting_vault = SharedCredentialVault::new(Arc::clone(&backend));
        let service =
            CloudAccountService::new(LibraryStore::new(registry_path.clone()), &faulting_vault);

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = service.install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            );
        }));
        assert!(panic.is_err());
        assert_eq!(backend.inner.entry_count(), 1);
        assert_eq!(
            faulting_vault
                .cleanup_references(CloudProvider::Dropbox)
                .expect("persisted cleanup journal")
                .len(),
            1
        );

        let restarted_vault = SharedCredentialVault::new(Arc::clone(&backend));
        CloudAccountService::new(LibraryStore::new(registry_path), &restarted_vault)
            .snapshot()
            .expect("restart cleanup");
        assert_eq!(backend.inner.entry_count(), 0);
        assert!(restarted_vault
            .cleanup_references(CloudProvider::Dropbox)
            .expect("cleared cleanup journal")
            .is_empty());
    }

    #[test]
    fn pausing_keeps_credentials_and_disconnect_removes_only_catdb_managed_state() {
        let temp = tempdir().expect("temporary directory");
        let provider_file = temp.path().join("provider-owned.md");
        fs::write(&provider_file, "provider remains authoritative").expect("provider fixture");
        let vault = MemoryCredentialVault::default();
        let service =
            CloudAccountService::new(LibraryStore::new(temp.path().join("library.json")), &vault);

        service
            .install(CloudProvider::GoogleDrive, google_config(), credentials())
            .expect("install connected source");
        service
            .set_enabled(CloudProvider::GoogleDrive, false)
            .expect("pause source");

        let paused = service.snapshot().expect("paused snapshot");
        assert_eq!(
            paused.sources.len(),
            2,
            "Settings reads must retain the full supported-provider listing"
        );
        let drive = paused
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::GoogleDrive)
            .expect("Drive source");
        assert_eq!(drive.connection, CloudConnectionState::Connected);
        assert!(!drive.enabled);
        assert_eq!(vault.entry_count(), 1, "pause must retain the credential");

        service
            .disconnect_with_id(CloudProvider::GoogleDrive)
            .expect("disconnect source");

        let disconnected = service.snapshot().expect("disconnected snapshot");
        let drive = disconnected
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::GoogleDrive)
            .expect("Drive source");
        assert_eq!(drive.connection, CloudConnectionState::Disconnected);
        assert!(!drive.enabled);
        assert_eq!(vault.entry_count(), 0, "disconnect removes the credential");
        assert_eq!(
            fs::read_to_string(provider_file).expect("provider fixture remains"),
            "provider remains authoritative",
            "disconnect must never mutate provider-owned content"
        );
    }

    #[test]
    fn disconnect_publication_uses_the_identity_removed_under_its_own_lifecycle_lease() {
        let temp = tempdir().expect("temporary directory");
        let vault = MemoryCredentialVault::default();
        let service =
            CloudAccountService::new(LibraryStore::new(temp.path().join("library.json")), &vault);
        let first = service
            .install(CloudProvider::GoogleDrive, google_config(), credentials())
            .expect("initial connection");
        let stale_id = first
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::GoogleDrive)
            .expect("initial Drive source")
            .id
            .clone();
        let replacement = service
            .install(
                CloudProvider::GoogleDrive,
                google_config(),
                SecretBundle::new(
                    "replacement-access-token",
                    "replacement-refresh-token",
                    4_000_000_301,
                    [61; 32],
                )
                .expect("replacement credential"),
            )
            .expect("replacement connection");
        let replacement_id = replacement
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::GoogleDrive)
            .expect("replacement Drive source")
            .id
            .clone();
        assert_ne!(
            replacement_id, stale_id,
            "replacement owns a new source identity"
        );

        let publication = service
            .disconnect_with_id(CloudProvider::GoogleDrive)
            .expect("committed disconnect");

        assert_eq!(
            publication
                .retired_source_id
                .expect("committed retired source")
                .as_str(),
            replacement_id,
            "cleanup must follow the record committed as removed, never a pre-commit observation"
        );
        assert!(publication
            .snapshot
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::GoogleDrive)
            .is_some_and(|source| source.id.is_empty()));
    }

    #[test]
    fn cloud_account_updates_preserve_existing_local_library_membership() {
        let temp = tempdir().expect("temporary directory");
        let local_root = temp.path().join("notes");
        fs::create_dir(&local_root).expect("local source");
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&local_root).expect("admit local source");
        registry
            .save_to(&registry_path)
            .expect("persist local source");
        let vault = MemoryCredentialVault::default();
        let library = LibraryStore::new(registry_path.clone());
        let service = CloudAccountService::new(library.clone(), &vault);

        service
            .install(CloudProvider::GoogleDrive, google_config(), credentials())
            .expect("connect Drive");

        let restored = LibraryRegistry::load_from(&registry_path).expect("unified registry");
        assert_eq!(restored.sources().len(), 1);
        assert_eq!(restored.cloud_source_count(), 1);
        let snapshot = restored
            .snapshot(&temp.path().join("app-data"), None)
            .expect("unified Library snapshot");
        assert_eq!(
            snapshot.cloud_sources.len(),
            1,
            "cloud membership must be visible from the primary Library contract"
        );
        assert_eq!(
            snapshot.cloud_sources[0].id,
            service.snapshot().expect("cloud snapshot").sources[0].id
        );
        let query_snapshot = library
            .read_query_snapshot()
            .expect("one immutable query membership snapshot");
        assert_eq!(query_snapshot.local_sources.len(), 1);
        assert_eq!(query_snapshot.cloud_sources.len(), 1);

        service
            .set_enabled(CloudProvider::GoogleDrive, false)
            .expect("pause Drive");
        let paused = library
            .read_query_snapshot()
            .expect("paused query membership snapshot");
        assert_eq!(paused.local_sources.len(), 1);
        assert!(
            paused.cloud_sources.is_empty(),
            "a paused account remains connected but is not an enabled query member"
        );

        let persisted: serde_json::Value = serde_json::from_slice(
            &fs::read(&registry_path).expect("read unified Library registry"),
        )
        .expect("parse unified Library registry");
        assert!(
            persisted.get("cloud_sources").is_none(),
            "local and cloud membership must not be persisted in parallel lists"
        );
        let sources = persisted["sources"].as_array().expect("source array");
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0]["kind"], "local-folder");
        assert_eq!(sources[1]["kind"], "google-drive");
        assert_eq!(
            sources[0]["contract"]["connector_version"],
            "local-folder/v1"
        );
        assert_eq!(
            sources[0]["contract"]["mapping_version"],
            "mdreader.Document.local-folder/v2"
        );
        assert_eq!(
            sources[1]["contract"]["mapping_version"],
            "mdreader.Document.cloud-mapping/v2"
        );
        assert_eq!(sources[1]["contract"]["index_policy"], "read-through");
        assert_eq!(sources[1]["contract"]["capabilities"]["write"], false);
    }

    #[test]
    fn local_query_membership_activity_requires_exact_live_workspace_authority() {
        let temp = tempdir().expect("temporary directory");
        let local_root = temp.path().join("notes");
        fs::create_dir(&local_root).expect("local source");
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let local = registry.add_root(&local_root).expect("admit local source");
        let membership_id = local.engine_source_id();
        let bound_root = mdreader_ingest::SourceRoot::capture(
            fs::canonicalize(&local_root).expect("canonical local root"),
        )
        .expect("capture live workspace authority");
        registry
            .mark_active(&local.id, Some(&bound_root))
            .expect("persist restore candidate");
        registry
            .save_to(&registry_path)
            .expect("persist local source");

        let membership_active = |live_root: Option<&mdreader_ingest::SourceRoot>| {
            admit_library_query(
                LibraryStore::new(registry_path.clone()),
                Arc::new(MemoryCredentialVault::default()),
                Some(&[membership_id]),
                live_root,
            )
            .expect("admit local Library")
            .members
            .into_iter()
            .find(|member| member.membership_id == membership_id)
            .expect("local membership")
            .active
        };

        assert!(
            !membership_active(None),
            "a persisted restore candidate is not a live workspace grant"
        );
        assert!(
            membership_active(Some(&bound_root)),
            "the exact current SourceRoot is the live membership"
        );

        fs::rename(&local_root, temp.path().join("retired")).expect("retire admitted object");
        fs::create_dir(&local_root).expect("same-path replacement");
        assert!(
            !membership_active(Some(&bound_root)),
            "an unavailable or identity-replaced root cannot remain active"
        );
    }

    #[tokio::test]
    async fn local_query_admission_keeps_a_replaced_membership_as_a_typed_partial_source() {
        let temp = tempdir().expect("temporary directory");
        let unavailable_root = temp.path().join("unavailable");
        let healthy_root = temp.path().join("healthy");
        fs::create_dir(&unavailable_root).expect("unavailable source");
        fs::create_dir(&healthy_root).expect("healthy source");
        fs::write(unavailable_root.join("original.md"), "# original").expect("original document");
        fs::write(healthy_root.join("healthy.md"), "# healthy").expect("healthy document");
        initialize_committed_git_fixture(&healthy_root);
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let unavailable = registry
            .add_root(&unavailable_root)
            .expect("admit original object");
        let healthy = registry
            .add_root(&healthy_root)
            .expect("admit healthy object");
        let unavailable_membership = unavailable.engine_source_id();
        let healthy_membership = healthy.engine_source_id();
        registry
            .save_to(&registry_path)
            .expect("persist original identity");

        fs::rename(&unavailable_root, temp.path().join("retired")).expect("retire original object");
        fs::create_dir(&unavailable_root).expect("same-path replacement");
        fs::write(unavailable_root.join("replacement.md"), "# replacement")
            .expect("replacement document");

        let admitted = admit_library_query(
            LibraryStore::new(registry_path),
            Arc::new(
                KeyringCredentialVault::new("dev.mdreader.identity-test").expect("test vault"),
            ),
            Some(&[unavailable_membership, healthy_membership]),
            None,
        )
        .expect("admit healthy and unavailable memberships");
        let unavailable_source_ids = admitted
            .members
            .iter()
            .find(|member| member.membership_id == unavailable_membership)
            .expect("unavailable membership")
            .sources
            .iter()
            .map(|source| source.source_id)
            .collect::<Vec<_>>();

        let partial_request = catdb_api_types::EntityQueryRequest {
            workspace_id: uuid::Uuid::new_v4(),
            catalog_revision_id: None,
            schema_version_id: None,
            entity: "document.Document".to_string(),
            select: vec!["path".to_string(), "body".to_string()],
            filters: Vec::new(),
            order_by: Vec::new(),
            source_ids: Some(vec![unavailable_membership, healthy_membership]),
            max_staleness_ms: None,
            require_complete: Some(false),
            page_size: 100,
            limit: 100,
            query_id: None,
            cursor: None,
        };
        let result = crate::library_query::execute_registered_library_query(
            partial_request.clone(),
            admitted.members.clone(),
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("healthy rows survive unavailable membership");

        assert_eq!(
            result.answer.completeness,
            catdb_api_types::Completeness::Partial {
                excluded_source_ids: unavailable_source_ids.clone(),
            }
        );
        assert_eq!(result.answer.rows.len(), 1);
        assert_eq!(
            result.answer.rows[0].record_key,
            format!("library:{healthy_membership}:healthy.md")
        );
        assert!(result
            .answer
            .rows
            .iter()
            .all(|row| { row.fields.get("body") != Some(&serde_json::json!("# replacement")) }));

        let mut complete_request = partial_request;
        complete_request.require_complete = Some(true);
        let strict = crate::library_query::execute_registered_library_query(
            complete_request,
            admitted.members,
            catdb_connectors::CancellationToken::new(),
        )
        .await;
        assert_eq!(
            strict,
            Err(crate::library_query::LibraryQueryError::Incomplete {
                source_ids: unavailable_source_ids,
            })
        );
    }

    #[tokio::test]
    async fn full_library_query_keeps_healthy_local_sources_when_frontmatter_is_malformed() {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../fixtures/corpus")
            .canonicalize()
            .expect("committed corpus");
        let temp = tempdir().expect("temporary registry");
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let local = registry.add_root(&corpus).expect("admit committed corpus");
        let membership_id = local.engine_source_id();
        registry
            .save_to(&registry_path)
            .expect("persist local membership");

        let admitted = admit_library_query(
            LibraryStore::new(registry_path),
            Arc::new(MemoryCredentialVault::default()),
            Some(&[membership_id]),
            None,
        )
        .expect("admit local Library");
        let frontmatter_source_id = admitted.members[0]
            .sources
            .iter()
            .find(|source| source.source.id().as_str() == "frontmatter")
            .map(|source| source.source_id)
            .expect("frontmatter source");
        let filesystem_source_id = admitted.members[0]
            .sources
            .iter()
            .find(|source| source.source.id().as_str() == "filesystem")
            .map(|source| source.source_id)
            .expect("filesystem source");

        let answer = crate::library_query::execute_registered_library_query(
            catdb_api_types::EntityQueryRequest {
                workspace_id: uuid::Uuid::new_v4(),
                catalog_revision_id: None,
                schema_version_id: None,
                entity: "document.Document".to_string(),
                select: vec![
                    "title".to_string(),
                    "author".to_string(),
                    "path".to_string(),
                    "modifiedAt".to_string(),
                    "size".to_string(),
                    "body".to_string(),
                ],
                filters: Vec::new(),
                order_by: Vec::new(),
                source_ids: Some(vec![membership_id]),
                max_staleness_ms: None,
                require_complete: Some(false),
                page_size: 1_000,
                limit: 1_000,
                query_id: None,
                cursor: None,
            },
            admitted.members,
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("healthy filesystem and Git answers survive malformed frontmatter");

        assert!(!answer.answer.rows.is_empty());
        assert_eq!(
            answer.answer.completeness,
            catdb_api_types::Completeness::Partial {
                excluded_source_ids: vec![frontmatter_source_id],
            }
        );
        assert_eq!(
            answer
                .answer
                .sources
                .iter()
                .find(|source| source.source_id == filesystem_source_id)
                .map(|source| source.outcome),
            Some(catdb_api_types::SourceOutcome::Ran)
        );
        assert_eq!(
            answer
                .answer
                .sources
                .iter()
                .find(|source| source.source_id == frontmatter_source_id)
                .map(|source| source.outcome),
            Some(catdb_api_types::SourceOutcome::SourceFailed)
        );
    }

    #[test]
    fn foreground_library_admission_never_resolves_a_cloud_credential() {
        let temp = tempdir().expect("temporary Library registry");
        let registry_path = temp.path().join("library.json");
        let install_vault = MemoryCredentialVault::default();
        let service =
            CloudAccountService::new(LibraryStore::new(registry_path.clone()), &install_vault);
        let (provider, config) = if cfg!(feature = "google-drive-connector") {
            (CloudProvider::GoogleDrive, google_config())
        } else {
            (CloudProvider::Dropbox, CloudSourceConfig::Dropbox)
        };
        service
            .install(provider, config, credentials())
            .expect("publish cloud Library membership");

        let admitted = admit_library_query(
            LibraryStore::new(registry_path),
            Arc::new(super::PanicCredentialVault),
            None,
            None,
        )
        .expect("foreground admission reads only the registry snapshot");

        assert_eq!(admitted.members.len(), 0);
        assert_eq!(admitted.clouds.len(), 1);
        assert_eq!(admitted.clouds[0].0.provider, provider);
        assert!(admitted.clouds[0].1.is_none());
    }

    #[tokio::test]
    async fn missing_cloud_credential_is_a_typed_partial_source_not_a_library_abort() {
        let temp = tempdir().expect("temporary directory");
        let local_root = temp.path().join("healthy");
        fs::create_dir(&local_root).expect("healthy source");
        fs::write(local_root.join("healthy.md"), "# healthy").expect("healthy document");
        initialize_committed_git_fixture(&local_root);

        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let local = registry
            .add_root(&local_root)
            .expect("admit healthy object");
        let local_membership = local.engine_source_id();
        registry
            .save_to(&registry_path)
            .expect("persist healthy source");

        let vault = Arc::new(MemoryCredentialVault::default());
        let service =
            CloudAccountService::new(LibraryStore::new(registry_path.clone()), vault.as_ref());
        let (provider, config) = if cfg!(feature = "google-drive-connector") {
            (CloudProvider::GoogleDrive, google_config())
        } else {
            (CloudProvider::Dropbox, CloudSourceConfig::Dropbox)
        };
        service
            .install(provider, config, credentials())
            .expect("install cloud source");
        let cloud_membership = LibraryStore::new(registry_path.clone())
            .read_query_snapshot()
            .expect("read unified Library")
            .cloud_sources
            .iter()
            .find(|source| source.provider == provider)
            .map(|source| crate::library_query::membership_id(source.id()))
            .expect("cloud membership");
        vault.clear();

        let admitted = admit_library_query(
            LibraryStore::new(registry_path),
            vault.clone(),
            Some(&[local_membership, cloud_membership]),
            None,
        )
        .expect("missing credential remains an admitted unavailable source");
        assert_eq!(admitted.clouds.len(), 1);
        assert!(admitted.clouds[0].1.is_none());
        let AdmittedLibraryQuery {
            mut members,
            local_records: _,
            clouds,
            registry,
            vault,
        } = admitted;
        for (record, credential) in clouds {
            members.push(
                bind_library_cloud_source(
                    record,
                    credential,
                    vault.clone(),
                    registry.clone(),
                    &uuid::Uuid::new_v4().to_string(),
                )
                .await
                .expect("bind declared unavailable cloud source"),
            );
        }
        let unavailable_source_id = members
            .iter()
            .find(|member| member.membership_id == cloud_membership)
            .and_then(|member| member.sources.first())
            .map(|source| source.source_id)
            .expect("unavailable cloud source");

        let answer = crate::library_query::execute_registered_library_query(
            catdb_api_types::EntityQueryRequest {
                workspace_id: uuid::Uuid::new_v4(),
                catalog_revision_id: None,
                schema_version_id: None,
                entity: "document.Document".to_string(),
                select: vec!["path".to_string(), "body".to_string()],
                filters: Vec::new(),
                order_by: Vec::new(),
                source_ids: Some(vec![local_membership, cloud_membership]),
                max_staleness_ms: None,
                require_complete: Some(false),
                page_size: 100,
                limit: 100,
                query_id: None,
                cursor: None,
            },
            members,
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("healthy rows survive missing cloud credential");

        assert_eq!(
            answer.answer.completeness,
            catdb_api_types::Completeness::Partial {
                excluded_source_ids: vec![unavailable_source_id],
            }
        );
        assert_eq!(answer.answer.rows.len(), 1);
        assert_eq!(
            answer.answer.rows[0].record_key,
            format!("library:{local_membership}:healthy.md")
        );
        let unavailable_report = answer
            .answer
            .sources
            .iter()
            .find(|source| source.source_id == unavailable_source_id)
            .expect("unavailable cloud report");
        assert_eq!(
            unavailable_report.outcome,
            catdb_api_types::SourceOutcome::Refused
        );
        assert!(
            answer.answer.rows.iter().all(|row| {
                row.provenance.values().all(|field| {
                    field.considered().iter().all(|observation| {
                        observation.source_version_id() != unavailable_report.source_version_id
                    })
                })
            }),
            "a missing credential cannot make cloud a considered provider for a local record"
        );
    }

    #[tokio::test]
    async fn admitted_local_query_revalidates_identity_before_connector_io() {
        let temp = tempdir().expect("temporary directory");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("local source");
        fs::write(root.join("original.md"), "# original").expect("original document");
        let original_git = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .output()
            .expect("initialize original git source");
        assert!(original_git.status.success());
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let source = registry.add_root(&root).expect("admit original object");
        let membership_id = source.engine_source_id();
        registry
            .save_to(&registry_path)
            .expect("persist original identity");
        let admitted = admit_library_query(
            LibraryStore::new(registry_path),
            Arc::new(
                KeyringCredentialVault::new("dev.mdreader.identity-effect-test")
                    .expect("test vault"),
            ),
            Some(&[membership_id]),
            None,
        )
        .expect("admit exact original object");

        fs::rename(&root, temp.path().join("retired")).expect("retire original object");
        fs::create_dir(&root).expect("same-path replacement");
        fs::write(root.join("replacement.md"), "# replacement").expect("replacement document");
        let replacement_git = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .output()
            .expect("initialize replacement git source");
        assert!(replacement_git.status.success());

        let result = crate::library_query::execute_registered_library_query(
            catdb_api_types::EntityQueryRequest {
                workspace_id: uuid::Uuid::new_v4(),
                catalog_revision_id: None,
                schema_version_id: None,
                entity: "document.Document".to_string(),
                select: vec!["path".to_string(), "body".to_string()],
                filters: Vec::new(),
                order_by: Vec::new(),
                source_ids: Some(vec![membership_id]),
                max_staleness_ms: None,
                require_complete: Some(true),
                page_size: 100,
                limit: 100,
                query_id: None,
                cursor: None,
            },
            admitted.members,
            catdb_connectors::CancellationToken::new(),
        )
        .await;

        let Err(crate::library_query::LibraryQueryError::ExecutionFailed { sources }) = result
        else {
            panic!("identity replacement must fail before connector I/O");
        };
        assert_eq!(sources.len(), 3);
        assert!(sources
            .iter()
            .all(|source| source.outcome == catdb_api_types::SourceOutcome::RuntimeRefused));
    }

    #[test]
    fn forgetting_local_membership_retires_admitted_query_before_publication() {
        let temp = tempdir().expect("temporary directory");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("local source");
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        let admitted = registry.add_root(&root).expect("admit local membership");
        registry
            .save_to(&registry_path)
            .expect("persist admitted membership");

        let (query_admitted_tx, query_admitted_rx) = std::sync::mpsc::channel();
        let (publish_tx, publish_rx) = std::sync::mpsc::channel();
        let query_store = LibraryStore::new(registry_path.clone());
        let expected = admitted.clone();
        let query = std::thread::spawn(move || {
            query_admitted_tx.send(()).expect("query admitted");
            publish_rx.recv().expect("release publication");
            super::validate_current_library_memberships(
                &query_store,
                std::slice::from_ref(&expected),
                &[],
            )
        });
        query_admitted_rx.recv().expect("query admission");
        registry
            .forget(&admitted.id, None)
            .expect("forget overlapping membership");
        registry
            .save_to(&registry_path)
            .expect("publish forgotten membership");
        publish_tx.send(()).expect("release query publication");
        assert_eq!(
            query.join().expect("query publication check"),
            Err(CloudSourceError::QueryRetired)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_connector_handoff_retains_a_across_an_exact_a_b_a_path_swap() {
        fn initialize(root: &Path, filename: &str, body: &str) {
            fs::create_dir(root).expect("source directory");
            fs::write(root.join(filename), body).expect("source document");
            initialize_committed_git_fixture(root);
        }

        let temp = tempdir().expect("temporary directory");
        let root = temp.path().join("notes");
        let replacement = temp.path().join("replacement");
        let retired = temp.path().join("retired");
        initialize(&root, "original.md", "# Original\n");
        initialize(&replacement, "replacement.md", "# Replacement\n");
        let bound_root = mdreader_ingest::SourceRoot::capture(
            fs::canonicalize(&root).expect("canonical original root"),
        )
        .expect("bind authority A");
        let membership_id = uuid::Uuid::new_v4();

        fs::rename(&root, &retired).expect("retire A");
        fs::rename(&replacement, &root).expect("publish B at the admitted path");
        let physical = bind_identity_bound_local_sources(membership_id, &bound_root)
            .expect("handoff the already-admitted authority");
        fs::rename(&root, &replacement).expect("retire B");
        fs::rename(&retired, &root).expect("restore A before outer verification");

        let result = crate::library_query::execute_registered_library_query(
            catdb_api_types::EntityQueryRequest {
                workspace_id: uuid::Uuid::new_v4(),
                catalog_revision_id: None,
                schema_version_id: None,
                entity: "document.Document".to_string(),
                select: vec!["path".to_string(), "body".to_string()],
                filters: Vec::new(),
                order_by: Vec::new(),
                source_ids: Some(vec![membership_id]),
                max_staleness_ms: None,
                require_complete: Some(true),
                page_size: 100,
                limit: 100,
                query_id: None,
                cursor: None,
            },
            vec![crate::library_query::LibraryQueryMember {
                membership_id,
                registry_id: "local-a".to_string(),
                name: "Authority A".to_string(),
                kind: crate::library_query::LibraryQueryMemberKind::LocalFolder,
                editable: true,
                active: true,
                sources: physical,
            }],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("identity-bound A query");
        let keys = result
            .answer
            .rows
            .iter()
            .map(|row| row.record_key.as_str())
            .collect::<Vec<_>>();
        let expected_key = format!("library:{membership_id}:original.md");

        assert_eq!(keys, [expected_key.as_str()]);
        assert!(result
            .answer
            .rows
            .iter()
            .all(|row| row.fields.get("body") != Some(&serde_json::json!("# Replacement\n"))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn identity_bound_git_never_observes_an_overlapping_a_b_a_replacement() {
        use std::os::unix::fs::PermissionsExt as _;

        fn commit(root: &Path, filename: &str, author: &str) {
            fs::create_dir(root).expect("repository directory");
            fs::write(root.join(filename), format!("# {author}\n")).expect("tracked document");
            for args in [
                vec!["init", "-q"],
                vec!["add", "--all"],
                vec![
                    "-c",
                    &format!("user.name={author}"),
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    author,
                ],
            ] {
                let output = std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .output()
                    .expect("git fixture command");
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }

        let temp = tempdir().expect("temporary directory");
        let root = temp.path().join("notes");
        let replacement = temp.path().join("replacement");
        let retired = temp.path().join("retired");
        commit(&root, "original.md", "Original Author");
        commit(&replacement, "replacement.md", "Replacement Author");
        let bound_root = mdreader_ingest::SourceRoot::capture(
            fs::canonicalize(&root).expect("canonical original root"),
        )
        .expect("bind original root");

        let shim = temp.path().join("git-overlap");
        fs::write(
            &shim,
            format!(
                "#!/bin/sh\nset -eu\nmv '{}' '{}'\nmv '{}' '{}'\nset +e\ngit \"$@\"\nstatus=$?\nset -e\nmv '{}' '{}'\nmv '{}' '{}'\nexit \"$status\"\n",
                root.display(),
                retired.display(),
                replacement.display(),
                root.display(),
                root.display(),
                replacement.display(),
                retired.display(),
                root.display(),
            ),
        )
        .expect("git overlap shim");
        let mut permissions = fs::metadata(&shim).expect("shim metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&shim, permissions).expect("executable shim");

        let source: Arc<dyn DataSource> = Arc::new(IdentityBoundLocalSource {
            root: bound_root,
            source: Arc::new(catdb_connectors::GitSource::open_with_git_binary(
                &root, &shim,
            )),
        });
        let query = catdb_query::EntityQuery {
            target: catdb_query::EntityTarget::Entity("doc.Document".to_string()),
            filters: Vec::new(),
        };
        let plan = source
            .compile(&query, &["path".to_string()])
            .expect("compile tracked paths");
        let context = ReadContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_secs(5),
            100,
            1024 * 1024,
            100,
            catdb_connectors::CancellationToken::new(),
        )
        .expect("bounded read context");
        let page = source
            .execute_page(&plan, None, &context)
            .await
            .expect("identity-bound read");
        let keys = page
            .rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>();

        assert_eq!(keys, ["original.md"]);
        assert!(root.join("original.md").is_file());
        assert!(replacement.join("replacement.md").is_file());
    }

    #[test]
    fn version_four_parallel_membership_migrates_once_to_first_class_sources() {
        let temp = tempdir().expect("temporary directory");
        let local_root = temp.path().join("notes");
        fs::create_dir(&local_root).expect("local source");
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&local_root).expect("admit local source");
        registry
            .save_to(&registry_path)
            .expect("persist local source");
        let vault = MemoryCredentialVault::default();
        let service = CloudAccountService::new(LibraryStore::new(registry_path.clone()), &vault);
        service
            .install(CloudProvider::GoogleDrive, google_config(), credentials())
            .expect("connect Drive");

        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read V5 registry"))
                .expect("parse V5 registry");
        let mut entries = legacy["sources"]
            .as_array_mut()
            .expect("source array")
            .drain(..)
            .collect::<Vec<_>>();
        let mut local = entries.remove(0);
        {
            let local_record = local.as_object_mut().expect("local source");
            local_record.remove("kind");
            local_record.get_mut("contract").expect("local contract")["mapping_version"] =
                serde_json::json!("mdreader.Document.local-folder/v1");
        }
        let mut drive = entries.remove(0);
        {
            let drive_record = drive.as_object_mut().expect("Drive source");
            drive_record.remove("kind");
            drive_record.get_mut("contract").expect("Drive contract")["mapping_version"] =
                serde_json::json!("mdreader.Document.cloud-mapping/v1");
        }
        legacy["version"] = serde_json::json!(4);
        legacy["sources"] = serde_json::json!([local]);
        legacy["cloud_sources"] = serde_json::json!([drive]);
        legacy
            .as_object_mut()
            .expect("registry object")
            .remove("connected_catdb_rediscovery_required");
        legacy
            .as_object_mut()
            .expect("registry object")
            .remove("restore_suppressed_source_ids");
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&legacy).expect("encode V4 registry"),
        )
        .expect("write V4 registry");

        let migrated = LibraryRegistry::load_from(&registry_path).expect("migrate V4");
        assert_eq!(migrated.sources().len(), 1);
        assert_eq!(migrated.cloud_source_count(), 1);
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read migrated registry"))
                .expect("parse migrated registry");
        assert_eq!(
            persisted["version"],
            serde_json::json!(crate::library::LIBRARY_VERSION)
        );
        assert!(persisted.get("cloud_sources").is_none());
        assert_eq!(persisted["sources"][0]["kind"], "local-folder");
        assert_eq!(persisted["sources"][1]["kind"], "google-drive");
        assert_eq!(
            persisted["sources"][0]["contract"]["mapping_version"],
            "mdreader.Document.local-folder/v2"
        );
        assert_eq!(
            persisted["sources"][1]["contract"]["mapping_version"],
            "mdreader.Document.cloud-mapping/v2"
        );
    }

    #[test]
    fn version_five_contracts_migrate_to_the_unified_document_contract() {
        let temp = tempdir().expect("temporary directory");
        let local_root = temp.path().join("notes");
        fs::create_dir(&local_root).expect("local source");
        let registry_path = temp.path().join("library.json");
        let mut registry = LibraryRegistry::empty();
        registry.add_root(&local_root).expect("admit local source");
        registry
            .save_to(&registry_path)
            .expect("persist current registry");
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read registry"))
                .expect("parse registry");
        legacy["version"] = serde_json::json!(5);
        legacy["sources"][0]["contract"]["connector_version"] =
            serde_json::json!("local-folder/v1");
        legacy["sources"][0]["contract"]["mapping_version"] =
            serde_json::json!("mdreader.Document.local-folder/v1");
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&legacy).expect("encode V5 registry"),
        )
        .expect("write V5 registry");

        LibraryRegistry::load_from(&registry_path).expect("migrate V5 registry");
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry_path).expect("read migrated registry"))
                .expect("parse migrated registry");
        assert_eq!(
            persisted["version"],
            serde_json::json!(crate::library::LIBRARY_VERSION)
        );
        assert_eq!(
            persisted["sources"][0]["contract"]["connector_version"],
            "local-folder/v1"
        );
        assert_eq!(
            persisted["sources"][0]["contract"]["mapping_version"],
            "mdreader.Document.local-folder/v2"
        );
    }

    #[test]
    fn disconnect_restores_the_credential_when_registry_publication_fails() {
        let temp = tempdir().expect("temporary directory");
        let vault = MemoryCredentialVault::default();
        let registry = LibraryStore::new(temp.path().join("library.json"));
        let service = CloudAccountService::new(registry.clone(), &vault);
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("install connected source");
        registry.fail_next_cloud_persist();

        assert_eq!(
            service
                .disconnect_with_id(CloudProvider::Dropbox)
                .map(|publication| publication.snapshot),
            Err(CloudSourceError::PersistFailed)
        );
        assert_eq!(
            vault.entry_count(),
            1,
            "the deleted credential must be restored when state publication fails"
        );
        let snapshot = service.snapshot().expect("pre-failure state remains");
        let dropbox = snapshot
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::Dropbox)
            .expect("Dropbox source");
        assert_eq!(dropbox.connection, CloudConnectionState::Connected);
    }

    #[test]
    fn a_wrong_oauth_state_cannot_produce_a_verified_authorization() {
        let (provider, redirect) = if cfg!(feature = "google-drive-connector") {
            (
                CloudProvider::GoogleDrive,
                "http://127.0.0.1:53681/oauth/google-drive",
            )
        } else {
            (
                CloudProvider::Dropbox,
                "http://127.0.0.1:53682/oauth/dropbox",
            )
        };
        let attempt = OAuthAttempt::new(provider, "a".repeat(64), "expected-state", redirect)
            .expect("valid attempt");

        let callback = format!("{redirect}?code=provider-code&state=wrong-state");
        let result = attempt.verify_callback(&callback);

        assert!(matches!(result, Err(CloudSourceError::OauthStateMismatch)));
    }

    #[test]
    fn oauth_callback_requires_the_exact_redirect_and_unique_parameters() {
        let (provider, redirect, wrong_redirect) = if cfg!(feature = "google-drive-connector") {
            (
                CloudProvider::GoogleDrive,
                "http://127.0.0.1:53681/oauth/google-drive",
                "http://127.0.0.1:53681/oauth/other",
            )
        } else {
            (
                CloudProvider::Dropbox,
                "http://127.0.0.1:53682/oauth/dropbox",
                "http://127.0.0.1:53682/oauth/other",
            )
        };
        let attempt = OAuthAttempt::new(provider, "a".repeat(64), "expected-state", redirect)
            .expect("valid attempt");

        let wrong_callback = format!("{wrong_redirect}?code=provider-code&state=expected-state");
        let duplicate_callback = format!("{redirect}?code=one&code=two&state=expected-state");
        for callback in [wrong_callback.as_str(), duplicate_callback.as_str()] {
            assert!(matches!(
                attempt.verify_callback(callback),
                Err(CloudSourceError::OauthInvalidCallback)
            ));
        }
    }

    #[test]
    #[cfg(feature = "google-drive-connector")]
    fn readonly_drive_callback_defaults_to_the_drive_root_without_picker_parameters() {
        let attempt = OAuthAttempt::new(
            CloudProvider::GoogleDrive,
            "a".repeat(64),
            "expected-state",
            "http://127.0.0.1:53681/oauth/google-drive",
        )
        .expect("valid attempt");

        let authorization = attempt
            .verify_callback(
                "http://127.0.0.1:53681/oauth/google-drive?code=provider-code&state=expected-state",
            )
            .expect("standard OAuth callback");

        assert_eq!(authorization.picked_file_ids(), ["root"]);
    }

    #[test]
    fn authorization_token_response_accepts_omitted_unchanged_scope() {
        let result = validate_authorization_token_response(
            CloudProvider::GoogleDrive,
            CloudAuthorizationProfile::ReadOnlyV1,
            OAuthTokenResponse {
                access_token: "access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                expires_in: Some(3600),
                token_type: "Bearer".to_string(),
                scope: None,
            },
        );

        assert!(result.is_ok());
    }

    #[test]
    fn authorization_token_response_rejects_an_explicit_reduced_scope() {
        let result = validate_authorization_token_response(
            CloudProvider::GoogleDrive,
            CloudAuthorizationProfile::ReadOnlyV1,
            OAuthTokenResponse {
                access_token: "access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                expires_in: Some(3600),
                token_type: "Bearer".to_string(),
                scope: Some("openid email".to_string()),
            },
        );

        assert!(matches!(result, Err(CloudSourceError::OauthTokenRejected)));
    }

    #[test]
    fn dropbox_token_accepts_provider_granted_extra_scopes_without_widening_authority() {
        let result = validate_authorization_token_response(
            CloudProvider::Dropbox,
            CloudAuthorizationProfile::MarkdownReadWriteV2,
            OAuthTokenResponse {
                access_token: "access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                expires_in: Some(3600),
                token_type: "Bearer".to_string(),
                scope: Some(
                    "account_info.read files.metadata.write files.metadata.read files.content.read files.content.write".to_string(),
                ),
            },
        );

        assert!(result.is_ok());
    }

    #[cfg(feature = "google-drive-connector")]
    #[test]
    fn authorization_token_response_names_a_missing_offline_grant() {
        let result = validate_authorization_token_response(
            CloudProvider::GoogleDrive,
            CloudAuthorizationProfile::ReadOnlyV1,
            OAuthTokenResponse {
                access_token: "access-token".to_string(),
                refresh_token: None,
                expires_in: Some(3600),
                token_type: "Bearer".to_string(),
                scope: Some(catdb_connector_google_drive::GOOGLE_DRIVE_READONLY_SCOPE.to_string()),
            },
        );

        assert!(matches!(
            result,
            Err(CloudSourceError::OauthOfflineGrantMissing)
        ));
    }

    #[test]
    fn drive_root_alias_accepts_the_provider_root_identity_only_for_root() {
        let root = super::drive_selection_from_metadata(
            "root",
            super::DriveFileMetadata {
                id: "provider-root-id".to_string(),
                mime_type: super::GOOGLE_DRIVE_FOLDER_MIME.to_string(),
                resource_key: None,
            },
        )
        .expect("Drive root alias");
        assert_eq!(root.id, "provider-root-id");
        assert!(root.folder);

        assert!(matches!(
            super::drive_selection_from_metadata(
                "root",
                super::DriveFileMetadata {
                    id: "provider-root-id".to_string(),
                    mime_type: "text/markdown".to_string(),
                    resource_key: None,
                },
            ),
            Err(CloudSourceError::ProviderRejected)
        ));

        assert!(matches!(
            super::drive_selection_from_metadata(
                "different-requested-id",
                super::DriveFileMetadata {
                    id: "provider-root-id".to_string(),
                    mime_type: super::GOOGLE_DRIVE_FOLDER_MIME.to_string(),
                    resource_key: None,
                },
            ),
            Err(CloudSourceError::ProviderRejected)
        ));
    }

    #[test]
    fn dropbox_redirect_configuration_distinguishes_missing_from_invalid_values() {
        for missing in [None, Some(""), Some(" \t")] {
            assert_eq!(
                dropbox_redirect_configuration(missing),
                Err(CloudSourceError::ClientConfigurationMissing)
            );
        }
        for invalid in [
            "https://127.0.0.1:53682/oauth/dropbox",
            "http://127.0.0.1/oauth/dropbox",
            "http://127.0.0.1:0/oauth/dropbox",
            "http://127.0.0.1:80/oauth/dropbox",
            "http://127.0.0.1:053682/oauth/dropbox",
            "http://127.0.0.2:53682/oauth/dropbox",
            "http://[::1]:53683/oauth/dropbox",
            "http://192.0.2.1:53682/oauth/dropbox",
            "http://user@127.0.0.1:53682/oauth/dropbox",
            "http://127.0.0.1:53682/oauth/dropbox?source=test",
            "http://127.0.0.1:53682/oauth/dropbox#fragment",
            "http://127.0.0.1:53682/oauth/dropbox/",
            "http://127.0.0.1:53682/oauth/google-drive",
        ] {
            assert_eq!(
                dropbox_redirect_configuration(Some(invalid)),
                Err(CloudSourceError::InvalidConfiguration),
                "{invalid}"
            );
        }
    }

    #[test]
    fn dropbox_redirect_configuration_preserves_the_exact_registered_ipv4_loopback_uri() {
        let configured = "http://127.0.0.1:53682/oauth/dropbox";
        let redirect = dropbox_redirect_configuration(Some(configured)).expect("valid redirect");
        assert_eq!(redirect.redirect_uri(), configured);
        assert_ne!(redirect.socket_addr().port(), 0);
        assert!(redirect.socket_addr().ip().is_loopback());
    }

    #[test]
    fn runtime_oauth_client_ids_use_the_provider_specific_public_shapes() {
        assert_eq!(
            validate_oauth_client_id(
                CloudProvider::GoogleDrive,
                "123456789-example.apps.googleusercontent.com",
            ),
            Ok(())
        );
        assert_eq!(
            validate_oauth_client_id(CloudProvider::Dropbox, "dropbox-public-app-key"),
            Ok(())
        );

        for (provider, invalid) in [
            (CloudProvider::GoogleDrive, "dropbox-public-app-key"),
            (
                CloudProvider::Dropbox,
                "123456789-example.apps.googleusercontent.com",
            ),
            (
                CloudProvider::GoogleDrive,
                "123-no spaces.apps.googleusercontent.com",
            ),
            (CloudProvider::Dropbox, "short"),
        ] {
            assert_eq!(
                validate_oauth_client_id(provider, invalid),
                Err(CloudSourceError::ClientConfigurationMissing),
                "{provider:?}: {invalid}",
            );
        }
    }

    #[tokio::test]
    async fn google_callback_uses_a_random_reserved_port_for_each_attempt() {
        let _callback_lease = OAUTH_CALLBACK_TEST_LOCK.lock().await;
        let (first_listener, first_redirect) = bind_oauth_callback_listener(
            oauth_callback_configuration(CloudProvider::GoogleDrive, None)
                .expect("Google callback configuration"),
        )
        .await
        .expect("first callback listener");
        let (second_listener, second_redirect) = bind_oauth_callback_listener(
            oauth_callback_configuration(CloudProvider::GoogleDrive, None)
                .expect("Google callback configuration"),
        )
        .await
        .expect("second callback listener");

        let first_port = first_listener
            .local_addr()
            .expect("first listener address")
            .port();
        let second_port = second_listener
            .local_addr()
            .expect("second listener address")
            .port();
        assert_ne!(first_port, 0);
        assert_ne!(second_port, 0);
        assert_ne!(
            first_port, second_port,
            "concurrent OAuth attempts must not contend for one fixed port"
        );
        assert_eq!(
            first_redirect,
            format!("http://127.0.0.1:{first_port}/oauth/google-drive")
        );
        assert_eq!(
            second_redirect,
            format!("http://127.0.0.1:{second_port}/oauth/google-drive")
        );
    }

    #[tokio::test]
    async fn dropbox_callback_binds_the_exact_configured_address_and_fails_if_occupied() {
        let _callback_lease = OAUTH_CALLBACK_TEST_LOCK.lock().await;
        let reservation = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("reserve callback port");
        let occupied = reservation.local_addr().expect("reserved address");
        let redirect_uri = format!("http://{occupied}/oauth/dropbox");
        let configuration =
            oauth_callback_configuration(CloudProvider::Dropbox, Some(&redirect_uri))
                .expect("Dropbox callback configuration");
        assert!(matches!(
            bind_oauth_callback_listener(configuration).await,
            Err(CloudSourceError::OauthCallbackUnavailable)
        ));
        drop(reservation);

        let configuration =
            oauth_callback_configuration(CloudProvider::Dropbox, Some(&redirect_uri))
                .expect("Dropbox callback configuration");
        let (listener, bound_redirect) = bind_oauth_callback_listener(configuration)
            .await
            .expect("exact Dropbox callback listener");
        assert_eq!(listener.local_addr().expect("bound address"), occupied);
        assert_eq!(bound_redirect, redirect_uri);
    }

    #[tokio::test]
    async fn oauth_callback_rejects_a_terminated_request_over_16_kib() {
        let _callback_lease = OAUTH_CALLBACK_TEST_LOCK.lock().await;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("callback listener");
        let address = listener.local_addr().expect("callback address");
        let redirect_uri = format!("http://{address}/oauth/google-drive");
        let receiver = receive_oauth_callback(listener, &redirect_uri);
        let sender = async move {
            let mut stream = TcpStream::connect(address)
                .await
                .expect("connect to callback listener");
            let mut request = format!(
                "GET /oauth/google-drive?code=provider-code&state=expected-state HTTP/1.1\r\nX-Padding: {}",
                "a".repeat(MAX_CALLBACK_BYTES)
            )
            .into_bytes();
            request.extend_from_slice(b"\r\n\r\n");
            assert!(request.len() > MAX_CALLBACK_BYTES);
            stream
                .write_all(&request)
                .await
                .expect("send oversized callback");
        };

        let (result, ()) = tokio::join!(receiver, sender);
        assert_eq!(result, Err(CloudSourceError::OauthInvalidCallback));
    }

    #[cfg(all(feature = "google-drive-connector", feature = "dropbox-connector"))]
    #[test]
    fn oauth_authorization_urls_and_token_forms_emit_required_fields_once() {
        for provider in [CloudProvider::GoogleDrive, CloudProvider::Dropbox] {
            let redirect_uri = match provider {
                CloudProvider::GoogleDrive => "http://127.0.0.1:53681/oauth/google-drive",
                CloudProvider::Dropbox => "http://127.0.0.1:53682/oauth/dropbox",
            };
            let attempt =
                OAuthAttempt::new(provider, "a".repeat(64), "expected-state", redirect_uri)
                    .expect("valid OAuth attempt");
            let authorization =
                Url::parse(&attempt.authorization_url("desktop-client").expect("URL"))
                    .expect("parse authorization URL");
            let parameters = authorization.query_pairs().collect::<Vec<_>>();
            for required in [
                "client_id",
                "redirect_uri",
                "response_type",
                "scope",
                "code_challenge",
                "code_challenge_method",
                "state",
            ] {
                assert_eq!(
                    parameters.iter().filter(|(key, _)| key == required).count(),
                    1,
                    "{provider:?} authorization URL field {required}"
                );
            }
            assert_eq!(
                parameters
                    .iter()
                    .filter(|(key, value)| {
                        key == "redirect_uri" && value.as_ref() == redirect_uri
                    })
                    .count(),
                1
            );
            assert!(!parameters.iter().any(|(key, _)| key == "client_secret"));
            match provider {
                CloudProvider::GoogleDrive => {
                    for (key, value) in [("access_type", "offline"), ("prompt", "consent")] {
                        assert_eq!(
                            parameters
                                .iter()
                                .filter(|(candidate_key, candidate_value)| {
                                    candidate_key == key && candidate_value == value
                                })
                                .count(),
                            1,
                            "Google authorization URL field {key}"
                        );
                    }
                    assert!(parameters.iter().any(|(key, value)| {
                        key == "scope"
                            && value == catdb_connector_google_drive::GOOGLE_DRIVE_READONLY_SCOPE
                    }));
                    for unsupported in [
                        "trigger_onepick",
                        "allow_multiple",
                        "allow_folder_selection",
                    ] {
                        assert!(
                            !parameters.iter().any(|(key, _)| key == unsupported),
                            "Google OAuth must not invent the Picker parameter {unsupported}"
                        );
                    }
                }
                CloudProvider::Dropbox => {
                    assert_eq!(
                        parameters
                            .iter()
                            .filter(|(key, value)| {
                                key == "token_access_type" && value == "offline"
                            })
                            .count(),
                        1,
                        "Dropbox authorization URL must request an offline token once"
                    );
                }
            }
        }

        let google_client = oauth_client_configuration_from(
            CloudProvider::GoogleDrive,
            Some("123-desktop.apps.googleusercontent.com"),
            Some("GOCSPX-reference-value"),
        )
        .expect("valid Google desktop client");
        let dropbox_client = oauth_client_configuration_from(
            CloudProvider::Dropbox,
            Some("dropbox-public-app-key"),
            Some("GOCSPX-must-not-route-to-dropbox"),
        )
        .expect("valid Dropbox public client");
        let authorization_form = authorization_code_token_form(
            &google_client,
            "provider-code",
            "pkce-verifier",
            "http://127.0.0.1:53682/oauth/dropbox",
        );
        for required in [
            "client_id",
            "code",
            "code_verifier",
            "grant_type",
            "redirect_uri",
        ] {
            assert_eq!(
                authorization_form
                    .iter()
                    .filter(|(key, _)| *key == required)
                    .count(),
                1,
                "authorization-code token field {required}"
            );
        }
        assert_eq!(
            authorization_form
                .iter()
                .filter(|(key, value)| { *key == "grant_type" && *value == "authorization_code" })
                .count(),
            1
        );
        assert_eq!(
            authorization_form
                .iter()
                .filter(|(key, value)| {
                    *key == "redirect_uri" && *value == "http://127.0.0.1:53682/oauth/dropbox"
                })
                .count(),
            1,
            "the authorization-code exchange must preserve the registered redirect URI"
        );
        assert_eq!(
            authorization_form
                .iter()
                .filter(|(key, value)| {
                    *key == "client_secret" && *value == "GOCSPX-reference-value"
                })
                .count(),
            1,
            "the Google desktop credential must be sent exactly once"
        );
        assert!(!authorization_code_token_form(
            &dropbox_client,
            "provider-code",
            "pkce-verifier",
            "http://127.0.0.1:53682/oauth/dropbox",
        )
        .iter()
        .any(|(key, _)| *key == "client_secret"));

        let refresh_form = refresh_token_form(&google_client, "refresh-token");
        for required in ["client_id", "grant_type", "refresh_token"] {
            assert_eq!(
                refresh_form
                    .iter()
                    .filter(|(key, _)| *key == required)
                    .count(),
                1,
                "refresh token field {required}"
            );
        }
        assert_eq!(
            refresh_form
                .iter()
                .filter(|(key, value)| {
                    *key == "client_secret" && *value == "GOCSPX-reference-value"
                })
                .count(),
            1,
            "the Google desktop credential must be sent on refresh"
        );
        assert_eq!(
            refresh_form
                .iter()
                .filter(|(key, value)| *key == "grant_type" && *value == "refresh_token")
                .count(),
            1
        );
        assert!(!refresh_token_form(&dropbox_client, "refresh-token")
            .iter()
            .any(|(key, _)| *key == "client_secret"));
    }

    #[test]
    fn google_desktop_client_secret_is_validated_without_becoming_runtime_state() {
        assert!(validate_google_oauth_client_secret("GOCSPX-reference-value").is_ok());
        for invalid in ["", "not-google", "GOCSPX-has\ncontrol"] {
            assert_eq!(
                validate_google_oauth_client_secret(invalid),
                Err(CloudSourceError::ClientConfigurationMissing)
            );
        }
        for missing in [
            oauth_client_configuration_from(
                CloudProvider::GoogleDrive,
                None,
                Some("GOCSPX-reference-value"),
            ),
            oauth_client_configuration_from(
                CloudProvider::GoogleDrive,
                Some("123-desktop.apps.googleusercontent.com"),
                None,
            ),
            oauth_client_configuration_from(
                CloudProvider::GoogleDrive,
                Some("123-desktop.apps.googleusercontent.com"),
                Some("not-google"),
            ),
        ] {
            assert_eq!(missing, Err(CloudSourceError::ClientConfigurationMissing));
        }
        assert_eq!(
            oauth_client_configuration_from(
                CloudProvider::Dropbox,
                Some("dropbox-public-app-key"),
                Some("GOCSPX-must-not-route-to-dropbox"),
            )
            .expect("Dropbox needs no client secret")
            .client_secret,
            None
        );
    }

    #[test]
    fn deserialized_credentials_must_satisfy_the_runtime_secret_invariants() {
        let malformed = serde_json::json!({
            "access_token": "",
            "refresh_token": "refresh-token-value",
            "expires_at_unix_seconds": 4_000_000_000_u64,
            "cursor_key": vec![7_u8; 32],
        });
        let secret: SecretBundle =
            serde_json::from_value(malformed).expect("shape is syntactically valid");

        assert_eq!(
            secret.validate(),
            Err(CloudSourceError::InvalidConfiguration)
        );
    }

    #[test]
    fn credential_expiry_is_absolute_and_survives_serialization() {
        let secret = credentials();
        let encoded = serde_json::to_string(&secret).expect("serialize credential");
        assert!(
            !encoded.contains("expires_in_seconds"),
            "a relative lifetime cannot survive an application restart"
        );
        let restored: SecretBundle =
            serde_json::from_str(&encoded).expect("restore serialized credential");
        assert_eq!(
            restored
                .expires_at()
                .expect("absolute expiry")
                .duration_since(std::time::UNIX_EPOCH)
                .expect("after epoch")
                .as_secs(),
            4_000_000_000
        );
    }

    #[test]
    fn persisted_and_debug_forms_never_expose_token_material() {
        let temp = tempdir().expect("temporary directory");
        let registry_path = temp.path().join("library.json");
        let vault = MemoryCredentialVault::default();
        let service = CloudAccountService::new(LibraryStore::new(registry_path.clone()), &vault);
        let secret = credentials();

        assert_eq!(format!("{secret:?}"), "SecretBundle([redacted])");
        service
            .install(CloudProvider::GoogleDrive, google_config(), secret)
            .expect("install connected source");

        let persisted = fs::read_to_string(registry_path).expect("read registry");
        assert!(!persisted.contains("access-token-value"));
        assert!(!persisted.contains("refresh-token-value"));
        let registry: serde_json::Value =
            serde_json::from_str(&persisted).expect("unified Library registry");
        assert_eq!(
            registry["sources"]
                .as_array()
                .expect("Library membership")
                .len(),
            1
        );
        assert_eq!(registry["sources"][0]["kind"], "google-drive");
        assert!(
            !temp.path().join("cloud-sources.json").exists(),
            "cloud membership must not create a parallel source registry"
        );
        assert!(!format!("{:?}", service.snapshot().expect("snapshot")).contains("picked_file_123"));
    }

    #[test]
    fn a_missing_vault_entry_is_reauthorization_not_a_connected_source() {
        let temp = tempdir().expect("temporary directory");
        let vault = MemoryCredentialVault::default();
        let service =
            CloudAccountService::new(LibraryStore::new(temp.path().join("library.json")), &vault);
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("install connected source");
        vault.clear();

        let snapshot = service.snapshot().expect("snapshot");
        let dropbox = snapshot
            .sources
            .iter()
            .find(|source| source.provider == CloudProvider::Dropbox)
            .expect("Dropbox source");
        assert_eq!(dropbox.connection, CloudConnectionState::Reauthorize);
        assert!(!dropbox.enabled, "missing credentials must fail closed");
    }
}
