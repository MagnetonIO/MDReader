//! Native cloud-account ownership for MDReader document sources.
//!
//! Provider files remain authoritative. This module persists only non-secret
//! source membership and keeps OAuth material behind an operating-system
//! credential-vault boundary. Pausing excludes a source from reads without
//! revoking it; disconnecting removes only MDReader-managed state.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(test)]
use std::sync::Mutex;

#[cfg(any(
    test,
    feature = "google-drive-connector",
    feature = "dropbox-connector"
))]
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::file_lock::AdvisoryFileLock;

const CLOUD_REGISTRY_VERSION: u32 = 1;
const MAX_CLOUD_SOURCES: usize = 2;
const MAX_CALLBACK_BYTES: usize = 16 * 1024;
#[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
const MAX_AUTHORIZATION_CODE_BYTES: usize = 4 * 1024;
const MAX_DRIVE_SELECTIONS: usize = 512;
const MAX_TOKEN_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_DRIVE_METADATA_BYTES: usize = 64 * 1024;
const OAUTH_CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const GOOGLE_REDIRECT_URI: &str = "http://127.0.0.1:53681/oauth/google-drive";
const DROPBOX_REDIRECT_URI: &str = "http://127.0.0.1:53682/oauth/dropbox";
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
    ProviderRejected,
    #[cfg(any(
        not(feature = "google-drive-connector"),
        not(feature = "dropbox-connector")
    ))]
    ConnectorUnavailable,
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
            Self::ProviderRejected => "cloud-oauth-provider-rejected",
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
    expires_in_seconds: u64,
    cursor_key: [u8; 32],
}

impl SecretBundle {
    pub(crate) fn new(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        expires_in_seconds: u64,
        cursor_key: [u8; 32],
    ) -> Result<Self, CloudSourceError> {
        let bundle = Self {
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            expires_in_seconds,
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
            || self.expires_in_seconds == 0
            || self.cursor_key.iter().all(|byte| *byte == 0)
        {
            return Err(CloudSourceError::InvalidConfiguration);
        }
        Ok(())
    }
}

impl fmt::Debug for SecretBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBundle([redacted])")
    }
}

pub(crate) trait CredentialVault {
    fn get(&self, reference: &str) -> Result<Option<SecretBundle>, CloudSourceError>;
    fn set(&self, reference: &str, secret: &SecretBundle) -> Result<(), CloudSourceError>;
    fn delete(&self, reference: &str) -> Result<(), CloudSourceError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudSourceRecord {
    id: String,
    provider: CloudProvider,
    label: String,
    enabled: bool,
    credential_reference: String,
    config: CloudSourceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCloudRegistry {
    version: u32,
    sources: Vec<CloudSourceRecord>,
}

#[derive(Debug, Clone)]
struct CloudRegistry {
    sources: Vec<CloudSourceRecord>,
}

impl CloudRegistry {
    fn empty() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    fn load(path: &Path) -> Result<Self, CloudSourceError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::empty());
            }
            Err(_) => return Err(CloudSourceError::PersistFailed),
        };
        let stored: StoredCloudRegistry =
            serde_json::from_slice(&bytes).map_err(|_| CloudSourceError::CorruptRegistry)?;
        if stored.version != CLOUD_REGISTRY_VERSION {
            return Err(CloudSourceError::UnsupportedVersion {
                found: stored.version,
                expected: CLOUD_REGISTRY_VERSION,
            });
        }
        let registry = Self {
            sources: stored.sources,
        };
        registry.validate()?;
        Ok(registry)
    }

    fn validate(&self) -> Result<(), CloudSourceError> {
        if self.sources.len() > MAX_CLOUD_SOURCES {
            return Err(CloudSourceError::CorruptRegistry);
        }
        let mut ids = HashSet::new();
        let mut providers = HashSet::new();
        let mut references = HashSet::new();
        for source in &self.sources {
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
                || !ids.insert(source.id.as_str())
                || !providers.insert(source.provider)
                || !references.insert(source.credential_reference.as_str())
            {
                return Err(CloudSourceError::CorruptRegistry);
            }
        }
        Ok(())
    }

    fn persist(&self, path: &Path) -> Result<(), CloudSourceError> {
        self.validate()?;
        let parent = path.parent().ok_or(CloudSourceError::PersistFailed)?;
        fs::create_dir_all(parent).map_err(|_| CloudSourceError::PersistFailed)?;
        let bytes = serde_json::to_vec_pretty(&StoredCloudRegistry {
            version: CLOUD_REGISTRY_VERSION,
            sources: self.sources.clone(),
        })
        .map_err(|_| CloudSourceError::PersistFailed)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| CloudSourceError::PersistFailed)?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| CloudSourceError::PersistFailed)?;
        temporary
            .persist(path)
            .map(|_| ())
            .map_err(|_| CloudSourceError::PersistFailed)
    }
}

