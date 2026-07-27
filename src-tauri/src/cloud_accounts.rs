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
use std::path::{Path, PathBuf};
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use std::sync::Arc;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::library::LibraryStore;

const CLOUD_REGISTRY_VERSION: u32 = 1;
const MAX_CLOUD_SOURCES: usize = 2;
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
const CLOUD_CATALOG_REVISION_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_63_61_74_5f_01_00);
const CLOUD_WORKSPACE_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_77_73_70_5f_01_00);
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const CLOUD_CATALOG_RESOLVER_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x6d_64_72_5f_63_6c_6f_75_64_5f_72_73_6c_5f_01_00);
const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const DROPBOX_TOKEN_ENDPOINT: &str = "https://api.dropboxapi.com/oauth2/token";
const GOOGLE_DRIVE_FILES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/files/";
const GOOGLE_DRIVE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudProvider {
    GoogleDrive,
    Dropbox,
}

impl CloudProvider {
    fn label(self) -> &'static str {
        match self {
            Self::GoogleDrive => "Google Drive",
            Self::Dropbox => "Dropbox",
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

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudObservation {
    Observed,
    ObservedNothing,
    Unobservable,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct CloudFieldProvenance {
    pub field: String,
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
    OauthTransport,
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
    QuerySchemaDrift {
        source_id: String,
        expected: String,
        observed: String,
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
            Self::OauthTransport => "cloud-oauth-transport-failed",
            Self::OauthRateLimited { .. } => "cloud-oauth-rate-limited",
            Self::ProviderRejected => "cloud-oauth-provider-rejected",
            Self::SourcePaused => "cloud-source-paused",
            Self::QueryInvalid { .. } => "cloud-query-invalid",
            Self::QueryCancelled => "cloud-query-cancelled",
            Self::QueryRetired => "cloud-query-retired",
            Self::QueryCatalogUnavailable => "cloud-query-catalog-unavailable",
            Self::QueryCatalogInvalid => "cloud-query-catalog-invalid",
            Self::QueryDeadlineExceeded => "cloud-query-deadline-exceeded",
            Self::QueryTimeout { .. } => "cloud-query-source-timeout",
            Self::QueryAuthentication { .. } => "cloud-query-authentication-failed",
            Self::QueryRateLimited { .. } => "cloud-query-rate-limited",
            Self::QueryIncomplete { .. } => "cloud-query-incomplete",
            Self::QueryTransport { .. } => "cloud-query-transport-failed",
            Self::QuerySourceCancelled { .. } => "cloud-query-source-cancelled",
            Self::QueryLimitExceeded => "cloud-query-limit-exceeded",
            Self::QueryNotFound { .. } => "cloud-query-not-found",
            Self::QueryRefused { .. } => "cloud-query-refused",
            Self::QueryReadFailed { .. } => "cloud-query-read-failed",
            Self::QuerySchemaDrift { .. } => "cloud-query-schema-drift",
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudSourceRecord {
    id: String,
    provider: CloudProvider,
    label: String,
    enabled: bool,
    credential_reference: String,
    config: CloudSourceConfig,
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
        let valid_reference = !source.credential_reference.is_empty()
            && source.credential_reference.len() <= 256
            && !source.credential_reference.chars().any(char::is_control);
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

impl<'a, V: CredentialVault> CloudAccountService<'a, V> {
    pub(crate) fn new(registry: LibraryStore, vault: &'a V) -> Self {
        Self { registry, vault }
    }

    pub(crate) fn snapshot(&self) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        self.registry
            .read_cloud(|sources| self.snapshot_records(sources))
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
        config.validate()?;
        if config.provider() != provider {
            return Err(CloudSourceError::InvalidConfiguration);
        }
        let mut rollback = None;
        let update = self.registry.update_cloud(|sources| {
            let existing_index = sources
                .iter()
                .position(|source| source.provider == provider);
            let (id, credential_reference) = existing_index.map_or_else(
                || {
                    let id = new_source_id();
                    let reference = format!("cloud-credential-{id}");
                    (id, reference)
                },
                |index| {
                    let source = &sources[index];
                    (source.id.clone(), source.credential_reference.clone())
                },
            );
            let previous = self.vault.get(&credential_reference)?;
            self.vault.set(&credential_reference, &secret)?;
            rollback = Some((credential_reference.clone(), previous));
            let record = CloudSourceRecord {
                id,
                provider,
                label: provider.label().to_string(),
                enabled: true,
                credential_reference,
                config,
                contract: Some(crate::library::LibrarySourceContract::cloud(provider)),
            };
            if let Some(index) = existing_index {
                sources[index] = record;
            } else {
                sources.push(record);
            }
            Ok(())
        });
        if let Err(error) = update {
            if let Some((reference, previous)) = rollback.as_ref() {
                self.restore_secret(reference, previous.as_ref())?;
            }
            return Err(error);
        }
        self.snapshot()
    }

    pub(crate) fn set_enabled(
        &self,
        provider: CloudProvider,
        enabled: bool,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
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
        self.snapshot()
    }

    pub(crate) fn disconnect(
        &self,
        provider: CloudProvider,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let mut rollback = None;
        let update = self.registry.update_cloud(|sources| {
            let Some(index) = sources
                .iter()
                .position(|source| source.provider == provider)
            else {
                return Ok(());
            };
            let reference = sources[index].credential_reference.clone();
            let previous = self.vault.get(&reference)?;
            self.vault.delete(&reference)?;
            rollback = Some((reference, previous));
            sources.remove(index);
            Ok(())
        });
        if let Err(error) = update {
            if let Some((reference, previous)) = rollback.as_ref() {
                self.restore_secret(reference, previous.as_ref())?;
            }
            return Err(error);
        }
        self.snapshot()
    }

    fn restore_secret(
        &self,
        reference: &str,
        previous: Option<&SecretBundle>,
    ) -> Result<(), CloudSourceError> {
        let result = match previous {
            Some(secret) => self.vault.set(reference, secret),
            None => self.vault.delete(reference),
        };
        result.map_err(|_| CloudSourceError::RollbackFailed)
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
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = (provider, verifier.into(), state.into(), redirect_uri.into());
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
                .authorization_url(client_id)
                .map_err(|_| CloudSourceError::InvalidConfiguration),
            #[cfg(feature = "dropbox-connector")]
            OAuthPkce::Dropbox(pkce) => pkce
                .authorization_url(client_id)
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
                parameters
                    .get("picked_file_ids")
                    .map_or_else(Vec::new, |value| {
                        value
                            .split(',')
                            .map(str::to_string)
                            .collect::<Vec<String>>()
                    })
            } else {
                Vec::new()
            };
            if self.provider == CloudProvider::GoogleDrive
                && (picked_file_ids.is_empty() || picked_file_ids.len() > MAX_DRIVE_SELECTIONS)
            {
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

#[derive(Debug)]
struct CloudQueryHandle {
    request_id: String,
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Debug, Clone, Copy)]
struct RequestedCatalogPins {
    workspace_id: uuid::Uuid,
    catalog_revision_id: Option<uuid::Uuid>,
    schema_version_id: Option<uuid::Uuid>,
}

pub(crate) struct CloudAccountsState {
    operations: tokio::sync::Mutex<()>,
    catalog: tokio::sync::Mutex<Option<CloudCatalogAuthority>>,
    catalog_location: Mutex<Option<(PathBuf, uuid::Uuid)>>,
    active_queries: Mutex<HashMap<String, ActiveCloudQuery>>,
    next_query_serial: AtomicU64,
}

impl Default for CloudAccountsState {
    fn default() -> Self {
        Self {
            operations: tokio::sync::Mutex::new(()),
            catalog: tokio::sync::Mutex::new(None),
            catalog_location: Mutex::new(None),
            active_queries: Mutex::new(HashMap::new()),
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
            Some(_) => Ok(()),
            None => {
                *configured = Some(location);
                Ok(())
            }
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
}

async fn blocking_cloud_operation<T: Send + 'static>(
    app: AppHandle,
    operation: impl FnOnce(LibraryStore, KeyringCredentialVault) -> Result<T, CloudSourceError>
        + Send
        + 'static,
) -> Result<T, CloudSourceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|_| CloudSourceError::PersistFailed)?;
        let vault = KeyringCredentialVault::new(&app.config().identifier)?;
        operation(LibraryStore::new(app_data.join("library.json")), vault)
    })
    .await
    .map_err(|_| CloudSourceError::PersistFailed)?
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
#[derive(Clone)]
struct CloudCredentialResolver {
    provider: CloudProvider,
    reference: String,
    vault: Arc<dyn CredentialVault>,
    admitted: Option<Arc<Mutex<SecretBundle>>>,
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
        }
    }

    fn from_admitted(
        provider: CloudProvider,
        reference: String,
        credential: SecretBundle,
        vault: Arc<dyn CredentialVault>,
    ) -> Self {
        Self {
            provider,
            reference,
            vault,
            admitted: Some(Arc::new(Mutex::new(credential))),
        }
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
        tauri::async_runtime::spawn_blocking(move || vault.set(&reference, &stored))
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
        let refreshed = exchange_refresh_token(self.provider, refresh_credential).await?;
        secret.access_token = refreshed.access_token.to_string();
        secret.expires_at_unix_seconds = refreshed.expires_at_unix_seconds;
        secret.validate()?;
        self.store(secret.clone()).await?;
        Ok(secret)
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
    vault: V,
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
fn admit_cloud_read<V: CredentialVault>(
    registry: LibraryStore,
    vault: V,
    provider: CloudProvider,
) -> Result<AdmittedCloudRead<V>, CloudSourceError> {
    let (record, credential) =
        CloudAccountService::new(registry, &vault).admitted_record(provider)?;
    let read_authority = CloudReadAuthority::from_record(&record)?;
    Ok(AdmittedCloudRead {
        record,
        credential,
        read_authority,
        vault,
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn admit_cloud_read_before_catalog<V, A, C, CFut>(
    admission: A,
    catalog: C,
) -> Result<(AdmittedCloudRead<V>, CloudCatalogAuthority), CloudSourceError>
where
    A: std::future::Future<Output = Result<AdmittedCloudRead<V>, CloudSourceError>>,
    C: FnOnce() -> CFut,
    CFut: std::future::Future<Output = Result<CloudCatalogAuthority, CloudSourceError>>,
{
    let admitted = admission.await?;
    let catalog = catalog().await?;
    Ok((admitted, catalog))
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
        let query = state.begin_query(&request_id)?;
        let result = async {
            let _operation = tokio::select! {
                _ = query.cancellation.cancelled() => {
                    return Err(CloudSourceError::QueryCancelled);
                }
                operation = state.operations.lock() => operation,
            };
            let admission = blocking_cloud_operation(app.clone(), move |registry, vault| {
                admit_cloud_read(registry, vault, provider)
            });
            let (admitted, catalog) =
                admit_cloud_read_before_catalog(admission, || ensure_cloud_catalog(&app, state))
                    .await?;
            let AdmittedCloudRead {
                record,
                credential,
                read_authority,
                vault,
            } = admitted;
            let resolver = Arc::new(CloudCredentialResolver::from_admitted(
                provider,
                record.credential_reference.clone(),
                credential,
                Arc::new(vault),
            ));

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
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let spec = catdb_connector_google_drive::PinnedDriveSourceSpec::new(
                        config,
                        &catalog.schema_version,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let source =
                        catdb_connector_google_drive::DriveSource::open_pinned(spec, resolver)
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
                        query.cancellation.clone(),
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
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let spec = catdb_connector_dropbox::PinnedDropboxSourceSpec::new(
                        config,
                        &catalog.schema_version,
                    )
                    .map_err(|error| query_invalid(error.to_string()))?;
                    let source =
                        catdb_connector_dropbox::DropboxSource::open_pinned(spec, resolver)
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
                        query.cancellation.clone(),
                    )
                    .await
                }
                #[cfg(not(feature = "dropbox-connector"))]
                CloudSourceConfig::Dropbox => Err(CloudSourceError::ConnectorUnavailable),
            }?;
            Ok(response)
        }
        .await;
        state.finish_query(&query)?;
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
    use catdb_provenance::{FieldObservation, FieldProvenance, ObservationKind};

    let source_report = response
        .sources
        .first()
        .ok_or(CloudSourceError::QueryInvalidResponse)?;
    let source_version_id = parse_pin(&response.version_pins.source_version)?;
    let mapping_version_id = parse_pin(&response.version_pins.mapping_version)?;
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
                            CloudObservation::Observed => {
                                let value = fields
                                    .get(&field.field)
                                    .cloned()
                                    .ok_or(CloudSourceError::QueryInvalidResponse)?;
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
                            CloudObservation::ObservedNothing => {
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
                            CloudObservation::Unobservable => Ok(FieldObservation::not_asked(
                                source_version_id,
                                provider.record_key,
                                mapping_version_id,
                                "the provider mapping does not expose this Document field",
                            )),
                        })
                        .collect::<Result<Vec<_>, CloudSourceError>>()?;
                    let selected = considered.iter().position(|observation| {
                        matches!(
                            observation.state().kind(),
                            ObservationKind::ObservedValue | ObservationKind::ObservedNull
                        )
                    });
                    let evidence = FieldProvenance::new(field.field.clone(), considered, selected)
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
        },
        rows,
        sources: vec![SourceExecution {
            source_id: uuid::Uuid::new_v5(
                &CLOUD_SOURCE_VERSION_NAMESPACE,
                source_report.source_id.as_bytes(),
            ),
            source_version_id,
            outcome: SourceOutcome::Ran,
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
            |selection| match (selection.folder, selection.resource_key) {
                (true, Some(key)) => {
                    catdb_connector_google_drive::DriveSelection::folder_with_resource_key(
                        selection.id,
                        key,
                    )
                }
                (true, None) => Ok(catdb_connector_google_drive::DriveSelection::folder(
                    selection.id,
                )),
                (false, Some(key)) => {
                    catdb_connector_google_drive::DriveSelection::file_with_resource_key(
                        selection.id,
                        key,
                    )
                }
                (false, None) => Ok(catdb_connector_google_drive::DriveSelection::file(
                    selection.id,
                )),
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
    let schema_version_id =
        catdb_engine::operational_schema_version_id(&mdreader_domain::library::document_schema());
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
struct CloudExecutionSpecs {
    mapping: serde_json::Value,
    physical_schema: serde_json::Value,
    source: serde_json::Value,
    policy: serde_json::Value,
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
        SchemaContent, SchemaVersion, SourceVersion,
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
    mapping_version
        .verify()
        .and_then(|()| source_version.verify())
        .and_then(|()| policy_version.verify())
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
        if stored_canonical != canonical_version
            || stored_physical != physical_version
            || stored_mapping != mapping_version
            || stored_source != source_version
            || stored_policy != policy_version
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
                    Ok(CloudFieldProvenance {
                        field: field.clone(),
                        providers: vec![CloudConsideredProvider {
                            provider,
                            source_id: row_source.clone(),
                            record_key: row.key.clone(),
                            mapping_version: authority.pins.mapping_version.clone(),
                            physical_attribute,
                            observation: match (
                                provider_field(&authority.mapping, field),
                                observed.is_some(),
                            ) {
                                (Some(_), true) => CloudObservation::Observed,
                                (Some(_), false) => CloudObservation::ObservedNothing,
                                (None, _) => CloudObservation::Unobservable,
                            },
                            observed_at_ms: provider_field(&authority.mapping, field)
                                .map(|_| freshness.observed_at_ms),
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
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    let _operation = state.operations.lock().await;
    blocking_cloud_operation(app, |registry, vault| {
        CloudAccountService::new(registry, &vault).snapshot()
    })
    .await
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn set_cloud_source_enabled(
    app: AppHandle,
    provider: CloudProvider,
    enabled: bool,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    let _operation = state.operations.lock().await;
    blocking_cloud_operation(app, move |registry, vault| {
        CloudAccountService::new(registry, &vault).set_enabled(provider, enabled)
    })
    .await
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn disconnect_cloud_source(
    app: AppHandle,
    provider: CloudProvider,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    let _operation = state.operations.lock().await;
    blocking_cloud_operation(app, move |registry, vault| {
        CloudAccountService::new(registry, &vault).disconnect(provider)
    })
    .await
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn connect_cloud_source(
    app: AppHandle,
    provider: CloudProvider,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    let _operation = state.operations.lock().await;
    let client_id = oauth_client_id(provider)?;
    let (listener, redirect_uri) = bind_oauth_callback_listener(provider).await?;
    let (verifier, oauth_state) = new_pkce_material();
    let attempt = OAuthAttempt::new(provider, verifier, oauth_state, redirect_uri.clone())?;
    let authorization_url = attempt.authorization_url(client_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        tauri_plugin_opener::open_url(authorization_url, None::<&str>)
            .map_err(|_| CloudSourceError::OauthTransport)
    })
    .await
    .map_err(|_| CloudSourceError::OauthTransport)??;

    let callback = receive_oauth_callback(listener, &redirect_uri).await?;
    let authorization = attempt.verify_callback(&callback)?;
    let token = exchange_authorization_code(provider, client_id, &attempt, &authorization).await?;
    let config = cloud_source_config(provider, &token, &authorization).await?;
    let secret = SecretBundle::new(
        token.access_token.as_str().to_owned(),
        token.refresh_token.as_str().to_owned(),
        expires_at_unix_seconds(token.expires_in_seconds)?,
        new_cursor_key(),
    )?;
    blocking_cloud_operation(app, move |registry, vault| {
        CloudAccountService::new(registry, &vault).install(provider, config, secret)
    })
    .await
}

fn oauth_client_id(provider: CloudProvider) -> Result<&'static str, CloudSourceError> {
    let client_id = match provider {
        CloudProvider::GoogleDrive => option_env!("MDREADER_GOOGLE_DRIVE_CLIENT_ID"),
        CloudProvider::Dropbox => option_env!("MDREADER_DROPBOX_CLIENT_ID"),
    }
    .ok_or(CloudSourceError::ClientConfigurationMissing)?;
    if client_id.is_empty() || client_id.len() > 512 || client_id.chars().any(char::is_control) {
        return Err(CloudSourceError::ClientConfigurationMissing);
    }
    Ok(client_id)
}

fn expires_at_unix_seconds(expires_in_seconds: u64) -> Result<u64, CloudSourceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CloudSourceError::InvalidConfiguration)?
        .as_secs()
        .checked_add(expires_in_seconds)
        .ok_or(CloudSourceError::InvalidConfiguration)
}

async fn bind_oauth_callback_listener(
    provider: CloudProvider,
) -> Result<(TcpListener, String), CloudSourceError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|_| CloudSourceError::OauthCallbackUnavailable)?;
    let port = listener
        .local_addr()
        .map_err(|_| CloudSourceError::OauthCallbackUnavailable)?
        .port();
    let path = match provider {
        CloudProvider::GoogleDrive => "/oauth/google-drive",
        CloudProvider::Dropbox => "/oauth/dropbox",
    };
    Ok((listener, format!("http://127.0.0.1:{port}{path}")))
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
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > MAX_CALLBACK_BYTES {
            return Err(CloudSourceError::OauthInvalidCallback);
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
    let callback = format!(
        "{}://{}:{}{}",
        redirect.scheme(),
        redirect
            .host_str()
            .ok_or(CloudSourceError::OauthInvalidCallback)?,
        redirect
            .port()
            .ok_or(CloudSourceError::OauthInvalidCallback)?,
        target
    );
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

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
struct RefreshedOAuthToken {
    access_token: Zeroizing<String>,
    expires_at_unix_seconds: u64,
}

async fn exchange_authorization_code(
    provider: CloudProvider,
    client_id: &str,
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
    let form = [
        ("client_id", client_id),
        ("code", authorization.code()),
        ("code_verifier", attempt.verifier()?),
        ("grant_type", "authorization_code"),
        ("redirect_uri", attempt.redirect_uri()),
    ];
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
    let bytes = bounded_provider_response(response, MAX_TOKEN_RESPONSE_BYTES).await?;
    let response: OAuthTokenResponse =
        serde_json::from_slice(&bytes).map_err(|_| CloudSourceError::ProviderRejected)?;
    let refresh_token = response
        .refresh_token
        .filter(|token| !token.is_empty())
        .ok_or(CloudSourceError::ProviderRejected)?;
    let expires_in_seconds = response
        .expires_in
        .filter(|seconds| *seconds > 0 && *seconds <= 365 * 24 * 60 * 60)
        .ok_or(CloudSourceError::ProviderRejected)?;
    if !response.token_type.eq_ignore_ascii_case("bearer")
        || !oauth_scope_is_sufficient(provider, response.scope.as_deref())
    {
        return Err(CloudSourceError::ProviderRejected);
    }
    SecretBundle::new(
        response.access_token.clone(),
        refresh_token.clone(),
        expires_at_unix_seconds(expires_in_seconds)?,
        [1; 32],
    )?;
    Ok(OAuthToken {
        access_token: Zeroizing::new(response.access_token),
        refresh_token: Zeroizing::new(refresh_token),
        expires_in_seconds,
    })
}

#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
async fn exchange_refresh_token(
    provider: CloudProvider,
    refresh_token: &str,
) -> Result<RefreshedOAuthToken, CloudSourceError> {
    let client_id = oauth_client_id(provider)?;
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
    let response = client
        .post(endpoint)
        .form(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(CloudSourceError::OauthRateLimited {
            retry_after_ms: retry_after_ms(response.headers()),
        });
    }
    let bytes = bounded_provider_response(response, MAX_TOKEN_RESPONSE_BYTES).await?;
    let response: OAuthTokenResponse =
        serde_json::from_slice(&bytes).map_err(|_| CloudSourceError::ProviderRejected)?;
    let expires_in_seconds = response
        .expires_in
        .filter(|seconds| *seconds > 0 && *seconds <= 365 * 24 * 60 * 60)
        .ok_or(CloudSourceError::ProviderRejected)?;
    if response.access_token.is_empty()
        || response.access_token.len() > 16 * 1024
        || response.access_token.chars().any(char::is_control)
        || !response.token_type.eq_ignore_ascii_case("bearer")
        || response
            .scope
            .as_deref()
            .is_some_and(|scope| !oauth_scope_is_sufficient(provider, Some(scope)))
    {
        return Err(CloudSourceError::ProviderRejected);
    }
    Ok(RefreshedOAuthToken {
        access_token: Zeroizing::new(response.access_token),
        expires_at_unix_seconds: expires_at_unix_seconds(expires_in_seconds)?,
    })
}

fn oauth_scope_is_sufficient(provider: CloudProvider, scope: Option<&str>) -> bool {
    let Some(scope) = scope else {
        return false;
    };
    let scopes = scope.split_whitespace().collect::<HashSet<_>>();
    #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
    let _ = &scopes;
    match provider {
        CloudProvider::GoogleDrive => {
            #[cfg(feature = "google-drive-connector")]
            {
                scopes.contains(catdb_connector_google_drive::GOOGLE_DRIVE_FILE_SCOPE)
            }
            #[cfg(not(feature = "google-drive-connector"))]
            {
                false
            }
        }
        CloudProvider::Dropbox => {
            #[cfg(feature = "dropbox-connector")]
            {
                scopes.contains(catdb_connector_dropbox::DROPBOX_METADATA_READ_SCOPE)
                    && scopes.contains(catdb_connector_dropbox::DROPBOX_CONTENT_READ_SCOPE)
            }
            #[cfg(not(feature = "dropbox-connector"))]
            {
                false
            }
        }
    }
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
        if metadata.id != *picked_id {
            return Err(CloudSourceError::ProviderRejected);
        }
        selections.push(DriveItemSelection {
            id: metadata.id,
            resource_key: metadata.resource_key,
            folder: metadata.mime_type == GOOGLE_DRIVE_FOLDER_MIME,
        });
    }
    Ok(selections)
}

fn validate_drive_config(selections: &[DriveItemSelection]) -> Result<(), CloudSourceError> {
    #[cfg(feature = "google-drive-connector")]
    {
        let mut connector_selections = Vec::with_capacity(selections.len());
        for selection in selections {
            let connector_selection = match (selection.folder, &selection.resource_key) {
                (true, Some(key)) => {
                    catdb_connector_google_drive::DriveSelection::folder_with_resource_key(
                        selection.id.clone(),
                        key.clone(),
                    )
                }
                (true, None) => Ok(catdb_connector_google_drive::DriveSelection::folder(
                    selection.id.clone(),
                )),
                (false, Some(key)) => {
                    catdb_connector_google_drive::DriveSelection::file_with_resource_key(
                        selection.id.clone(),
                        key.clone(),
                    )
                }
                (false, None) => Ok(catdb_connector_google_drive::DriveSelection::file(
                    selection.id.clone(),
                )),
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
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    use catdb_connectors::{
        DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor, SourceError,
        SourceFreshness, SourceId, SourceModel, SourcePage, SourcePlan, SourceRow,
    };
    use tempfile::tempdir;

    use super::{
        bind_oauth_callback_listener, execute_cloud_query, CloudAccountService, CloudAccountsState,
        CloudCompleteness, CloudConnectionState, CloudCredentialResolver,
        CloudDocumentQueryResponse, CloudExecutionMode, CloudProvider, CloudSourceConfig,
        CloudSourceError, CloudSourceOutcome, CredentialVault, MemoryCredentialVault, OAuthAttempt,
        SecretBundle,
    };
    use crate::library::{LibraryRegistry, LibraryStore};

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
            let result = super::admit_cloud_read_before_catalog(
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
    async fn unmapped_unobservable_and_observed_null_are_distinct_provenance_states() {
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
                    "rev": "1",
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
            vec!["title".to_string(), "mediaType".to_string()],
            catdb_connectors::CancellationToken::new(),
        )
        .await
        .expect("unmapped fields remain queryable as absence evidence");
        let shared = super::shared_entity_response(cloud).expect("shared entity response");
        let title = &shared.rows[0].provenance["title"].considered()[0];
        let media_type = &shared.rows[0].provenance["mediaType"].considered()[0];

        assert_eq!(
            title.state().kind(),
            catdb_provenance::ObservationKind::ObservedNull
        );
        assert_eq!(
            media_type.state().kind(),
            catdb_provenance::ObservationKind::NotAsked
        );
        assert_eq!(media_type.physical_attribute(), None);
        assert_eq!(media_type.observed_at_ms(), None);
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
            .disconnect(CloudProvider::GoogleDrive)
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
        let service = CloudAccountService::new(LibraryStore::new(registry_path.clone()), &vault);

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
        assert_eq!(persisted["version"], serde_json::json!(6));
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
        assert_eq!(persisted["version"], serde_json::json!(6));
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
            service.disconnect(CloudProvider::Dropbox),
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
        let attempt = OAuthAttempt::new(
            CloudProvider::Dropbox,
            "a".repeat(64),
            "expected-state",
            "http://127.0.0.1:53682/oauth/dropbox",
        )
        .expect("valid attempt");

        let result = attempt.verify_callback(
            "http://127.0.0.1:53682/oauth/dropbox?code=provider-code&state=wrong-state",
        );

        assert!(matches!(result, Err(CloudSourceError::OauthStateMismatch)));
    }

    #[test]
    fn oauth_callback_requires_the_exact_redirect_and_unique_parameters() {
        let attempt = OAuthAttempt::new(
            CloudProvider::GoogleDrive,
            "a".repeat(64),
            "expected-state",
            "http://127.0.0.1:53681/oauth/google-drive",
        )
        .expect("valid attempt");

        for callback in [
            "http://127.0.0.1:53681/oauth/other?code=provider-code&state=expected-state&picked_file_ids=file_1",
            "http://127.0.0.1:53681/oauth/google-drive?code=one&code=two&state=expected-state&picked_file_ids=file_1",
            "http://127.0.0.1:53681/oauth/google-drive?code=provider-code&state=expected-state",
        ] {
            assert!(matches!(
                attempt.verify_callback(callback),
                Err(CloudSourceError::OauthInvalidCallback)
            ));
        }
    }

    #[tokio::test]
    async fn oauth_callback_uses_the_port_reserved_for_this_attempt() {
        let (first_listener, first_redirect) =
            bind_oauth_callback_listener(CloudProvider::GoogleDrive)
                .await
                .expect("first callback listener");
        let (second_listener, second_redirect) =
            bind_oauth_callback_listener(CloudProvider::GoogleDrive)
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