#[derive(Clone)]
pub(crate) struct FileCloudRegistry {
    path: PathBuf,
    #[cfg(test)]
    fail_next_persist: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl FileCloudRegistry {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            #[cfg(test)]
            fail_next_persist: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    fn begin(&self) -> Result<CloudRegistryTransaction<'_>, CloudSourceError> {
        let lock_path = self.path.with_extension("lock");
        let lock =
            AdvisoryFileLock::acquire(&lock_path).map_err(|_| CloudSourceError::PersistFailed)?;
        let registry = CloudRegistry::load(&self.path)?;
        Ok(CloudRegistryTransaction {
            store: self,
            _lock: lock,
            registry,
        })
    }

    #[cfg(test)]
    fn fail_next_persist(&self) {
        self.fail_next_persist
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

struct CloudRegistryTransaction<'a> {
    store: &'a FileCloudRegistry,
    _lock: AdvisoryFileLock,
    registry: CloudRegistry,
}

impl CloudRegistryTransaction<'_> {
    fn commit(self) -> Result<(), CloudSourceError> {
        #[cfg(test)]
        if self
            .store
            .fail_next_persist
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(CloudSourceError::PersistFailed);
        }
        self.registry.persist(&self.store.path)
    }
}

pub(crate) struct CloudAccountService<'a, V> {
    registry: FileCloudRegistry,
    vault: &'a V,
}

impl<'a, V: CredentialVault> CloudAccountService<'a, V> {
    pub(crate) fn new(registry: FileCloudRegistry, vault: &'a V) -> Self {
        Self { registry, vault }
    }

    pub(crate) fn snapshot(&self) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let transaction = self.registry.begin()?;
        self.snapshot_registry(&transaction.registry)
    }

    fn snapshot_registry(
        &self,
        registry: &CloudRegistry,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let mut sources = Vec::with_capacity(MAX_CLOUD_SOURCES);
        for provider in CloudProvider::all() {
            let record = registry
                .sources
                .iter()
                .find(|source| source.provider == provider);
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
        let mut transaction = self.registry.begin()?;
        let existing_index = transaction
            .registry
            .sources
            .iter()
            .position(|source| source.provider == provider);
        let (id, credential_reference) = existing_index.map_or_else(
            || {
                let id = new_source_id();
                let reference = format!("cloud-credential-{id}");
                (id, reference)
            },
            |index| {
                let source = &transaction.registry.sources[index];
                (source.id.clone(), source.credential_reference.clone())
            },
        );
        let previous = self.vault.get(&credential_reference)?;
        self.vault.set(&credential_reference, &secret)?;
        let record = CloudSourceRecord {
            id,
            provider,
            label: provider.label().to_string(),
            enabled: true,
            credential_reference: credential_reference.clone(),
            config,
        };
        if let Some(index) = existing_index {
            transaction.registry.sources[index] = record;
        } else {
            transaction.registry.sources.push(record);
        }
        if let Err(error) = transaction.commit() {
            self.restore_secret(&credential_reference, previous.as_ref())?;
            return Err(error);
        }
        self.snapshot()
    }

    pub(crate) fn set_enabled(
        &self,
        provider: CloudProvider,
        enabled: bool,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let mut transaction = self.registry.begin()?;
        let record = transaction
            .registry
            .sources
            .iter_mut()
            .find(|source| source.provider == provider)
            .ok_or(CloudSourceError::SourceNotFound)?;
        if enabled && self.vault.get(&record.credential_reference)?.is_none() {
            return Err(CloudSourceError::CredentialMissing);
        }
        record.enabled = enabled;
        transaction.commit()?;
        self.snapshot()
    }

    pub(crate) fn disconnect(
        &self,
        provider: CloudProvider,
    ) -> Result<CloudSourcesSnapshot, CloudSourceError> {
        let mut transaction = self.registry.begin()?;
        let Some(index) = transaction
            .registry
            .sources
            .iter()
            .position(|source| source.provider == provider)
        else {
            return self.snapshot_registry(&transaction.registry);
        };
        let reference = transaction.registry.sources[index]
            .credential_reference
            .clone();
        let previous = self.vault.get(&reference)?;
        self.vault.delete(&reference)?;
        transaction.registry.sources.remove(index);
        if let Err(error) = transaction.commit() {
            self.restore_secret(&reference, previous.as_ref())?;
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
            return Err(CloudSourceError::ConnectorUnavailable);
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
            return Err(CloudSourceError::ConnectorUnavailable);
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
            return Err(CloudSourceError::ConnectorUnavailable);
        }
        #[cfg(any(feature = "google-drive-connector", feature = "dropbox-connector"))]
        match &self.pkce {
            #[cfg(feature = "google-drive-connector")]
            OAuthPkce::GoogleDrive(pkce) => Ok(pkce.verifier()),
            #[cfg(feature = "dropbox-connector")]
            OAuthPkce::Dropbox(pkce) => Ok(pkce.verifier()),
        }
    }

    pub(crate) fn verify_callback(
        &self,
        callback: &str,
    ) -> Result<VerifiedAuthorization, CloudSourceError> {
        #[cfg(not(any(feature = "google-drive-connector", feature = "dropbox-connector")))]
        {
            let _ = (&self.pkce, callback);
            return Err(CloudSourceError::ConnectorUnavailable);
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

#[derive(Default)]
pub(crate) struct CloudAccountsState(tokio::sync::Mutex<()>);

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
    operation: impl FnOnce(FileCloudRegistry, KeyringCredentialVault) -> Result<T, CloudSourceError>
        + Send
        + 'static,
) -> Result<T, CloudSourceError> {
    tauri::async_runtime::spawn_blocking(move || {
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|_| CloudSourceError::PersistFailed)?;
        let vault = KeyringCredentialVault::new(&app.config().identifier)?;
        operation(
            FileCloudRegistry::new(app_data.join("cloud-sources.json")),
            vault,
        )
    })
    .await
    .map_err(|_| CloudSourceError::PersistFailed)?
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn list_cloud_sources(
    app: AppHandle,
    state: tauri::State<'_, CloudAccountsState>,
) -> Result<CloudSourcesSnapshot, CloudSourceError> {
    let _operation = state.0.lock().await;
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
    let _operation = state.0.lock().await;
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
    let _operation = state.0.lock().await;
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
    let _operation = state.0.lock().await;
    let client_id = oauth_client_id(provider)?;
    let redirect_uri = oauth_redirect_uri(provider);
    let listener = TcpListener::bind(oauth_callback_address(provider))
        .await
        .map_err(|_| CloudSourceError::OauthCallbackUnavailable)?;
    let (verifier, oauth_state) = new_pkce_material();
    let attempt = OAuthAttempt::new(provider, verifier, oauth_state, redirect_uri)?;
    let authorization_url = attempt.authorization_url(client_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        tauri_plugin_opener::open_url(authorization_url, None::<&str>)
            .map_err(|_| CloudSourceError::OauthTransport)
    })
    .await
    .map_err(|_| CloudSourceError::OauthTransport)??;

    let callback = receive_oauth_callback(listener, redirect_uri).await?;
    let authorization = attempt.verify_callback(&callback)?;
    let token = exchange_authorization_code(provider, client_id, &attempt, &authorization).await?;
    let config = cloud_source_config(provider, &token, &authorization).await?;
    let secret = SecretBundle::new(
        token.access_token.as_str().to_owned(),
        token.refresh_token.as_str().to_owned(),
        token.expires_in_seconds,
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

fn oauth_redirect_uri(provider: CloudProvider) -> &'static str {
    match provider {
        CloudProvider::GoogleDrive => GOOGLE_REDIRECT_URI,
        CloudProvider::Dropbox => DROPBOX_REDIRECT_URI,
    }
}

fn oauth_callback_address(provider: CloudProvider) -> (&'static str, u16) {
    match provider {
        CloudProvider::GoogleDrive => ("127.0.0.1", 53_681),
        CloudProvider::Dropbox => ("127.0.0.1", 53_682),
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
        ("redirect_uri", oauth_redirect_uri(provider)),
    ];
    let response = client
        .post(endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|_| CloudSourceError::OauthTransport)?;
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
        expires_in_seconds,
        [1; 32],
    )?;
    Ok(OAuthToken {
        access_token: Zeroizing::new(response.access_token),
        refresh_token: Zeroizing::new(refresh_token),
        expires_in_seconds,
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
    use std::fs;

    use tempfile::tempdir;

    use super::{
        CloudAccountService, CloudConnectionState, CloudProvider, CloudSourceConfig,
        CloudSourceError, FileCloudRegistry, MemoryCredentialVault, OAuthAttempt, SecretBundle,
    };

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
        SecretBundle::new("access-token-value", "refresh-token-value", 3_600, [7; 32])
            .expect("valid test credentials")
    }

    #[test]
    fn pausing_keeps_credentials_and_disconnect_removes_only_catdb_managed_state() {
        let temp = tempdir().expect("temporary directory");
        let provider_file = temp.path().join("provider-owned.md");
        fs::write(&provider_file, "provider remains authoritative").expect("provider fixture");
        let vault = MemoryCredentialVault::default();
        let service = CloudAccountService::new(
            FileCloudRegistry::new(temp.path().join("cloud-sources.json")),
            &vault,
        );

        service
            .install(CloudProvider::GoogleDrive, google_config(), credentials())
            .expect("install connected source");
        service
            .set_enabled(CloudProvider::GoogleDrive, false)
            .expect("pause source");

        let paused = service.snapshot().expect("paused snapshot");
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
    fn disconnect_restores_the_credential_when_registry_publication_fails() {
        let temp = tempdir().expect("temporary directory");
        let vault = MemoryCredentialVault::default();
        let registry = FileCloudRegistry::new(temp.path().join("cloud-sources.json"));
        let service = CloudAccountService::new(registry.clone(), &vault);
        service
            .install(
                CloudProvider::Dropbox,
                CloudSourceConfig::Dropbox,
                credentials(),
            )
            .expect("install connected source");
        registry.fail_next_persist();

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

    #[test]
    fn deserialized_credentials_must_satisfy_the_runtime_secret_invariants() {
        let malformed = serde_json::json!({
            "access_token": "",
            "refresh_token": "refresh-token-value",
            "expires_in_seconds": 3_600,
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
    fn persisted_and_debug_forms_never_expose_token_material() {
        let temp = tempdir().expect("temporary directory");
        let registry_path = temp.path().join("cloud-sources.json");
        let vault = MemoryCredentialVault::default();
        let service =
            CloudAccountService::new(FileCloudRegistry::new(registry_path.clone()), &vault);
        let secret = credentials();

        assert_eq!(format!("{secret:?}"), "SecretBundle([redacted])");
        service
            .install(CloudProvider::GoogleDrive, google_config(), secret)
            .expect("install connected source");

        let persisted = fs::read_to_string(registry_path).expect("read registry");
        assert!(!persisted.contains("access-token-value"));
        assert!(!persisted.contains("refresh-token-value"));
        assert!(!format!("{:?}", service.snapshot().expect("snapshot")).contains("picked_file_123"));
    }

    #[test]
    fn a_missing_vault_entry_is_reauthorization_not_a_connected_source() {
        let temp = tempdir().expect("temporary directory");
        let vault = MemoryCredentialVault::default();
        let service = CloudAccountService::new(
            FileCloudRegistry::new(temp.path().join("cloud-sources.json")),
            &vault,
        );
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
