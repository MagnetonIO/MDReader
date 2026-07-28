//! Native, bearer-authenticated transport for the mobile Enterprise source.
//!
//! The webview names an admitted connection and supplies only a generated
//! CatDB request body. The native layer resolves the origin and bearer token,
//! restricts calls to the session/entity-query contract, and bounds the
//! response before returning JSON to the renderer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use catdb_api_types::{
    ControlPlaneSnapshot, EntityQueryRequest, EntityQueryResponse, SessionResponse,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use url::Url;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use zeroize::Zeroizing;
use zeroize::{Zeroize, ZeroizeOnDrop};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 3 * 1024 * 1024;
const MAX_CONNECTED_SOURCES: usize = 128;
const MAX_LEGACY_CONNECTIONS: usize = 16;
const MAX_LEGACY_REGISTRY_BYTES: usize = 64 * 1024;
const MAX_OPERATION_ID_BYTES: usize = 256;
const MAX_CONNECTED_LIBRARY_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONNECTED_LIBRARY_PAGES: usize = 128;
const LEGACY_REGISTRY_VERSION: u32 = 1;
const REQUEST_DEADLINE: Duration = Duration::from_secs(15);
const LIBRARY_LIFECYCLE_OPERATION_ID: &str = "library-lifecycle";

#[cfg(feature = "e2e-build")]
const E2E_CONNECTION_ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
#[cfg(feature = "e2e-build")]
const E2E_CONNECTION_LABEL: &str = "Isolated Enterprise fixture";
#[cfg(feature = "e2e-build")]
const E2E_ORIGIN_ENV: &str = "MDREADER_E2E_CONNECTED_ORIGIN";
#[cfg(feature = "e2e-build")]
const E2E_BEARER: &str = "mdreader-e2e-bearer";
#[cfg(feature = "e2e-build")]
const E2E_CREDENTIAL_REFERENCE: &str = "mdreader-e2e-connected-catdb";
#[cfg(feature = "e2e-build")]
const E2E_DEPLOYMENT_ID: uuid::Uuid = uuid::Uuid::from_u128(0x201);
#[cfg(feature = "e2e-build")]
const E2E_WORKSPACE_ID: uuid::Uuid = uuid::Uuid::from_u128(0x202);
#[cfg(feature = "e2e-build")]
const E2E_CATALOG_REVISION_ID: uuid::Uuid = uuid::Uuid::from_u128(0x204);
#[cfg(feature = "e2e-build")]
const E2E_SCHEMA_VERSION_ID: uuid::Uuid = uuid::Uuid::from_u128(0x205);
#[cfg(feature = "e2e-build")]
const E2E_SOURCE_ID: uuid::Uuid = uuid::Uuid::from_u128(0x206);
#[cfg(feature = "e2e-build")]
const E2E_SOURCE_VERSION_ID: uuid::Uuid = uuid::Uuid::from_u128(0x207);
#[cfg(feature = "e2e-build")]
const E2E_EXCLUDED_SOURCE_ID: uuid::Uuid = uuid::Uuid::from_u128(0x213);
#[cfg(feature = "e2e-build")]
const E2E_EXCLUDED_SOURCE_VERSION_ID: uuid::Uuid = uuid::Uuid::from_u128(0x214);

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct MobileEnterpriseConnection {
    pub connection_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MobileEnterpriseOperation {
    Session,
    EntityQuery,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct MobileEnterpriseResponse {
    pub status: u16,
    pub body_json: String,
}

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConnectedCatdbKind {
    ConnectedCatdb,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct ConnectedCatdbSourceView {
    pub source_id: uuid::Uuid,
    pub source_version_id: uuid::Uuid,
    pub runtime_bound: bool,
    pub connector_id: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum ConnectedCatdbState {
    Disconnected {
        kind: ConnectedCatdbKind,
    },
    Connected {
        kind: ConnectedCatdbKind,
        connection_id: String,
        label: String,
        server_origin: String,
        workspace_id: uuid::Uuid,
        catalog_revision_id: uuid::Uuid,
        sources: Vec<ConnectedCatdbSourceView>,
        contract: crate::library::LibrarySourceContract,
    },
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum MobileEnterpriseError {
    ConnectionNotFound,
    CredentialUnavailable,
    InvalidConfiguration,
    InvalidRequest,
    TransportUnavailable,
    TimedOut,
    ResponseTooLarge,
    InvalidResponse,
    PermissionDenied,
    ConnectionIdentityChanged,
    AlreadyConfigured,
    NoSourcesSelected,
    PersistFailed,
    RollbackFailed,
    Cancelled,
}

impl fmt::Display for MobileEnterpriseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConnectionNotFound => "mobile-enterprise-connection-not-found",
            Self::CredentialUnavailable => "mobile-enterprise-credential-unavailable",
            Self::InvalidConfiguration => "mobile-enterprise-invalid-configuration",
            Self::InvalidRequest => "mobile-enterprise-invalid-request",
            Self::TransportUnavailable => "mobile-enterprise-transport-unavailable",
            Self::TimedOut => "mobile-enterprise-timeout",
            Self::ResponseTooLarge => "mobile-enterprise-response-too-large",
            Self::InvalidResponse => "mobile-enterprise-invalid-response",
            Self::PermissionDenied => "mobile-enterprise-permission-denied",
            Self::ConnectionIdentityChanged => "mobile-enterprise-connection-identity-changed",
            Self::AlreadyConfigured => "mobile-enterprise-already-configured",
            Self::NoSourcesSelected => "mobile-enterprise-no-sources-selected",
            Self::PersistFailed => "mobile-enterprise-persist-failed",
            Self::RollbackFailed => "mobile-enterprise-rollback-failed",
            Self::Cancelled => "mobile-enterprise-cancelled",
        })
    }
}

impl std::error::Error for MobileEnterpriseError {}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionRecord {
    id: String,
    label: String,
    server_origin: String,
    credential_reference: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyConnectionRegistry {
    version: u32,
    active_connection_id: Option<String>,
    connections: Vec<ConnectionRecord>,
}

impl fmt::Debug for ConnectionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionRecord")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("server_origin", &self.server_origin)
            .field("credential_reference", &"[native-secure-reference]")
            .finish()
    }
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct BearerToken(String);

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken([redacted])")
    }
}

trait BearerVault: Send + Sync {
    fn get(&self, reference: &str) -> Result<Option<BearerToken>, MobileEnterpriseError>;
    fn set(&self, reference: &str, token: &BearerToken) -> Result<(), MobileEnterpriseError>;
    fn delete(&self, reference: &str) -> Result<(), MobileEnterpriseError>;
}

#[derive(Clone)]
struct KeyringBearerVault {
    service: String,
}

impl KeyringBearerVault {
    fn new(identifier: &str) -> Result<Self, MobileEnterpriseError> {
        if !valid_identifier(identifier, 200) {
            return Err(MobileEnterpriseError::CredentialUnavailable);
        }
        Ok(Self {
            service: format!("{identifier}.enterprise-credentials"),
        })
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn entry(&self, reference: &str) -> Result<keyring::Entry, MobileEnterpriseError> {
        keyring::Entry::new(&self.service, reference)
            .map_err(|_| MobileEnterpriseError::CredentialUnavailable)
    }
}

impl BearerVault for KeyringBearerVault {
    fn get(&self, reference: &str) -> Result<Option<BearerToken>, MobileEnterpriseError> {
        if !valid_identifier(reference, 256) {
            return Err(MobileEnterpriseError::CredentialUnavailable);
        }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let token = match self.entry(reference)?.get_password() {
                Ok(token) => Zeroizing::new(token),
                Err(keyring::Error::NoEntry) => return Ok(None),
                Err(_) => return Err(MobileEnterpriseError::CredentialUnavailable),
            };
            validate_bearer(token.as_str())?;
            Ok(Some(BearerToken(token.as_str().to_owned())))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = reference;
            Err(MobileEnterpriseError::CredentialUnavailable)
        }
    }

    fn set(&self, reference: &str, token: &BearerToken) -> Result<(), MobileEnterpriseError> {
        if !valid_identifier(reference, 256) {
            return Err(MobileEnterpriseError::CredentialUnavailable);
        }
        validate_bearer(&token.0)?;
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.entry(reference)?
                .set_password(&token.0)
                .map_err(|_| MobileEnterpriseError::CredentialUnavailable)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (reference, token);
            Err(MobileEnterpriseError::CredentialUnavailable)
        }
    }

    fn delete(&self, reference: &str) -> Result<(), MobileEnterpriseError> {
        if !valid_identifier(reference, 256) {
            return Err(MobileEnterpriseError::CredentialUnavailable);
        }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            match self.entry(reference)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(MobileEnterpriseError::CredentialUnavailable),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = reference;
            Err(MobileEnterpriseError::CredentialUnavailable)
        }
    }
}

#[derive(Debug)]
struct ActiveMobileEnterpriseOperation {
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
    phase: Arc<AtomicU8>,
}

#[derive(Debug)]
struct MobileEnterpriseOperationHandle {
    operation_id: String,
    serial: u64,
    lifecycle: bool,
    connection_generation: u64,
    cancellation: catdb_connectors::CancellationToken,
    phase: Arc<AtomicU8>,
}

const MOBILE_OPERATION_PENDING: u8 = 0;
const MOBILE_OPERATION_COMMITTING: u8 = 1;
const MOBILE_OPERATION_COMPLETE: u8 = 2;

#[derive(Clone)]
struct MobileEnterpriseCommitAuthority {
    lifecycle_operation: Arc<Mutex<Option<ActiveMobileEnterpriseOperation>>>,
    serial: u64,
    cancellation: catdb_connectors::CancellationToken,
    phase: Arc<AtomicU8>,
}

impl MobileEnterpriseCommitAuthority {
    fn begin_commit(&self) -> Result<(), MobileEnterpriseError> {
        let lifecycle = self
            .lifecycle_operation
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        match lifecycle.as_ref() {
            Some(entry)
                if entry.serial == self.serial
                    && !self.cancellation.is_cancelled()
                    && entry
                        .phase
                        .compare_exchange(
                            MOBILE_OPERATION_PENDING,
                            MOBILE_OPERATION_COMMITTING,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        )
                        .is_ok() =>
            {
                Ok(())
            }
            _ => Err(MobileEnterpriseError::Cancelled),
        }
    }

    fn finish_commit(&self) {
        self.phase
            .store(MOBILE_OPERATION_COMPLETE, Ordering::SeqCst);
        if let Ok(mut lifecycle) = self.lifecycle_operation.lock() {
            if lifecycle
                .as_ref()
                .is_some_and(|entry| entry.serial == self.serial)
            {
                lifecycle.take();
            }
        }
    }
}

struct MobileEnterpriseOperationOwner<'a> {
    state: &'a MobileEnterpriseState,
    handle: MobileEnterpriseOperationHandle,
    settled: bool,
}

impl<'a> MobileEnterpriseOperationOwner<'a> {
    fn begin(
        state: &'a MobileEnterpriseState,
        operation_id: &str,
    ) -> Result<Self, MobileEnterpriseError> {
        Ok(Self {
            state,
            handle: state.begin_operation(operation_id)?,
            settled: false,
        })
    }

    fn begin_lifecycle(state: &'a MobileEnterpriseState) -> Result<Self, MobileEnterpriseError> {
        Ok(Self {
            state,
            handle: state.begin_lifecycle_operation()?,
            settled: false,
        })
    }

    fn cancellation(&self) -> catdb_connectors::CancellationToken {
        self.handle.cancellation.clone()
    }

    fn commit_authority(&self) -> MobileEnterpriseCommitAuthority {
        MobileEnterpriseCommitAuthority {
            lifecycle_operation: Arc::clone(&self.state.lifecycle_operation),
            serial: self.handle.serial,
            cancellation: self.handle.cancellation.clone(),
            phase: Arc::clone(&self.handle.phase),
        }
    }

    fn finish_committed(mut self) {
        self.settled = true;
    }

    fn finish(mut self) -> Result<(), MobileEnterpriseError> {
        let result = self.state.finish_operation(&self.handle);
        self.settled = true;
        result
    }
}

impl Drop for MobileEnterpriseOperationOwner<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if self.handle.phase.load(Ordering::SeqCst) == MOBILE_OPERATION_PENDING {
            self.handle.cancellation.cancel();
        }
        let _ = self.state.finish_operation(&self.handle);
        self.settled = true;
    }
}

pub(crate) struct MobileEnterpriseState {
    lifecycle_commits: Arc<Mutex<()>>,
    active_operations: Arc<Mutex<HashMap<String, ActiveMobileEnterpriseOperation>>>,
    lifecycle_operation: Arc<Mutex<Option<ActiveMobileEnterpriseOperation>>>,
    connection_generation: AtomicU64,
    next_operation_serial: AtomicU64,
}

impl Default for MobileEnterpriseState {
    fn default() -> Self {
        Self {
            lifecycle_commits: Arc::new(Mutex::new(())),
            active_operations: Arc::new(Mutex::new(HashMap::new())),
            lifecycle_operation: Arc::new(Mutex::new(None)),
            connection_generation: AtomicU64::new(1),
            next_operation_serial: AtomicU64::new(1),
        }
    }
}

impl MobileEnterpriseState {
    fn begin_operation(
        &self,
        operation_id: &str,
    ) -> Result<MobileEnterpriseOperationHandle, MobileEnterpriseError> {
        if !valid_identifier(operation_id, MAX_OPERATION_ID_BYTES) {
            return Err(MobileEnterpriseError::InvalidRequest);
        }
        let serial = self.next_operation_serial.fetch_add(1, Ordering::SeqCst);
        let cancellation = catdb_connectors::CancellationToken::new();
        let phase = Arc::new(AtomicU8::new(MOBILE_OPERATION_PENDING));
        let mut active = self
            .active_operations
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        let lifecycle = self
            .lifecycle_operation
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        if lifecycle.is_some() {
            return Err(MobileEnterpriseError::Cancelled);
        }
        let connection_generation = self.connection_generation.load(Ordering::SeqCst);
        if let Some(retired) = active.insert(
            operation_id.to_string(),
            ActiveMobileEnterpriseOperation {
                serial,
                cancellation: cancellation.clone(),
                phase: Arc::clone(&phase),
            },
        ) {
            if retired.phase.load(Ordering::SeqCst) == MOBILE_OPERATION_PENDING {
                retired.cancellation.cancel();
            }
        }
        Ok(MobileEnterpriseOperationHandle {
            operation_id: operation_id.to_string(),
            serial,
            lifecycle: false,
            connection_generation,
            cancellation,
            phase,
        })
    }

    fn begin_lifecycle_operation(
        &self,
    ) -> Result<MobileEnterpriseOperationHandle, MobileEnterpriseError> {
        let serial = self.next_operation_serial.fetch_add(1, Ordering::SeqCst);
        let cancellation = catdb_connectors::CancellationToken::new();
        let phase = Arc::new(AtomicU8::new(MOBILE_OPERATION_PENDING));
        let mut active = self
            .active_operations
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        let mut lifecycle = self
            .lifecycle_operation
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        for (_, operation) in active.drain() {
            operation.cancellation.cancel();
        }
        if let Some(retired) = lifecycle.replace(ActiveMobileEnterpriseOperation {
            serial,
            cancellation: cancellation.clone(),
            phase: Arc::clone(&phase),
        }) {
            if retired.phase.load(Ordering::SeqCst) == MOBILE_OPERATION_PENDING {
                retired.cancellation.cancel();
            }
        }
        let connection_generation = self
            .connection_generation
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(1)
            })
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?
            + 1;
        Ok(MobileEnterpriseOperationHandle {
            operation_id: LIBRARY_LIFECYCLE_OPERATION_ID.to_string(),
            serial,
            lifecycle: true,
            connection_generation,
            cancellation,
            phase,
        })
    }

    fn finish_operation(
        &self,
        operation: &MobileEnterpriseOperationHandle,
    ) -> Result<(), MobileEnterpriseError> {
        if operation.lifecycle {
            let mut lifecycle = self
                .lifecycle_operation
                .lock()
                .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
            return match lifecycle.as_ref() {
                Some(entry) if entry.serial == operation.serial => {
                    lifecycle.take();
                    if operation.cancellation.is_cancelled() {
                        Err(MobileEnterpriseError::Cancelled)
                    } else {
                        Ok(())
                    }
                }
                _ => Err(MobileEnterpriseError::Cancelled),
            };
        }
        let mut active = self
            .active_operations
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        let lifecycle = self
            .lifecycle_operation
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        if lifecycle.is_some()
            || self.connection_generation.load(Ordering::SeqCst) != operation.connection_generation
        {
            if active
                .get(&operation.operation_id)
                .is_some_and(|entry| entry.serial == operation.serial)
            {
                active.remove(&operation.operation_id);
            }
            operation.cancellation.cancel();
            return Err(MobileEnterpriseError::Cancelled);
        }
        match active.get(&operation.operation_id) {
            Some(entry) if entry.serial == operation.serial => {
                active.remove(&operation.operation_id);
                if operation.cancellation.is_cancelled() {
                    Err(MobileEnterpriseError::Cancelled)
                } else {
                    Ok(())
                }
            }
            _ => Err(MobileEnterpriseError::Cancelled),
        }
    }

    fn cancel_operation(&self, operation_id: &str) -> bool {
        let Ok(mut active) = self.active_operations.lock() else {
            return false;
        };
        let Some(operation) = active.remove(operation_id) else {
            return false;
        };
        operation.cancellation.cancel();
        true
    }

    #[cfg(test)]
    fn cancel_all_operations(&self) -> Result<(), MobileEnterpriseError> {
        let mut active = self
            .active_operations
            .lock()
            .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
        for (_, operation) in active.drain() {
            operation.cancellation.cancel();
        }
        Ok(())
    }
}

fn valid_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.chars().any(char::is_control)
        && value.trim() == value
}

fn validate_bearer(value: &str) -> Result<(), MobileEnterpriseError> {
    if valid_identifier(value, 16 * 1024) {
        Ok(())
    } else {
        Err(MobileEnterpriseError::CredentialUnavailable)
    }
}

fn exact_origin(value: &str) -> Result<Url, MobileEnterpriseError> {
    let url = Url::parse(value).map_err(|_| MobileEnterpriseError::InvalidConfiguration)?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    Ok(url)
}

fn validate_record(record: &ConnectionRecord) -> Result<(), MobileEnterpriseError> {
    if !valid_identifier(&record.id, 200)
        || !valid_identifier(&record.label, 200)
        || !valid_identifier(&record.credential_reference, 256)
    {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    exact_origin(&record.server_origin)?;
    Ok(())
}

fn legacy_registry_path(app: &AppHandle) -> Result<PathBuf, MobileEnterpriseError> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("mobile-enterprise.json"))
        .map_err(|_| MobileEnterpriseError::InvalidConfiguration)
}

fn retired_legacy_registry_path(path: &Path) -> Result<PathBuf, MobileEnterpriseError> {
    path.parent()
        .map(|parent| parent.join("mobile-enterprise.json.migrated"))
        .ok_or(MobileEnterpriseError::PersistFailed)
}

fn legacy_migration_lock_path(path: &Path) -> Result<PathBuf, MobileEnterpriseError> {
    path.parent()
        .map(|parent| parent.join("mobile-enterprise-migration.lock"))
        .ok_or(MobileEnterpriseError::PersistFailed)
}

fn with_legacy_migration_lock<T>(
    legacy_path: &Path,
    operation: impl FnOnce() -> Result<T, MobileEnterpriseError>,
) -> Result<T, MobileEnterpriseError> {
    let _lease =
        crate::file_lock::AdvisoryFileLock::acquire(&legacy_migration_lock_path(legacy_path)?)
            .map_err(|_| MobileEnterpriseError::PersistFailed)?;
    operation()
}

fn read_legacy_active_record(
    path: &Path,
) -> Result<Option<ConnectionRecord>, MobileEnterpriseError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(MobileEnterpriseError::InvalidConfiguration),
    };
    if bytes.len() > MAX_LEGACY_REGISTRY_BYTES {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    let registry: LegacyConnectionRegistry =
        serde_json::from_slice(&bytes).map_err(|_| MobileEnterpriseError::InvalidConfiguration)?;
    if registry.version != LEGACY_REGISTRY_VERSION
        || registry.connections.len() > MAX_LEGACY_CONNECTIONS
    {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    let mut ids = std::collections::HashSet::new();
    for connection in &registry.connections {
        validate_record(connection)?;
        if !ids.insert(connection.id.as_str()) {
            return Err(MobileEnterpriseError::InvalidConfiguration);
        }
    }
    let Some(active) = registry.active_connection_id else {
        return Ok(None);
    };
    let mut matches = registry
        .connections
        .into_iter()
        .filter(|connection| connection.id == active);
    let record = matches
        .next()
        .ok_or(MobileEnterpriseError::InvalidConfiguration)?;
    if matches.next().is_some() {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    Ok(Some(record))
}

fn map_library_error(error: crate::library::LibraryError) -> MobileEnterpriseError {
    match error {
        crate::library::LibraryError::DuplicateRoot => MobileEnterpriseError::AlreadyConfigured,
        crate::library::LibraryError::SourceNotFound => MobileEnterpriseError::ConnectionNotFound,
        crate::library::LibraryError::SourceUnavailable => MobileEnterpriseError::InvalidRequest,
        crate::library::LibraryError::CorruptRegistry
        | crate::library::LibraryError::UnsupportedVersion { .. } => {
            MobileEnterpriseError::InvalidConfiguration
        }
        _ => MobileEnterpriseError::PersistFailed,
    }
}

fn library_store(app: &AppHandle) -> Result<crate::library::LibraryStore, MobileEnterpriseError> {
    let (path, _) = crate::library::registry_path(app).map_err(map_library_error)?;
    Ok(crate::library::LibraryStore::new(path))
}

impl From<&crate::library::ConnectedCatdbSourceRecord> for ConnectionRecord {
    fn from(source: &crate::library::ConnectedCatdbSourceRecord) -> Self {
        Self {
            id: source.id.clone(),
            label: source.label.clone(),
            server_origin: source.server_origin.clone(),
            credential_reference: source.credential_reference.clone(),
        }
    }
}

fn record_for(
    store: &crate::library::LibraryStore,
    connection_id: &str,
) -> Result<crate::library::ConnectedCatdbSourceRecord, MobileEnterpriseError> {
    if !valid_identifier(connection_id, 200) {
        return Err(MobileEnterpriseError::ConnectionNotFound);
    }
    store
        .read_connected_catdb()
        .map_err(map_library_error)?
        .filter(|source| source.id == connection_id)
        .ok_or(MobileEnterpriseError::ConnectionNotFound)
}

fn disconnected_state() -> ConnectedCatdbState {
    ConnectedCatdbState::Disconnected {
        kind: ConnectedCatdbKind::ConnectedCatdb,
    }
}

fn connected_state(source: &crate::library::ConnectedCatdbSourceRecord) -> ConnectedCatdbState {
    let selected = source
        .selected_source_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    ConnectedCatdbState::Connected {
        kind: ConnectedCatdbKind::ConnectedCatdb,
        connection_id: source.id.clone(),
        label: source.label.clone(),
        server_origin: source.server_origin.clone(),
        workspace_id: source.workspace_id,
        catalog_revision_id: source.catalog_revision_id,
        sources: source
            .permitted_sources
            .iter()
            .map(|descriptor| ConnectedCatdbSourceView {
                source_id: descriptor.source_id,
                source_version_id: descriptor.source_version_id,
                runtime_bound: descriptor.runtime_bound,
                connector_id: descriptor.connector_id.clone(),
                enabled: selected.contains(&descriptor.source_id),
            })
            .collect(),
        contract: source.contract.clone(),
    }
}

#[cfg(feature = "e2e-build")]
fn e2e_record() -> Result<ConnectionRecord, MobileEnterpriseError> {
    let origin =
        std::env::var(E2E_ORIGIN_ENV).map_err(|_| MobileEnterpriseError::InvalidConfiguration)?;
    let origin = exact_origin(&origin)?.to_string();
    Ok(ConnectionRecord {
        id: E2E_CONNECTION_ID.to_string(),
        label: E2E_CONNECTION_LABEL.to_string(),
        server_origin: origin,
        credential_reference: "e2e-compiled-isolated-token".to_string(),
    })
}

#[cfg(feature = "e2e-build")]
fn e2e_connected_source_record(
) -> Result<crate::library::ConnectedCatdbSourceRecord, MobileEnterpriseError> {
    let origin = e2e_record()?.server_origin;
    let mut source = crate::library::ConnectedCatdbSourceRecord::new(
        E2E_CONNECTION_LABEL.to_string(),
        origin,
        E2E_CREDENTIAL_REFERENCE.to_string(),
        crate::library::ConnectedCatdbAuthority {
            deployment_id: E2E_DEPLOYMENT_ID,
            workspace_id: E2E_WORKSPACE_ID,
            catalog_revision_id: E2E_CATALOG_REVISION_ID,
            default_schema_version_id: E2E_SCHEMA_VERSION_ID,
        },
        vec![
            crate::library::ConnectedCatdbSourceDescriptor {
                source_id: E2E_SOURCE_ID,
                source_version_id: E2E_SOURCE_VERSION_ID,
                runtime_bound: true,
                connector_id: Some("postgres-e2e".to_string()),
            },
            crate::library::ConnectedCatdbSourceDescriptor {
                source_id: E2E_EXCLUDED_SOURCE_ID,
                source_version_id: E2E_EXCLUDED_SOURCE_VERSION_ID,
                runtime_bound: true,
                connector_id: Some("snowflake-e2e-rate-limited".to_string()),
            },
        ],
    );
    source.id = E2E_CONNECTION_ID.to_string();
    source.selected_source_ids = vec![E2E_SOURCE_ID, E2E_EXCLUDED_SOURCE_ID];
    Ok(source)
}

#[cfg(feature = "e2e-build")]
pub(crate) fn seed_e2e_connected_membership(app: &AppHandle) -> Result<(), MobileEnterpriseError> {
    let store = library_store(app)?;
    let vault = KeyringBearerVault::new(&app.config().identifier)?;
    let source = e2e_connected_source_record()?;
    vault.set(
        &source.credential_reference,
        &BearerToken(E2E_BEARER.to_string()),
    )?;
    if let Some(current) = store.read_connected_catdb().map_err(map_library_error)? {
        store
            .remove_connected_catdb(&current.id)
            .map_err(map_library_error)?;
    }
    store.add_connected_catdb(source).map_err(map_library_error)
}

async fn execute_http(
    record: &ConnectionRecord,
    token: &BearerToken,
    method: reqwest::Method,
    path: &str,
    body: Option<String>,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    validate_record(record)?;
    validate_bearer(&token.0)?;
    let endpoint = exact_origin(&record.server_origin)?
        .join(path)
        .map_err(|_| MobileEnterpriseError::InvalidConfiguration)?;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_DEADLINE)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| MobileEnterpriseError::TransportUnavailable)?;
    let mut request = client
        .request(method, endpoint)
        .bearer_auth(token.0.as_str())
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(body) = body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            MobileEnterpriseError::TimedOut
        } else {
            MobileEnterpriseError::TransportUnavailable
        }
    })?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(MobileEnterpriseError::ResponseTooLarge);
    }
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| MobileEnterpriseError::InvalidResponse)?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(MobileEnterpriseError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let body_json = String::from_utf8(bytes).map_err(|_| MobileEnterpriseError::InvalidResponse)?;
    serde_json::from_str::<serde_json::Value>(&body_json)
        .map_err(|_| MobileEnterpriseError::InvalidResponse)?;
    Ok(MobileEnterpriseResponse { status, body_json })
}

async fn execute_request(
    record: ConnectionRecord,
    token: BearerToken,
    operation: MobileEnterpriseOperation,
    body_json: Option<String>,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    let (path, method, body) = match (operation, body_json) {
        (MobileEnterpriseOperation::Session, None) => ("/v1/session", reqwest::Method::GET, None),
        (MobileEnterpriseOperation::EntityQuery, Some(body))
            if !body.is_empty() && body.len() <= MAX_REQUEST_BYTES =>
        {
            let parsed: serde_json::Value =
                serde_json::from_str(&body).map_err(|_| MobileEnterpriseError::InvalidRequest)?;
            let object = parsed
                .as_object()
                .ok_or(MobileEnterpriseError::InvalidRequest)?;
            if object
                .get("source_ids")
                .is_some_and(|source_ids| !source_ids.is_null())
            {
                return Err(MobileEnterpriseError::InvalidRequest);
            }
            ("/v1/entity/query", reqwest::Method::POST, Some(body))
        }
        _ => return Err(MobileEnterpriseError::InvalidRequest),
    };
    execute_http(&record, &token, method, path, body).await
}

async fn discover_connected_catdb(
    server_origin: Url,
    token: &BearerToken,
) -> Result<crate::library::ConnectedCatdbSourceRecord, MobileEnterpriseError> {
    let label = server_origin
        .host_str()
        .filter(|host| valid_identifier(host, 200))
        .ok_or(MobileEnterpriseError::InvalidConfiguration)?
        .to_string();
    let record = ConnectionRecord {
        id: "connected-catdb-enrollment".to_string(),
        label: label.clone(),
        server_origin: server_origin.to_string(),
        credential_reference: "pending-native-credential".to_string(),
    };
    let session_response =
        execute_http(&record, token, reqwest::Method::GET, "/v1/session", None).await?;
    if matches!(session_response.status, 401 | 403) {
        return Err(MobileEnterpriseError::PermissionDenied);
    }
    if session_response.status != 200 {
        return Err(MobileEnterpriseError::InvalidResponse);
    }
    let session: SessionResponse = serde_json::from_str(&session_response.body_json)
        .map_err(|_| MobileEnterpriseError::InvalidResponse)?;
    if session.deployment_id.is_nil()
        || session.workspace_ids.len() != 1
        || session.workspace_ids[0].is_nil()
        || !session.scopes.iter().any(|scope| scope == "query")
        || !session.scopes.iter().any(|scope| scope == "catalog_read")
    {
        return Err(MobileEnterpriseError::PermissionDenied);
    }
    let workspace_id = session.workspace_ids[0];
    let catalog_response = execute_http(
        &record,
        token,
        reqwest::Method::GET,
        &format!("/v1/control/workspaces/{workspace_id}/catalog"),
        None,
    )
    .await?;
    if matches!(catalog_response.status, 401 | 403) {
        return Err(MobileEnterpriseError::PermissionDenied);
    }
    if catalog_response.status != 200 {
        return Err(MobileEnterpriseError::InvalidResponse);
    }
    let catalog: ControlPlaneSnapshot = serde_json::from_str(&catalog_response.body_json)
        .map_err(|_| MobileEnterpriseError::InvalidResponse)?;
    if catalog.workspace_id != workspace_id
        || catalog.resolver_id.is_nil()
        || catalog.revision.revision_id.is_nil()
        || catalog.default_schema_version_id.is_nil()
        || !catalog
            .schemas
            .iter()
            .any(|schema| schema.schema_version_id == catalog.default_schema_version_id)
        || catalog.sources.len() > MAX_CONNECTED_SOURCES
    {
        return Err(MobileEnterpriseError::InvalidResponse);
    }
    let mut source_ids = std::collections::HashSet::new();
    let mut source_version_ids = std::collections::HashSet::new();
    let permitted_sources = catalog
        .sources
        .into_iter()
        .map(|source| {
            if source.source_id.is_nil()
                || source.source_version_id.is_nil()
                || !source_ids.insert(source.source_id)
                || !source_version_ids.insert(source.source_version_id)
                || source
                    .connector_id
                    .as_deref()
                    .is_some_and(|connector| !valid_identifier(connector, 200))
            {
                return Err(MobileEnterpriseError::InvalidResponse);
            }
            Ok(crate::library::ConnectedCatdbSourceDescriptor {
                source_id: source.source_id,
                source_version_id: source.source_version_id,
                runtime_bound: source.runtime_bound,
                connector_id: source.connector_id,
            })
        })
        .collect::<Result<Vec<_>, MobileEnterpriseError>>()?;
    Ok(crate::library::ConnectedCatdbSourceRecord::new(
        label,
        server_origin.to_string(),
        format!("connected-catdb-{}", uuid::Uuid::new_v4()),
        crate::library::ConnectedCatdbAuthority {
            deployment_id: session.deployment_id,
            workspace_id,
            catalog_revision_id: catalog.revision.revision_id,
            default_schema_version_id: catalog.default_schema_version_id,
        },
        permitted_sources,
    ))
}

fn publish_legacy_migration(
    store: &crate::library::LibraryStore,
    legacy_path: &Path,
    source: crate::library::ConnectedCatdbSourceRecord,
) -> Result<crate::library::ConnectedCatdbSourceRecord, MobileEnterpriseError> {
    publish_legacy_migration_with_rename(store, legacy_path, source, |from, to| {
        std::fs::rename(from, to)
    })
}

fn legacy_matches_source(
    record: &ConnectionRecord,
    source: &crate::library::ConnectedCatdbSourceRecord,
) -> bool {
    let same_origin = exact_origin(&record.server_origin)
        .map(|origin| origin.to_string() == source.server_origin)
        .unwrap_or(false);
    record.label == source.label
        && same_origin
        && record.credential_reference == source.credential_reference
}

fn publish_legacy_migration_with_rename(
    store: &crate::library::LibraryStore,
    legacy_path: &Path,
    source: crate::library::ConnectedCatdbSourceRecord,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<crate::library::ConnectedCatdbSourceRecord, MobileEnterpriseError> {
    with_legacy_migration_lock(legacy_path, || {
        publish_legacy_migration_transaction_with_rename(store, legacy_path, source, rename)
    })
}

fn publish_legacy_migration_transaction_with_rename(
    store: &crate::library::LibraryStore,
    legacy_path: &Path,
    source: crate::library::ConnectedCatdbSourceRecord,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<crate::library::ConnectedCatdbSourceRecord, MobileEnterpriseError> {
    if let Some(current) = store.read_connected_catdb().map_err(map_library_error)? {
        retire_matching_legacy_after_publication(legacy_path, &current)?;
        return Ok(current);
    }
    if !read_legacy_active_record(legacy_path)?
        .as_ref()
        .is_some_and(|record| legacy_matches_source(record, &source))
    {
        return Err(MobileEnterpriseError::PersistFailed);
    }
    let retired_path = retired_legacy_registry_path(legacy_path)?;
    match std::fs::symlink_metadata(&retired_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(MobileEnterpriseError::PersistFailed),
    }
    store
        .add_connected_catdb(source.clone())
        .map_err(map_library_error)?;
    if rename(legacy_path, &retired_path).is_err() {
        match read_legacy_active_record(legacy_path) {
            Ok(Some(_)) => {
                if store.remove_connected_catdb(&source.id).is_err() {
                    return Err(MobileEnterpriseError::RollbackFailed);
                }
                return Err(MobileEnterpriseError::PersistFailed);
            }
            Ok(None)
                if read_legacy_active_record(&retired_path)
                    .ok()
                    .flatten()
                    .as_ref()
                    .is_some_and(|record| legacy_matches_source(record, &source)) =>
            {
                return Ok(source);
            }
            Ok(None) | Err(_) => return Err(MobileEnterpriseError::PersistFailed),
        }
    }
    Ok(source)
}

fn retire_matching_legacy_after_publication(
    legacy_path: &Path,
    source: &crate::library::ConnectedCatdbSourceRecord,
) -> Result<(), MobileEnterpriseError> {
    let Ok(Some(record)) = read_legacy_active_record(legacy_path) else {
        return Ok(());
    };
    if !legacy_matches_source(&record, source) {
        return Ok(());
    }
    let retired_path = retired_legacy_registry_path(legacy_path)?;
    match std::fs::symlink_metadata(&retired_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(MobileEnterpriseError::PersistFailed),
    }
    std::fs::rename(legacy_path, retired_path).map_err(|_| MobileEnterpriseError::PersistFailed)
}

async fn migrate_legacy_connection(
    store: crate::library::LibraryStore,
    legacy_path: PathBuf,
    vault: Arc<dyn BearerVault>,
) -> Result<Option<crate::library::ConnectedCatdbSourceRecord>, MobileEnterpriseError> {
    let current_store = store.clone();
    let read_path = legacy_path.clone();
    let read_vault = vault.clone();
    let legacy = tokio::task::spawn_blocking(move || {
        with_legacy_migration_lock(&read_path, || {
            if let Some(source) = current_store
                .read_connected_catdb()
                .map_err(map_library_error)?
            {
                retire_matching_legacy_after_publication(&read_path, &source)?;
                return Ok(LegacyMigrationDecision::Current(source));
            }
            let Some(record) = read_legacy_active_record(&read_path)? else {
                return Ok(LegacyMigrationDecision::Absent);
            };
            let token = read_vault
                .get(&record.credential_reference)?
                .ok_or(MobileEnterpriseError::CredentialUnavailable)?;
            Ok(LegacyMigrationDecision::Pending(record, token))
        })
    })
    .await
    .map_err(|_| MobileEnterpriseError::PersistFailed)??;
    let (record, token) = match legacy {
        LegacyMigrationDecision::Current(source) => return Ok(Some(source)),
        LegacyMigrationDecision::Pending(record, token) => (record, token),
        LegacyMigrationDecision::Absent => return Ok(None),
    };

    let mut source = discover_connected_catdb(exact_origin(&record.server_origin)?, &token).await?;
    source.label = record.label;
    source.credential_reference = record.credential_reference;
    tokio::task::spawn_blocking(move || publish_legacy_migration(&store, &legacy_path, source))
        .await
        .map_err(|_| MobileEnterpriseError::PersistFailed)?
        .map(Some)
}

async fn connected_source_or_migrate(
    app: &AppHandle,
) -> Result<Option<crate::library::ConnectedCatdbSourceRecord>, MobileEnterpriseError> {
    let store = library_store(app)?;
    let legacy_path = legacy_registry_path(app)?;
    let vault: Arc<dyn BearerVault> = Arc::new(KeyringBearerVault::new(&app.config().identifier)?);
    migrate_legacy_connection(store, legacy_path, vault).await
}

enum LegacyMigrationDecision {
    Current(crate::library::ConnectedCatdbSourceRecord),
    Pending(ConnectionRecord, BearerToken),
    Absent,
}

fn persist_connected_catdb(
    store: &crate::library::LibraryStore,
    vault: &dyn BearerVault,
    source: crate::library::ConnectedCatdbSourceRecord,
    token: &BearerToken,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    if store
        .read_connected_catdb()
        .map_err(map_library_error)?
        .is_some()
    {
        return Err(MobileEnterpriseError::AlreadyConfigured);
    }
    vault.set(&source.credential_reference, token)?;
    if let Err(error) = store.add_connected_catdb(source.clone()) {
        if vault.delete(&source.credential_reference).is_err() {
            return Err(MobileEnterpriseError::RollbackFailed);
        }
        return Err(map_library_error(error));
    }
    Ok(connected_state(&source))
}

fn disconnect_connected_catdb_from(
    store: &crate::library::LibraryStore,
    vault: &dyn BearerVault,
    connection_id: &str,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    let source = record_for(store, connection_id)?;
    let token = vault.get(&source.credential_reference)?;
    vault.delete(&source.credential_reference)?;
    if let Err(error) = store.remove_connected_catdb(connection_id) {
        if token
            .as_ref()
            .is_some_and(|token| vault.set(&source.credential_reference, token).is_err())
        {
            return Err(MobileEnterpriseError::RollbackFailed);
        }
        return Err(map_library_error(error));
    }
    Ok(disconnected_state())
}

fn disconnect_connected_catdb_operation(
    store: &crate::library::LibraryStore,
    vault: &dyn BearerVault,
    legacy_path: &Path,
    connection_id: &str,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    #[cfg(test)]
    if let Some(signal_path) = std::env::var_os("MDREADER_CONNECTED_CATDB_DISCONNECT_ENTRY_SIGNAL")
    {
        std::fs::write(signal_path, b"entered")
            .map_err(|_| MobileEnterpriseError::PersistFailed)?;
    }
    with_legacy_migration_lock(legacy_path, || {
        disconnect_connected_catdb_from(store, vault, connection_id)
    })
}

#[cfg(test)]
fn pin_connected_query(
    source: &crate::library::ConnectedCatdbSourceRecord,
    request: &mut EntityQueryRequest,
) -> Result<String, MobileEnterpriseError> {
    pin_connected_query_phase(source, request, ConnectedQueryPhase::Initial)
}

#[derive(Debug, Clone, Copy)]
enum ConnectedQueryPhase {
    Initial,
    Continuation { query_id: uuid::Uuid },
}

fn pin_connected_query_phase(
    source: &crate::library::ConnectedCatdbSourceRecord,
    request: &mut EntityQueryRequest,
    phase: ConnectedQueryPhase,
) -> Result<String, MobileEnterpriseError> {
    if source.selected_source_ids().is_empty() {
        return Err(MobileEnterpriseError::NoSourcesSelected);
    }
    if request.entity != "document.Document" {
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    match phase {
        ConnectedQueryPhase::Initial if request.query_id.is_some() || request.cursor.is_some() => {
            return Err(MobileEnterpriseError::InvalidRequest);
        }
        ConnectedQueryPhase::Continuation { query_id }
            if request.query_id != Some(query_id) || request.cursor.is_none() =>
        {
            return Err(MobileEnterpriseError::InvalidResponse);
        }
        ConnectedQueryPhase::Initial | ConnectedQueryPhase::Continuation { .. } => {}
    }
    request.workspace_id = source.workspace_id;
    request.catalog_revision_id = Some(source.catalog_revision_id);
    if request
        .schema_version_id
        .is_some_and(|schema_version_id| schema_version_id != source.default_schema_version_id)
    {
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    request.schema_version_id = Some(source.default_schema_version_id);
    request.source_ids = Some(source.selected_source_ids().to_vec());
    request
        .validate()
        .map_err(|_| MobileEnterpriseError::InvalidRequest)?;
    let body =
        serde_json::to_string(&request).map_err(|_| MobileEnterpriseError::InvalidRequest)?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    Ok(body)
}

async fn verify_connected_identity(
    source: &crate::library::ConnectedCatdbSourceRecord,
    token: &BearerToken,
) -> Result<(), MobileEnterpriseError> {
    let response = execute_http(
        &ConnectionRecord::from(source),
        token,
        reqwest::Method::GET,
        "/v1/session",
        None,
    )
    .await?;
    if matches!(response.status, 401 | 403) {
        return Err(MobileEnterpriseError::PermissionDenied);
    }
    if response.status != 200 {
        return Err(MobileEnterpriseError::InvalidResponse);
    }
    let session: SessionResponse = serde_json::from_str(&response.body_json)
        .map_err(|_| MobileEnterpriseError::InvalidResponse)?;
    if session.deployment_id != source.deployment_id
        || session.workspace_ids.as_slice() != [source.workspace_id]
    {
        return Err(MobileEnterpriseError::ConnectionIdentityChanged);
    }
    if !session.scopes.iter().any(|scope| scope == "query") {
        return Err(MobileEnterpriseError::PermissionDenied);
    }
    Ok(())
}

async fn execute_connected_query(
    source: &crate::library::ConnectedCatdbSourceRecord,
    token: &BearerToken,
    request: EntityQueryRequest,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    execute_connected_query_phase(source, token, request, ConnectedQueryPhase::Initial).await
}

async fn execute_connected_query_phase(
    source: &crate::library::ConnectedCatdbSourceRecord,
    token: &BearerToken,
    mut request: EntityQueryRequest,
    phase: ConnectedQueryPhase,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    let body = pin_connected_query_phase(source, &mut request, phase)?;
    verify_connected_identity(source, token).await?;
    execute_http(
        &ConnectionRecord::from(source),
        token,
        reqwest::Method::POST,
        "/v1/entity/query",
        Some(body),
    )
    .await
}

struct ConnectedPaginationBudget {
    logical_limit: usize,
    page_size: usize,
    rows: usize,
    bytes: usize,
    pages: usize,
    seen_cursors: BTreeSet<String>,
    seen_record_keys: BTreeSet<String>,
}

impl ConnectedPaginationBudget {
    fn new(logical_limit: u32, page_size: u32) -> Result<Self, MobileEnterpriseError> {
        let logical_limit =
            usize::try_from(logical_limit).map_err(|_| MobileEnterpriseError::InvalidRequest)?;
        let page_size =
            usize::try_from(page_size).map_err(|_| MobileEnterpriseError::InvalidRequest)?;
        if logical_limit == 0 || page_size == 0 {
            return Err(MobileEnterpriseError::InvalidRequest);
        }
        Ok(Self {
            logical_limit,
            page_size,
            rows: 0,
            bytes: 0,
            pages: 0,
            seen_cursors: BTreeSet::new(),
            seen_record_keys: BTreeSet::new(),
        })
    }

    fn record_rows(
        &mut self,
        rows: &[catdb_api_types::EntityRow],
    ) -> Result<(), MobileEnterpriseError> {
        for row in rows {
            if !self.seen_record_keys.insert(row.record_key.clone()) {
                return Err(MobileEnterpriseError::InvalidResponse);
            }
        }
        Ok(())
    }

    fn record_page(
        &mut self,
        rows: usize,
        next_cursor: Option<&catdb_api_types::EntityCursor>,
        response_bytes: usize,
    ) -> Result<(), MobileEnterpriseError> {
        self.pages = self
            .pages
            .checked_add(1)
            .ok_or(MobileEnterpriseError::InvalidResponse)?;
        self.rows = self
            .rows
            .checked_add(rows)
            .ok_or(MobileEnterpriseError::InvalidResponse)?;
        self.bytes = self
            .bytes
            .checked_add(response_bytes)
            .ok_or(MobileEnterpriseError::ResponseTooLarge)?;
        if self.pages > MAX_CONNECTED_LIBRARY_PAGES
            || rows > self.page_size
            || self.rows > self.logical_limit
        {
            return Err(MobileEnterpriseError::InvalidResponse);
        }
        if self.bytes > MAX_CONNECTED_LIBRARY_RESPONSE_BYTES {
            return Err(MobileEnterpriseError::ResponseTooLarge);
        }
        if let Some(cursor) = next_cursor {
            if self.rows >= self.logical_limit
                || !self.seen_cursors.insert(cursor.as_str().to_string())
            {
                return Err(MobileEnterpriseError::InvalidResponse);
            }
        }
        Ok(())
    }
}

async fn execute_connected_library_page(
    source: &crate::library::ConnectedCatdbSourceRecord,
    token: &BearerToken,
    request: EntityQueryRequest,
    cancellation: &catdb_connectors::CancellationToken,
    deadline: tokio::time::Instant,
    phase: ConnectedQueryPhase,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(MobileEnterpriseError::Cancelled),
        response = tokio::time::timeout_at(
            deadline,
            execute_connected_query_phase(source, token, request, phase),
        ) => response.map_err(|_| MobileEnterpriseError::TimedOut)?,
    }
}

fn parse_connected_library_response(
    source: &crate::library::ConnectedCatdbSourceRecord,
    response: MobileEnterpriseResponse,
    selected_fields: &[String],
) -> Result<crate::library_query::LibraryEntityQueryResponse, MobileEnterpriseError> {
    if matches!(response.status, 401 | 403) {
        return Err(MobileEnterpriseError::PermissionDenied);
    }
    if response.status != 200 {
        return Err(MobileEnterpriseError::InvalidResponse);
    }
    let answer: EntityQueryResponse = serde_json::from_str(&response.body_json)
        .map_err(|_| MobileEnterpriseError::InvalidResponse)?;
    if answer.pins.catalog_revision_id != source.catalog_revision_id {
        return Err(MobileEnterpriseError::ConnectionIdentityChanged);
    }
    if answer.pins.schema_version_id != source.default_schema_version_id {
        return Err(MobileEnterpriseError::ConnectionIdentityChanged);
    }

    let permitted = source
        .permitted_sources
        .iter()
        .map(|descriptor| (descriptor.source_id, descriptor.source_version_id))
        .collect::<BTreeMap<_, _>>();
    let mut expected_source_versions = BTreeSet::new();
    let expected = source
        .selected_source_ids()
        .iter()
        .map(|source_id| {
            permitted
                .get(source_id)
                .copied()
                .and_then(|version_id| {
                    expected_source_versions
                        .insert(version_id)
                        .then_some((*source_id, version_id))
                })
                .ok_or(MobileEnterpriseError::ConnectionIdentityChanged)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut observed = BTreeMap::new();
    for execution in &answer.sources {
        if observed
            .insert(execution.source_id, execution.source_version_id)
            .is_some()
            || expected.get(&execution.source_id) != Some(&execution.source_version_id)
        {
            return Err(MobileEnterpriseError::ConnectionIdentityChanged);
        }
    }
    if observed != expected
        || answer.pins.source_version_ids.len() != expected.len()
        || answer
            .pins
            .source_version_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            != expected.values().copied().collect()
    {
        return Err(MobileEnterpriseError::ConnectionIdentityChanged);
    }
    let excluded_source_ids = answer
        .sources
        .iter()
        .filter(|source| source.outcome != catdb_api_types::SourceOutcome::Ran)
        .map(|source| source.source_id)
        .collect::<Vec<_>>();
    let derived_completeness = if excluded_source_ids.is_empty() {
        catdb_api_types::Completeness::Complete
    } else {
        catdb_api_types::Completeness::Partial {
            excluded_source_ids,
        }
    };
    if answer.completeness != derived_completeness {
        return Err(MobileEnterpriseError::InvalidResponse);
    }
    let pinned_mappings = answer
        .pins
        .mapping_version_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let selected_fields = selected_fields
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut record_keys = BTreeSet::new();
    for row in &answer.rows {
        if !record_keys.insert(row.record_key.as_str())
            || row
                .fields
                .keys()
                .any(|field| !selected_fields.contains(field.as_str()))
            || row
                .provenance
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != selected_fields
        {
            return Err(MobileEnterpriseError::InvalidResponse);
        }
        for (field, evidence) in &row.provenance {
            let selected_value_matches = match (row.fields.get(field), evidence.selected()) {
                (Some(value), Some(observation)) if value.is_null() => {
                    observation.state().kind() == catdb_provenance::ObservationKind::ObservedNull
                }
                (Some(value), Some(observation)) => observation.state().value() == Some(value),
                (None, None) => true,
                _ => false,
            };
            if evidence.field() != field
                || !selected_value_matches
                || evidence.provenance_rule_version_id() != answer.pins.provenance_rule_version_id
                || evidence.considered().iter().any(|observation| {
                    !expected_source_versions.contains(&observation.source_version_id())
                        || !pinned_mappings.contains(&observation.mapping_version_id())
                })
            {
                return Err(MobileEnterpriseError::ConnectionIdentityChanged);
            }
        }
    }

    Ok(crate::library_query::LibraryEntityQueryResponse {
        answer,
        memberships: vec![crate::library_query::LibrarySourceExecution {
            membership_id: crate::library_query::membership_id(&source.id),
            registry_id: source.id.clone(),
            name: source.label.clone(),
            kind: crate::library_query::LibraryQueryMemberKind::ConnectedCatdb,
            source_ids: source.selected_source_ids().to_vec(),
            editable: false,
            active: false,
        }],
    })
}

fn validate_current_connected_source(
    expected: &crate::library::ConnectedCatdbSourceRecord,
    current: Option<&crate::library::ConnectedCatdbSourceRecord>,
) -> Result<(), MobileEnterpriseError> {
    if current == Some(expected) {
        Ok(())
    } else {
        Err(MobileEnterpriseError::ConnectionIdentityChanged)
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn connected_catdb_state(
    app: AppHandle,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    #[cfg(mobile)]
    {
        let _ = (app, state);
        return Ok(disconnected_state());
    }
    #[cfg(not(mobile))]
    {
        let owner = MobileEnterpriseOperationOwner::begin(&state, "connected-state")?;
        let cancellation = owner.cancellation();
        let source = tokio::select! {
            _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
            source = connected_source_or_migrate(&app) => source?,
        };
        owner.finish()?;
        Ok(source
            .as_ref()
            .map_or_else(disconnected_state, connected_state))
    }
}

async fn commit_mobile_enterprise_operation<T, Operation>(
    state: &MobileEnterpriseState,
    owner: MobileEnterpriseOperationOwner<'_>,
    operation: Operation,
) -> Result<T, MobileEnterpriseError>
where
    T: Send + 'static,
    Operation: FnOnce() -> Result<T, MobileEnterpriseError> + Send + 'static,
{
    commit_mobile_enterprise_operation_observed(state, owner, || {}, operation).await
}

async fn commit_mobile_enterprise_operation_observed<T, Observe, Observation, Operation>(
    state: &MobileEnterpriseState,
    owner: MobileEnterpriseOperationOwner<'_>,
    observe_worker: Observe,
    operation: Operation,
) -> Result<T, MobileEnterpriseError>
where
    T: Send + 'static,
    Observe: FnOnce() -> Observation + Send + 'static,
    Observation: Send + 'static,
    Operation: FnOnce() -> Result<T, MobileEnterpriseError> + Send + 'static,
{
    let lifecycle_commits = Arc::clone(&state.lifecycle_commits);
    let authority = owner.commit_authority();
    let worker_authority = authority.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _observation = observe_worker();
        let _commit = lifecycle_commits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        worker_authority.begin_commit()?;
        let result = operation();
        worker_authority.finish_commit();
        result
    })
    .await
    .map_err(|_| MobileEnterpriseError::PersistFailed)??;
    owner.finish_committed();
    Ok(result)
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn enroll_connected_catdb(
    app: AppHandle,
    server_origin: String,
    bearer_token: String,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    #[cfg(mobile)]
    {
        let _ = (app, server_origin, bearer_token, state);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let owner = MobileEnterpriseOperationOwner::begin_lifecycle(&state)?;
        let cancellation = owner.cancellation();
        let server_origin = exact_origin(&server_origin)?;
        let token = BearerToken(bearer_token);
        validate_bearer(&token.0)?;
        let current = tokio::select! {
            _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
            current = connected_source_or_migrate(&app) => current?,
        };
        if current.is_some() {
            return Err(MobileEnterpriseError::AlreadyConfigured);
        }
        let source = tokio::select! {
            _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
            source = discover_connected_catdb(server_origin, &token) => source?,
        };
        let store = library_store(&app)?;
        let vault = KeyringBearerVault::new(&app.config().identifier)?;
        commit_mobile_enterprise_operation(&state, owner, move || {
            persist_connected_catdb(&store, &vault, source, &token)
        })
        .await
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn set_connected_catdb_source_enabled(
    app: AppHandle,
    connection_id: String,
    source_id: uuid::Uuid,
    enabled: bool,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    #[cfg(mobile)]
    {
        let _ = (app, connection_id, source_id, enabled, state);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let owner = MobileEnterpriseOperationOwner::begin_lifecycle(&state)?;
        let store = library_store(&app)?;
        commit_mobile_enterprise_operation(&state, owner, move || {
            store
                .set_connected_catdb_source_enabled(&connection_id, source_id, enabled)
                .map_err(map_library_error)
                .map(|source| connected_state(&source))
        })
        .await
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn disconnect_connected_catdb(
    app: AppHandle,
    connection_id: String,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    #[cfg(mobile)]
    {
        let _ = (app, connection_id, state);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let owner = MobileEnterpriseOperationOwner::begin_lifecycle(&state)?;
        let store = library_store(&app)?;
        let vault = KeyringBearerVault::new(&app.config().identifier)?;
        let legacy_path = legacy_registry_path(&app)?;
        commit_mobile_enterprise_operation(&state, owner, move || {
            disconnect_connected_catdb_operation(&store, &vault, &legacy_path, &connection_id)
        })
        .await
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn connected_catdb_entity_query(
    app: AppHandle,
    connection_id: String,
    request: EntityQueryRequest,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    #[cfg(mobile)]
    {
        let _ = (app, connection_id, request, state);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let operation_id = format!("entity-query:{connection_id}");
        let owner = MobileEnterpriseOperationOwner::begin(&state, &operation_id)?;
        let cancellation = owner.cancellation();
        let identifier = app.config().identifier.clone();
        let source = tokio::select! {
            _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
            source = connected_source_or_migrate(&app) => source?,
        }
        .filter(|source| source.id == connection_id)
        .ok_or(MobileEnterpriseError::ConnectionNotFound)?;
        let credential_reference = source.credential_reference.clone();
        let token = tokio::task::spawn_blocking(move || {
            let vault = KeyringBearerVault::new(&identifier)?;
            let token = vault
                .get(&credential_reference)?
                .ok_or(MobileEnterpriseError::CredentialUnavailable)?;
            Ok::<_, MobileEnterpriseError>(token)
        })
        .await
        .map_err(|_| MobileEnterpriseError::PersistFailed)??;
        // Deployment, workspace, revision, and source membership are native
        // authority. The renderer supplies only semantic shape such as fields
        // and filters.
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
            response = execute_connected_query(&source, &token, request) => response?,
        };
        owner.finish()?;
        Ok(response)
    }
}

/// Query one authenticated Connected CatDB membership as an exact Library evidence group.
#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn query_connected_library_documents(
    app: AppHandle,
    connection_id: String,
    request_id: String,
    request: EntityQueryRequest,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<crate::library_query::LibraryEntityQueryResponse, MobileEnterpriseError> {
    if request_id.is_empty() {
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    let owner = MobileEnterpriseOperationOwner::begin(&state, &request_id)?;
    let cancellation = owner.cancellation();
    let identifier = app.config().identifier.clone();
    let source = tokio::select! {
        _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
        source = connected_source_or_migrate(&app) => source?,
    }
    .filter(|source| source.id == connection_id)
    .ok_or(MobileEnterpriseError::ConnectionNotFound)?;
    let credential_reference = source.credential_reference.clone();
    let token = tokio::task::spawn_blocking(move || {
        let vault = KeyringBearerVault::new(&identifier)?;
        vault
            .get(&credential_reference)?
            .ok_or(MobileEnterpriseError::CredentialUnavailable)
    })
    .await
    .map_err(|_| MobileEnterpriseError::PersistFailed)??;
    let pinned_shape = request;
    let mut budget = ConnectedPaginationBudget::new(pinned_shape.limit, pinned_shape.page_size)?;
    let deadline = tokio::time::Instant::now()
        .checked_add(REQUEST_DEADLINE)
        .ok_or(MobileEnterpriseError::TimedOut)?;
    let response = execute_connected_library_page(
        &source,
        &token,
        pinned_shape.clone(),
        &cancellation,
        deadline,
        ConnectedQueryPhase::Initial,
    )
    .await?;
    let response_bytes = response.body_json.len();
    let mut evidence = parse_connected_library_response(&source, response, &pinned_shape.select)?;
    budget.record_rows(&evidence.answer.rows)?;
    budget.record_page(
        evidence.answer.rows.len(),
        evidence.answer.next_cursor.as_ref(),
        response_bytes,
    )?;
    while let Some(cursor) = evidence.answer.next_cursor.take() {
        let mut continuation = pinned_shape.clone();
        continuation.query_id = Some(evidence.answer.query_id);
        continuation.cursor = Some(cursor);
        let response = execute_connected_library_page(
            &source,
            &token,
            continuation,
            &cancellation,
            deadline,
            ConnectedQueryPhase::Continuation {
                query_id: evidence.answer.query_id,
            },
        )
        .await?;
        let response_bytes = response.body_json.len();
        let page = parse_connected_library_response(&source, response, &pinned_shape.select)?;
        budget.record_rows(&page.answer.rows)?;
        budget.record_page(
            page.answer.rows.len(),
            page.answer.next_cursor.as_ref(),
            response_bytes,
        )?;
        if page.answer.query_id != evidence.answer.query_id
            || page.answer.pins != evidence.answer.pins
            || page.answer.sources != evidence.answer.sources
            || page.answer.completeness != evidence.answer.completeness
            || page.memberships != evidence.memberships
        {
            return Err(MobileEnterpriseError::ConnectionIdentityChanged);
        }
        evidence.answer.rows.extend(page.answer.rows);
        evidence.answer.next_cursor = page.answer.next_cursor;
    }
    let current = tokio::select! {
        _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
        current = connected_source_or_migrate(&app) => current?,
    };
    validate_current_connected_source(&source, current.as_ref())?;
    owner.finish()?;
    Ok(evidence)
}

/// Cancel one renderer-owned Connected CatDB Library query before it publishes evidence.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn cancel_connected_catdb_query(
    request_id: String,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<bool, MobileEnterpriseError> {
    if !valid_identifier(&request_id, MAX_OPERATION_ID_BYTES) {
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    Ok(state.cancel_operation(&request_id))
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn mobile_enterprise_connection(
    app: AppHandle,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<Option<MobileEnterpriseConnection>, MobileEnterpriseError> {
    let owner = MobileEnterpriseOperationOwner::begin(&state, "connection-state")?;
    #[cfg(not(feature = "e2e-build"))]
    let cancellation = owner.cancellation();
    #[cfg(feature = "e2e-build")]
    let record = e2e_record()?;
    #[cfg(not(feature = "e2e-build"))]
    #[cfg(mobile)]
    let record: ConnectionRecord = return Ok(None);
    #[cfg(not(feature = "e2e-build"))]
    #[cfg(not(mobile))]
    let record = {
        let source = tokio::select! {
            _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
            source = connected_source_or_migrate(&app) => source?,
        };
        let Some(source) = source else {
            return Ok(None);
        };
        ConnectionRecord::from(&source)
    };
    let _ = app;
    owner.finish()?;
    Ok(Some(MobileEnterpriseConnection {
        connection_id: record.id,
        label: record.label,
    }))
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn mobile_enterprise_request(
    app: AppHandle,
    connection_id: String,
    operation: MobileEnterpriseOperation,
    body_json: Option<String>,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    let operation_id = format!("transport:{connection_id}:{operation:?}");
    let owner = MobileEnterpriseOperationOwner::begin(&state, &operation_id)?;
    let cancellation = owner.cancellation();
    #[cfg(feature = "e2e-build")]
    let (record, token, connected_source) = {
        if connection_id != E2E_CONNECTION_ID {
            return Err(MobileEnterpriseError::ConnectionNotFound);
        }
        (
            e2e_record()?,
            BearerToken(E2E_BEARER.to_string()),
            None::<crate::library::ConnectedCatdbSourceRecord>,
        )
    };
    #[cfg(not(feature = "e2e-build"))]
    let (record, token, connected_source) = {
        #[cfg(mobile)]
        {
            let _ = (&app, &connection_id);
            return Err(MobileEnterpriseError::ConnectionNotFound);
        }
        #[cfg(not(mobile))]
        {
            let identifier = app.config().identifier.clone();
            let source = tokio::select! {
                _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
                source = connected_source_or_migrate(&app) => source?,
            }
            .filter(|source| source.id == connection_id)
            .ok_or(MobileEnterpriseError::ConnectionNotFound)?;
            tokio::task::spawn_blocking(move || {
                let record = ConnectionRecord::from(&source);
                let vault = KeyringBearerVault::new(&identifier)?;
                let token = vault
                    .get(&record.credential_reference)?
                    .ok_or(MobileEnterpriseError::CredentialUnavailable)?;
                Ok::<_, MobileEnterpriseError>((record, token, Some(source)))
            })
            .await
            .map_err(|_| MobileEnterpriseError::PersistFailed)??
        }
    };
    let _ = app;
    if operation == MobileEnterpriseOperation::EntityQuery {
        if let Some(source) = connected_source {
            let request: EntityQueryRequest = serde_json::from_str(
                body_json
                    .as_deref()
                    .ok_or(MobileEnterpriseError::InvalidRequest)?,
            )
            .map_err(|_| MobileEnterpriseError::InvalidRequest)?;
            let response = tokio::select! {
                _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
                response = execute_connected_query(&source, &token, request) => response?,
            };
            owner.finish()?;
            return Ok(response);
        }
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(MobileEnterpriseError::Cancelled),
        response = execute_request(record, token, operation, body_json) => response?,
    };
    owner.finish()?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;

    use super::{
        commit_mobile_enterprise_operation, commit_mobile_enterprise_operation_observed,
        disconnect_connected_catdb_from, disconnect_connected_catdb_operation,
        discover_connected_catdb, execute_connected_query, execute_request,
        migrate_legacy_connection, parse_connected_library_response, persist_connected_catdb,
        pin_connected_query, publish_legacy_migration,
        publish_legacy_migration_transaction_with_rename, publish_legacy_migration_with_rename,
        retire_matching_legacy_after_publication, validate_current_connected_source,
        with_legacy_migration_lock, BearerToken, BearerVault, ConnectedCatdbState,
        ConnectedPaginationBudget, ConnectionRecord, MobileEnterpriseError,
        MobileEnterpriseOperation, MobileEnterpriseOperationOwner, MobileEnterpriseResponse,
        MobileEnterpriseState,
    };
    use crate::library::{
        ConnectedCatdbSourceDescriptor, ConnectedCatdbSourceRecord, LibraryStore,
    };
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    fn park_lifecycle_commit(
        commit: Arc<Mutex<()>>,
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

    #[derive(Default)]
    struct MemoryBearerVault(Mutex<HashMap<String, BearerToken>>);

    impl BearerVault for MemoryBearerVault {
        fn get(&self, reference: &str) -> Result<Option<BearerToken>, MobileEnterpriseError> {
            Ok(self.0.lock().expect("vault lock").get(reference).cloned())
        }

        fn set(&self, reference: &str, token: &BearerToken) -> Result<(), MobileEnterpriseError> {
            self.0
                .lock()
                .expect("vault lock")
                .insert(reference.to_string(), token.clone());
            Ok(())
        }

        fn delete(&self, reference: &str) -> Result<(), MobileEnterpriseError> {
            self.0.lock().expect("vault lock").remove(reference);
            Ok(())
        }
    }

    #[tokio::test]
    async fn dropping_a_lifecycle_future_before_the_worker_commit_point_prevents_mutation() {
        struct Completion(std::sync::mpsc::Sender<()>);

        impl Drop for Completion {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let state = MobileEnterpriseState::default();
        let owner =
            MobileEnterpriseOperationOwner::begin_lifecycle(&state).expect("enrollment owner");
        let mutated = Arc::new(AtomicBool::new(false));
        let worker_mutated = Arc::clone(&mutated);
        let (worker_started_tx, worker_started_rx) = std::sync::mpsc::channel();
        let (worker_completed_tx, worker_completed_rx) = std::sync::mpsc::channel();
        let (release_commit, commit_gate) =
            park_lifecycle_commit(Arc::clone(&state.lifecycle_commits));
        let mut enrollment = Box::pin(commit_mobile_enterprise_operation_observed(
            &state,
            owner,
            move || {
                worker_started_tx.send(()).expect("signal worker");
                Completion(worker_completed_tx)
            },
            move || {
                worker_mutated.store(true, Ordering::SeqCst);
                Ok("enrolled")
            },
        ));

        tokio::select! {
            result = &mut enrollment => panic!("enrollment completed while commit point was parked: {result:?}"),
            started = tokio::task::spawn_blocking(move || worker_started_rx.recv()) => {
                started.expect("start waiter did not panic").expect("worker started");
            }
        }
        drop(enrollment);
        release_commit.send(()).expect("release commit point");
        commit_gate.join().expect("commit gate did not panic");
        tokio::task::spawn_blocking(move || worker_completed_rx.recv())
            .await
            .expect("completion waiter did not panic")
            .expect("worker completed");
        assert!(
            !mutated.load(Ordering::SeqCst),
            "a detached enrollment worker mutated after its owner future was dropped"
        );
    }

    #[tokio::test]
    async fn a_stale_lifecycle_serial_cannot_mutate_after_replacement() {
        let state = MobileEnterpriseState::default();
        let owner = MobileEnterpriseOperationOwner::begin_lifecycle(&state).expect("first owner");
        let mutated = Arc::new(AtomicBool::new(false));
        let worker_mutated = Arc::clone(&mutated);
        let (worker_started_tx, worker_started_rx) = std::sync::mpsc::channel();
        let (release_commit, commit_gate) =
            park_lifecycle_commit(Arc::clone(&state.lifecycle_commits));
        let mut disconnect = Box::pin(commit_mobile_enterprise_operation_observed(
            &state,
            owner,
            move || worker_started_tx.send(()).expect("signal worker"),
            move || {
                worker_mutated.store(true, Ordering::SeqCst);
                Ok("disconnected")
            },
        ));

        tokio::select! {
            result = &mut disconnect => panic!("disconnect completed while commit point was parked: {result:?}"),
            started = tokio::task::spawn_blocking(move || worker_started_rx.recv()) => {
                started.expect("start waiter did not panic").expect("worker started");
            }
        }
        let replacement =
            MobileEnterpriseOperationOwner::begin_lifecycle(&state).expect("replacement owner");
        release_commit.send(()).expect("release commit point");
        commit_gate.join().expect("commit gate did not panic");
        assert_eq!(disconnect.await, Err(MobileEnterpriseError::Cancelled));
        assert!(!mutated.load(Ordering::SeqCst));
        assert_eq!(replacement.finish(), Ok(()));
    }

    #[tokio::test]
    async fn a_panicking_lifecycle_worker_releases_ownership_and_the_commit_lease() {
        let state = MobileEnterpriseState::default();
        let owner = MobileEnterpriseOperationOwner::begin_lifecycle(&state).expect("first owner");
        let result = commit_mobile_enterprise_operation(&state, owner, || -> Result<(), _> {
            panic!("deliberate worker panic")
        })
        .await;
        assert_eq!(result, Err(MobileEnterpriseError::PersistFailed));

        let replacement =
            MobileEnterpriseOperationOwner::begin_lifecycle(&state).expect("replacement owner");
        assert_eq!(
            commit_mobile_enterprise_operation(&state, replacement, || Ok("enrolled")).await,
            Ok("enrolled")
        );
    }

    fn connected_source() -> ConnectedCatdbSourceRecord {
        let mut source = ConnectedCatdbSourceRecord::new(
            "catdb.example.com".to_string(),
            "https://catdb.example.com/".to_string(),
            "credential-reference-1".to_string(),
            crate::library::ConnectedCatdbAuthority {
                deployment_id: uuid::Uuid::from_u128(2),
                workspace_id: uuid::Uuid::from_u128(3),
                catalog_revision_id: uuid::Uuid::from_u128(4),
                default_schema_version_id: uuid::Uuid::from_u128(5),
            },
            vec![ConnectedCatdbSourceDescriptor {
                source_id: uuid::Uuid::from_u128(11),
                source_version_id: uuid::Uuid::from_u128(12),
                runtime_bound: true,
                connector_id: Some("postgres-primary".to_string()),
            }],
        );
        source.selected_source_ids = vec![uuid::Uuid::from_u128(11)];
        source
    }

    fn connected_query_request() -> catdb_api_types::EntityQueryRequest {
        serde_json::from_value(serde_json::json!({
            "workspace_id": "00000000-0000-0000-0000-000000000099",
            "catalog_revision_id": null,
            "schema_version_id": null,
            "entity": "document.Document",
            "select": ["title"],
            "filters": [],
            "order_by": [],
            "source_ids": null,
            "max_staleness_ms": null,
            "require_complete": false,
            "page_size": 10,
            "limit": 10,
            "query_id": null,
            "cursor": null
        }))
        .expect("valid connected Library request")
    }

    fn connected_response(
        record_keys: &[&str],
        outcome: catdb_api_types::SourceOutcome,
        completeness: catdb_api_types::Completeness,
    ) -> MobileEnterpriseResponse {
        let provenance_rule_version_id = uuid::Uuid::from_u128(27);
        let mapping_version_id = uuid::Uuid::from_u128(25);
        let source_version_id = uuid::Uuid::from_u128(12);
        let rows = record_keys
            .iter()
            .map(|record_key| {
                let title = format!("Title for {record_key}");
                let observation = catdb_provenance::FieldObservation::observed(
                    source_version_id,
                    *record_key,
                    mapping_version_id,
                    1,
                    serde_json::json!(title),
                );
                let evidence = catdb_provenance::FieldProvenance::new(
                    provenance_rule_version_id,
                    "title",
                    vec![observation],
                    Some(0),
                )
                .expect("valid field evidence");
                catdb_api_types::EntityRow {
                    record_key: (*record_key).to_string(),
                    fields: std::collections::BTreeMap::from([(
                        "title".to_string(),
                        serde_json::json!(title),
                    )]),
                    provenance: std::collections::BTreeMap::from([("title".to_string(), evidence)]),
                }
            })
            .collect::<Vec<_>>();
        let response = catdb_api_types::EntityQueryResponse {
            query_id: uuid::Uuid::from_u128(21),
            pins: catdb_api_types::PinnedVersions {
                catalog_resolver_id: uuid::Uuid::from_u128(22),
                catalog_revision_id: uuid::Uuid::from_u128(4),
                schema_version_id: uuid::Uuid::from_u128(5),
                physical_schema_version_ids: vec![uuid::Uuid::from_u128(24)],
                mapping_version_ids: vec![mapping_version_id],
                source_version_ids: vec![source_version_id],
                policy_version_ids: vec![uuid::Uuid::from_u128(26)],
                provenance_rule_version_id,
            },
            sources: vec![catdb_api_types::SourceExecution {
                source_id: uuid::Uuid::from_u128(11),
                source_version_id,
                outcome,
                retry_after_ms: None,
                freshness_ms: (outcome == catdb_api_types::SourceOutcome::Ran).then_some(0),
                scanned: record_keys.len() as u64,
                returned: record_keys.len() as u64,
            }],
            completeness,
            rows,
            next_cursor: None,
        };
        MobileEnterpriseResponse {
            status: 200,
            body_json: serde_json::to_string(&response).expect("serialize connected response"),
        }
    }

    #[test]
    fn bearer_tokens_and_native_references_are_redacted() {
        let token = BearerToken("do-not-print-me".to_string());
        let record = ConnectionRecord {
            id: "connection-1".to_string(),
            label: "Test".to_string(),
            server_origin: "https://catdb.example.com".to_string(),
            credential_reference: "secret-reference-1".to_string(),
        };
        assert_eq!(format!("{token:?}"), "BearerToken([redacted])");
        let rendered = format!("{record:?}");
        assert!(!rendered.contains("secret-reference-1"));
        assert!(rendered.contains("[native-secure-reference]"));
    }

    #[test]
    fn lifecycle_generation_retires_inflight_and_rejects_late_query_publication() {
        let state = MobileEnterpriseState::default();
        let inflight =
            MobileEnterpriseOperationOwner::begin(&state, "inflight-query").expect("query owner");
        let lifecycle =
            MobileEnterpriseOperationOwner::begin_lifecycle(&state).expect("lifecycle owner");

        assert!(inflight.cancellation().is_cancelled());
        assert_eq!(inflight.finish(), Err(MobileEnterpriseError::Cancelled));
        assert!(matches!(
            MobileEnterpriseOperationOwner::begin(&state, "late-renderer-query"),
            Err(MobileEnterpriseError::Cancelled)
        ));
        assert_eq!(lifecycle.finish(), Ok(()));
        assert_eq!(
            MobileEnterpriseOperationOwner::begin(&state, "post-lifecycle-query")
                .expect("admission reopens")
                .finish(),
            Ok(())
        );
    }

    #[test]
    fn lifecycle_admission_retires_a_query_admitted_before_disconnect() {
        let state = MobileEnterpriseState::default();
        let query = state
            .begin_operation("query-before-disconnect")
            .expect("query admission");
        let lifecycle = state
            .begin_lifecycle_operation()
            .expect("disconnect admission");

        assert!(query.cancellation.is_cancelled());
        assert_eq!(
            state.finish_operation(&query),
            Err(MobileEnterpriseError::Cancelled)
        );
        assert_eq!(state.finish_operation(&lifecycle), Ok(()));
    }

    #[test]
    fn renderer_reserved_id_cannot_replace_or_cancel_lifecycle() {
        let state = MobileEnterpriseState::default();
        let lifecycle = state
            .begin_lifecycle_operation()
            .expect("trusted lifecycle operation");
        assert!(matches!(
            state.begin_operation(super::LIBRARY_LIFECYCLE_OPERATION_ID),
            Err(MobileEnterpriseError::Cancelled)
        ));
        assert!(
            !state.cancel_operation(super::LIBRARY_LIFECYCLE_OPERATION_ID),
            "renderer cancellation reached the trusted lifecycle namespace"
        );
        assert!(!lifecycle.cancellation.is_cancelled());
        assert_eq!(state.finish_operation(&lifecycle), Ok(()));
    }

    #[test]
    fn disconnect_retires_inflight_connected_oauth_and_query_operations() {
        let state = MobileEnterpriseState::default();
        let enrollment = state
            .begin_operation("enroll")
            .expect("enrollment operation");
        let query = state
            .begin_operation("library:connected")
            .expect("connected query");

        state.cancel_all_operations().expect("cancel operations");

        assert!(enrollment.cancellation.is_cancelled());
        assert!(query.cancellation.is_cancelled());
        assert_eq!(
            state.finish_operation(&enrollment),
            Err(MobileEnterpriseError::Cancelled)
        );
        assert_eq!(
            state.finish_operation(&query),
            Err(MobileEnterpriseError::Cancelled)
        );
    }

    #[test]
    fn a_replaced_connected_query_cannot_publish_after_its_successor() {
        let state = MobileEnterpriseState::default();
        let first = state
            .begin_operation("library:connected")
            .expect("first query");
        let replacement = state
            .begin_operation("library:connected")
            .expect("replacement query");

        assert!(first.cancellation.is_cancelled());
        assert_eq!(
            state.finish_operation(&first),
            Err(MobileEnterpriseError::Cancelled)
        );
        assert_eq!(state.finish_operation(&replacement), Ok(()));
    }

    #[test]
    fn connected_pagination_rejects_repeated_cursors_and_an_unbounded_total() {
        let cursor = catdb_api_types::EntityCursor::new("v1.same".to_string()).expect("cursor");
        let mut repeated = ConnectedPaginationBudget::new(2_000, 1_000).expect("budget");
        repeated
            .record_page(1_000, Some(&cursor), 1024)
            .expect("first page");
        assert_eq!(
            repeated.record_page(1, Some(&cursor), 1024),
            Err(MobileEnterpriseError::InvalidResponse)
        );

        let mut oversized = ConnectedPaginationBudget::new(1_000, 1_000).expect("budget");
        assert_eq!(
            oversized.record_page(1_001, None, 1024),
            Err(MobileEnterpriseError::InvalidResponse)
        );
    }

    #[test]
    fn connected_pagination_rejects_a_record_repeated_on_a_later_page() {
        let row = catdb_api_types::EntityRow {
            record_key: "same.md".to_string(),
            fields: std::collections::BTreeMap::new(),
            provenance: std::collections::BTreeMap::new(),
        };
        let mut budget = ConnectedPaginationBudget::new(2_000, 1_000).expect("budget");
        budget
            .record_rows(std::slice::from_ref(&row))
            .expect("first page identity");
        assert_eq!(
            budget.record_rows(&[row]),
            Err(MobileEnterpriseError::InvalidResponse),
            "a fresh cursor cannot replay an already-published record identity"
        );
    }

    #[test]
    fn connected_library_initial_query_rejects_renderer_identity_and_other_entities() {
        let source = connected_source();
        let mut continuation = connected_query_request();
        continuation.query_id = Some(uuid::Uuid::from_u128(77));
        continuation.cursor =
            Some(catdb_api_types::EntityCursor::new("v1.page-two".to_string()).expect("cursor"));
        assert_eq!(
            pin_connected_query(&source, &mut continuation),
            Err(MobileEnterpriseError::InvalidRequest),
            "the renderer cannot turn an initial Library request into a continuation"
        );

        let mut other_entity = connected_query_request();
        other_entity.entity = "crm.Customer".to_string();
        assert_eq!(
            pin_connected_query(&source, &mut other_entity),
            Err(MobileEnterpriseError::InvalidRequest),
            "MDReader's Connected CatDB membership exposes only document.Document"
        );
    }

    #[test]
    fn connected_library_response_cannot_claim_complete_when_a_source_did_not_run() {
        let source = connected_source();
        let response = connected_response(
            &[],
            catdb_api_types::SourceOutcome::Refused,
            catdb_api_types::Completeness::Complete,
        );

        assert_eq!(
            parse_connected_library_response(&source, response, &["title".to_string()]),
            Err(MobileEnterpriseError::InvalidResponse)
        );
    }

    #[test]
    fn connected_library_response_rejects_duplicate_record_identity_in_one_page() {
        let source = connected_source();
        let response = connected_response(
            &["same.md", "same.md"],
            catdb_api_types::SourceOutcome::Ran,
            catdb_api_types::Completeness::Complete,
        );

        assert_eq!(
            parse_connected_library_response(&source, response, &["title".to_string()]),
            Err(MobileEnterpriseError::InvalidResponse)
        );
    }

    #[test]
    fn connected_library_response_rejects_a_different_remote_catalog_pin() {
        let source = connected_source();
        let response = MobileEnterpriseResponse {
            status: 200,
            body_json: serde_json::json!({
                "query_id": "00000000-0000-0000-0000-000000000021",
                "pins": {
                    "catalog_resolver_id": "00000000-0000-0000-0000-000000000022",
                    "catalog_revision_id": "00000000-0000-0000-0000-000000000099",
                    "schema_version_id": "00000000-0000-0000-0000-000000000005",
                    "physical_schema_version_ids": ["00000000-0000-0000-0000-000000000024"],
                    "mapping_version_ids": ["00000000-0000-0000-0000-000000000025"],
                    "source_version_ids": ["00000000-0000-0000-0000-00000000000c"],
                    "policy_version_ids": ["00000000-0000-0000-0000-000000000026"],
                    "provenance_rule_version_id": "00000000-0000-0000-0000-000000000027"
                },
                "rows": [],
                "sources": [{
                    "source_id": "00000000-0000-0000-0000-00000000000b",
                    "source_version_id": "00000000-0000-0000-0000-00000000000c",
                    "outcome": "ran",
                    "retry_after_ms": null,
                    "freshness_ms": 0,
                    "scanned": 0,
                    "returned": 0
                }],
                "completeness": {"state": "complete"},
                "next_cursor": null
            })
            .to_string(),
        };

        assert_eq!(
            parse_connected_library_response(&source, response, &["title".to_string()]),
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
    }

    #[test]
    fn connected_library_response_rejects_a_value_without_field_provenance() {
        let source = connected_source();
        let response = MobileEnterpriseResponse {
            status: 200,
            body_json: serde_json::json!({
                "query_id": "00000000-0000-0000-0000-000000000021",
                "pins": {
                    "catalog_resolver_id": "00000000-0000-0000-0000-000000000022",
                    "catalog_revision_id": "00000000-0000-0000-0000-000000000004",
                    "schema_version_id": "00000000-0000-0000-0000-000000000005",
                    "physical_schema_version_ids": ["00000000-0000-0000-0000-000000000024"],
                    "mapping_version_ids": ["00000000-0000-0000-0000-000000000025"],
                    "source_version_ids": ["00000000-0000-0000-0000-00000000000c"],
                    "policy_version_ids": ["00000000-0000-0000-0000-000000000026"],
                    "provenance_rule_version_id": "00000000-0000-0000-0000-000000000027"
                },
                "rows": [{
                    "record_key": "remote-document",
                    "fields": {"title": "Unproven value"},
                    "provenance": {}
                }],
                "sources": [{
                    "source_id": "00000000-0000-0000-0000-00000000000b",
                    "source_version_id": "00000000-0000-0000-0000-00000000000c",
                    "outcome": "ran",
                    "retry_after_ms": null,
                    "freshness_ms": 0,
                    "scanned": 1,
                    "returned": 1
                }],
                "completeness": {"state": "complete"},
                "next_cursor": null
            })
            .to_string(),
        };

        assert_eq!(
            parse_connected_library_response(&source, response, &["title".to_string()]),
            Err(MobileEnterpriseError::InvalidResponse)
        );
    }

    #[tokio::test]
    async fn native_transport_uses_bearer_auth_and_rejects_the_legacy_browser_header() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0_u8; 16 * 1024];
            let count = stream.read(&mut request).await.expect("read");
            let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
            assert!(request.starts_with("get /v1/session "));
            assert!(request.contains("\r\nauthorization: bearer native-secret\r\n"));
            assert!(!request.contains("x-mdreader-session-reference"));
            let body = r#"{"deployment_id":"deployment-1"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });
        let response = execute_request(
            ConnectionRecord {
                id: "connection-1".to_string(),
                label: "Test".to_string(),
                server_origin: format!("http://{address}"),
                credential_reference: "native-reference".to_string(),
            },
            BearerToken("native-secret".to_string()),
            MobileEnterpriseOperation::Session,
            None,
        )
        .await
        .expect("request");
        assert_eq!(
            response,
            MobileEnterpriseResponse {
                status: 200,
                body_json: r#"{"deployment_id":"deployment-1"}"#.to_string(),
            }
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn native_transport_rejects_renderer_supplied_entity_query_source_ids() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0_u8; 16 * 1024];
            let count = stream.read(&mut request).await.expect("read");
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(
                !request.contains("\"source_ids\":[\"00000000-0000-0000-0000-000000000099\"]"),
                "renderer-selected source authority crossed the native boundary"
            );
            let body = r#"{"rows":[]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });
        let body = serde_json::json!({
            "workspace_id": "00000000-0000-0000-0000-000000000001",
            "entity": "document.Document",
            "select": ["title"],
            "filters": [],
            "order_by": [],
            "source_ids": ["00000000-0000-0000-0000-000000000099"],
            "page_size": 10,
            "limit": 10
        })
        .to_string();
        let result = execute_request(
            ConnectionRecord {
                id: "connection-1".to_string(),
                label: "Test".to_string(),
                server_origin: format!("http://{address}"),
                credential_reference: "native-reference".to_string(),
            },
            BearerToken("native-secret".to_string()),
            MobileEnterpriseOperation::EntityQuery,
            Some(body),
        )
        .await;

        assert_eq!(result, Err(MobileEnterpriseError::InvalidRequest));
        server.abort();
    }

    #[test]
    fn connected_query_pins_native_workspace_revision_and_selected_sources() {
        let source = connected_source();
        let mut request: catdb_api_types::EntityQueryRequest =
            serde_json::from_value(serde_json::json!({
                "workspace_id": "00000000-0000-0000-0000-000000000099",
                "catalog_revision_id": "00000000-0000-0000-0000-000000000098",
                "schema_version_id": null,
                "entity": "document.Document",
                "select": ["title"],
                "filters": [],
                "order_by": [],
                "source_ids": ["00000000-0000-0000-0000-000000000097"],
                "max_staleness_ms": null,
                "require_complete": true,
                "page_size": 10,
                "limit": 10,
                "cursor": null
            }))
            .expect("request");

        let body = pin_connected_query(&source, &mut request).expect("pin query");
        let forwarded: serde_json::Value = serde_json::from_str(&body).expect("forwarded body");

        assert_eq!(
            forwarded["workspace_id"],
            serde_json::json!("00000000-0000-0000-0000-000000000003")
        );
        assert_eq!(
            forwarded["catalog_revision_id"],
            serde_json::json!("00000000-0000-0000-0000-000000000004")
        );
        assert_eq!(
            forwarded["source_ids"],
            serde_json::json!(["00000000-0000-0000-0000-00000000000b"])
        );
        assert_eq!(
            forwarded["schema_version_id"],
            serde_json::json!("00000000-0000-0000-0000-000000000005")
        );
        assert_eq!(forwarded["select"], serde_json::json!(["title"]));
    }

    #[test]
    fn connected_query_rejects_a_renderer_selected_non_default_schema() {
        let source = connected_source();
        let mut request = connected_query_request();
        request.schema_version_id = Some(uuid::Uuid::from_u128(99));

        assert_eq!(
            pin_connected_query(&source, &mut request),
            Err(MobileEnterpriseError::InvalidRequest)
        );
    }

    #[test]
    fn connected_library_response_rejects_a_non_default_schema_pin() {
        let source = connected_source();
        let mut response = connected_response(
            &["remote.md"],
            catdb_api_types::SourceOutcome::Ran,
            catdb_api_types::Completeness::Complete,
        );
        let mut body: catdb_api_types::EntityQueryResponse =
            serde_json::from_str(&response.body_json).expect("response");
        body.pins.schema_version_id = uuid::Uuid::from_u128(99);
        response.body_json = serde_json::to_string(&body).expect("response");

        assert_eq!(
            parse_connected_library_response(&source, response, &["title".to_string()]),
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
    }

    #[test]
    fn connected_library_response_rejects_duplicate_source_version_pins() {
        let source = connected_source();
        let mut response = connected_response(
            &["remote.md"],
            catdb_api_types::SourceOutcome::Ran,
            catdb_api_types::Completeness::Complete,
        );
        let mut body: catdb_api_types::EntityQueryResponse =
            serde_json::from_str(&response.body_json).expect("response");
        body.pins
            .source_version_ids
            .push(body.pins.source_version_ids[0]);
        response.body_json = serde_json::to_string(&body).expect("response");

        assert_eq!(
            parse_connected_library_response(&source, response, &["title".to_string()]),
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
    }

    #[test]
    fn connected_library_response_rejects_distinct_sources_with_one_version_authority() {
        let mut source = connected_source();
        let shared_version = source.permitted_sources[0].source_version_id;
        let second_source = uuid::Uuid::from_u128(13);
        source
            .permitted_sources
            .push(crate::library::ConnectedCatdbSourceDescriptor {
                source_id: second_source,
                source_version_id: shared_version,
                runtime_bound: true,
                connector_id: Some("snowflake-secondary".to_string()),
            });
        source.selected_source_ids.push(second_source);

        let mut response = connected_response(
            &["remote.md"],
            catdb_api_types::SourceOutcome::Ran,
            catdb_api_types::Completeness::Complete,
        );
        let mut body: catdb_api_types::EntityQueryResponse =
            serde_json::from_str(&response.body_json).expect("response");
        let mut second_execution = body.sources[0].clone();
        second_execution.source_id = second_source;
        body.sources.push(second_execution);
        body.pins.source_version_ids.push(shared_version);
        response.body_json = serde_json::to_string(&body).expect("response");

        assert_eq!(
            parse_connected_library_response(&source, response, &["title".to_string()]),
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
    }

    #[test]
    fn connected_query_publication_revalidates_the_exact_connection_record() {
        let expected = connected_source();
        let mut changed_selection = expected.clone();
        changed_selection.selected_source_ids.clear();
        let mut changed_credential = expected.clone();
        changed_credential.credential_reference = "credential-reference-2".to_string();

        assert_eq!(
            validate_current_connected_source(&expected, Some(&expected)),
            Ok(())
        );
        assert_eq!(
            validate_current_connected_source(&expected, Some(&changed_selection)),
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
        assert_eq!(
            validate_current_connected_source(&expected, Some(&changed_credential)),
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
        assert_eq!(
            validate_current_connected_source(&expected, None),
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
    }

    #[test]
    fn connected_query_refuses_an_empty_native_selection() {
        let mut source = connected_source();
        source.selected_source_ids.clear();
        let mut request: catdb_api_types::EntityQueryRequest =
            serde_json::from_value(serde_json::json!({
                "workspace_id": "00000000-0000-0000-0000-000000000099",
                "catalog_revision_id": null,
                "schema_version_id": null,
                "entity": "document.Document",
                "select": ["title"],
                "filters": [],
                "order_by": [],
                "source_ids": null,
                "max_staleness_ms": null,
                "require_complete": true,
                "page_size": 10,
                "limit": 10,
                "cursor": null
            }))
            .expect("request");

        assert_eq!(
            pin_connected_query(&source, &mut request),
            Err(MobileEnterpriseError::NoSourcesSelected)
        );
    }

    #[test]
    fn connected_membership_and_token_have_separate_persistence_lifecycles() {
        let temp = tempfile::tempdir().expect("tempdir");
        let registry_path = temp.path().join("library.json");
        let store = LibraryStore::new(registry_path.clone());
        let vault = MemoryBearerVault::default();
        let source = connected_source();
        let connection_id = source.id.clone();
        let credential_reference = source.credential_reference.clone();

        let state = persist_connected_catdb(
            &store,
            &vault,
            source,
            &BearerToken("native-secret".to_string()),
        )
        .expect("persist");
        assert!(matches!(state, ConnectedCatdbState::Connected { .. }));
        let bytes = std::fs::read_to_string(&registry_path).expect("registry");
        assert!(bytes.contains("\"kind\": \"connected-catdb\""));
        assert!(!bytes.contains("native-secret"));
        assert_eq!(
            vault
                .get(&credential_reference)
                .expect("vault")
                .expect("token")
                .0,
            "native-secret"
        );

        store
            .update_cloud(|_| Ok::<(), crate::cloud_accounts::CloudSourceError>(()))
            .expect("unrelated cloud update");
        assert!(
            store
                .read_connected_catdb()
                .expect("read membership")
                .is_some(),
            "cloud membership updates must retain Connected CatDB"
        );

        disconnect_connected_catdb_from(&store, &vault, &connection_id).expect("disconnect");
        assert!(store
            .read_connected_catdb()
            .expect("read disconnected")
            .is_none());
        assert!(vault
            .get(&credential_reference)
            .expect("read vault")
            .is_none());
    }

    #[test]
    fn failed_membership_admission_rolls_back_the_new_bearer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = LibraryStore::new(temp.path().join("library.json"));
        let vault = MemoryBearerVault::default();
        let mut source = connected_source();
        source.label.clear();
        let credential_reference = source.credential_reference.clone();

        assert_eq!(
            persist_connected_catdb(
                &store,
                &vault,
                source,
                &BearerToken("native-secret".to_string()),
            ),
            Err(MobileEnterpriseError::InvalidConfiguration)
        );
        assert!(vault
            .get(&credential_reference)
            .expect("read vault")
            .is_none());
        assert!(store
            .read_connected_catdb()
            .expect("read membership")
            .is_none());
    }

    #[tokio::test]
    async fn authenticated_first_use_migrates_and_then_retires_the_legacy_registry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let library_path = temp.path().join("library.json");
        let legacy_path = temp.path().join("mobile-enterprise.json");
        let retired_path = temp.path().join("mobile-enterprise.json.migrated");
        let store = LibraryStore::new(library_path);
        let vault = Arc::new(MemoryBearerVault::default());
        vault
            .set(
                "legacy-credential-reference",
                &BearerToken("native-secret".to_string()),
            )
            .expect("seed legacy token");

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "version": 1,
                "active_connection_id": "legacy-connection",
                "connections": [{
                    "id": "legacy-connection",
                    "label": "Existing Enterprise",
                    "server_origin": format!("http://{address}"),
                    "credential_reference": "legacy-credential-reference"
                }]
            })
            .to_string(),
        )
        .expect("write legacy registry");

        let server = tokio::spawn(async move {
            for (expected_path, body) in [
                (
                    "/v1/session",
                    serde_json::json!({
                        "principal_kind": "service",
                        "deployment_id": "00000000-0000-0000-0000-000000000002",
                        "workspace_ids": ["00000000-0000-0000-0000-000000000003"],
                        "scopes": ["query", "catalog_read"]
                    }),
                ),
                (
                    "/v1/control/workspaces/00000000-0000-0000-0000-000000000003/catalog",
                    serde_json::json!({
                        "workspace_id": "00000000-0000-0000-0000-000000000003",
                        "resolver_id": "00000000-0000-0000-0000-000000000005",
                        "revision": {
                            "revision_id": "00000000-0000-0000-0000-000000000004",
                            "parent_revision_id": null,
                            "actor": "test",
                            "timestamp_ms": 1
                        },
                        "default_schema_version_id": "00000000-0000-0000-0000-000000000006",
                        "schemas": [{
                            "schema_version_id": "00000000-0000-0000-0000-000000000006",
                            "name": "document",
                            "content_id": "00000000-0000-0000-0000-000000000007",
                            "compatibility": "compatible",
                            "spec": {}
                        }],
                        "mappings": [],
                        "sources": [{
                            "source_id": "00000000-0000-0000-0000-000000000011",
                            "source_version_id": "00000000-0000-0000-0000-000000000012",
                            "checksum": "checksum",
                            "runtime_bound": true,
                            "connector_id": "postgres-primary"
                        }],
                        "policies": []
                    }),
                ),
            ] {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = vec![0_u8; 16 * 1024];
                let count = stream.read(&mut request).await.expect("read");
                let request = String::from_utf8_lossy(&request[..count]);
                assert!(request.starts_with(&format!("GET {expected_path} ")));
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.expect("write");
            }
        });

        let migrated = migrate_legacy_connection(store.clone(), legacy_path.clone(), vault.clone())
            .await
            .expect("migrate")
            .expect("legacy membership");

        assert_eq!(migrated.label, "Existing Enterprise");
        assert_eq!(migrated.credential_reference, "legacy-credential-reference");
        assert_eq!(migrated.deployment_id, uuid::Uuid::from_u128(2));
        assert_eq!(migrated.workspace_id, uuid::Uuid::from_u128(3));
        assert_eq!(migrated.catalog_revision_id, uuid::Uuid::from_u128(4));
        assert!(migrated.selected_source_ids.is_empty());
        assert_eq!(
            store
                .read_connected_catdb()
                .expect("read migrated membership"),
            Some(migrated)
        );
        assert!(!legacy_path.exists());
        assert!(retired_path.is_file());
        assert_eq!(
            vault
                .get("legacy-credential-reference")
                .expect("read vault")
                .expect("preserved token")
                .0,
            "native-secret"
        );
        server.await.expect("server");
    }

    #[test]
    fn failed_publisher_cannot_erase_membership_after_another_participant_retires_legacy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let library_path = temp.path().join("library.json");
        let legacy_path = temp.path().join("mobile-enterprise.json");
        let retired_path = temp.path().join("mobile-enterprise.json.migrated");
        let publisher = LibraryStore::new(library_path.clone());
        let observer = LibraryStore::new(library_path);
        let source = connected_source();
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "version": 1,
                "active_connection_id": "legacy-connection",
                "connections": [{
                    "id": "legacy-connection",
                    "label": source.label,
                    "server_origin": source.server_origin,
                    "credential_reference": source.credential_reference
                }]
            })
            .to_string(),
        )
        .expect("write legacy registry");

        let result = publish_legacy_migration_transaction_with_rename(
            &publisher,
            &legacy_path,
            source,
            |from, _to| {
                let observed = observer
                    .read_connected_catdb()
                    .expect("independent participant reads membership")
                    .expect("published membership");
                retire_matching_legacy_after_publication(from, &observed)
                    .expect("independent participant retires matching legacy");
                Err(std::io::Error::other("publisher rename lost the race"))
            },
        );

        assert!(result.is_ok() || matches!(result, Err(MobileEnterpriseError::PersistFailed)));
        assert!(
            retired_path.exists(),
            "the observer retired the legacy file"
        );
        assert!(
            legacy_path.exists()
                || publisher
                    .read_connected_catdb()
                    .expect("read final membership")
                    .is_some(),
            "every interleaving must leave authoritative legacy or unified membership"
        );
    }

    #[test]
    fn migration_publication_serializes_independent_retirement_participants() {
        let temp = tempfile::tempdir().expect("tempdir");
        let library_path = temp.path().join("library.json");
        let legacy_path = temp.path().join("mobile-enterprise.json");
        let publisher_path = legacy_path.clone();
        let observer_path = legacy_path.clone();
        let publisher = LibraryStore::new(library_path.clone());
        let observer = LibraryStore::new(library_path);
        let source = connected_source();
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "version": 1,
                "active_connection_id": "legacy-connection",
                "connections": [{
                    "id": "legacy-connection",
                    "label": source.label,
                    "server_origin": source.server_origin,
                    "credential_reference": source.credential_reference
                }]
            })
            .to_string(),
        )
        .expect("write legacy registry");
        let (published_tx, published_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first = std::thread::spawn(move || {
            publish_legacy_migration_with_rename(&publisher, &publisher_path, source, |from, to| {
                published_tx.send(()).expect("signal unified publication");
                release_rx.recv().expect("release legacy retirement");
                std::fs::rename(from, to)
            })
        });
        published_rx
            .recv()
            .expect("publisher added unified membership under lease");

        let (observer_done_tx, observer_done_rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            let result = with_legacy_migration_lock(&observer_path, || {
                let observed = observer
                    .read_connected_catdb()
                    .expect("read published membership")
                    .expect("unified membership");
                retire_matching_legacy_after_publication(&observer_path, &observed)
            });
            observer_done_tx
                .send(())
                .expect("signal observer completion");
            result
        });
        assert!(
            observer_done_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "an independent retirement participant must block until publication and retirement are one outcome"
        );
        release_tx.send(()).expect("release publisher retirement");
        first.join().expect("join publisher").expect("publication");
        second
            .join()
            .expect("join observer")
            .expect("observer retirement");
        assert!(!legacy_path.exists());
    }

    fn wait_for_process_signal(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for subprocess signal {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn_race_participant(
        role: &str,
        root: &std::path::Path,
        connection_id: Option<&str>,
    ) -> std::process::Child {
        let mut command = std::process::Command::new(
            std::env::current_exe().expect("resolve current Rust test executable"),
        );
        command
            .arg("--exact")
            .arg(
                "mobile_enterprise::tests::migration_publication_serializes_production_disconnect_across_processes",
            )
            .arg("--nocapture")
            .env("MDREADER_CONNECTED_CATDB_RACE_ROLE", role)
            .env("MDREADER_CONNECTED_CATDB_RACE_ROOT", root)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        if let Some(connection_id) = connection_id {
            command.env("MDREADER_CONNECTED_CATDB_RACE_CONNECTION_ID", connection_id);
        }
        if role == "disconnect" {
            command.env(
                "MDREADER_CONNECTED_CATDB_DISCONNECT_ENTRY_SIGNAL",
                root.join("disconnect-started"),
            );
        }
        command.spawn().expect("spawn race participant")
    }

    #[test]
    fn migration_publication_serializes_production_disconnect_across_processes() {
        if let Ok(role) = std::env::var("MDREADER_CONNECTED_CATDB_RACE_ROLE") {
            let root = std::path::PathBuf::from(
                std::env::var_os("MDREADER_CONNECTED_CATDB_RACE_ROOT")
                    .expect("race root is provided to subprocess"),
            );
            let store = LibraryStore::new(root.join("library.json"));
            let legacy_path = root.join("mobile-enterprise.json");
            match role.as_str() {
                "publisher" => {
                    let source = connected_source();
                    let connection_id = source.id.clone();
                    let ready_path = root.join("publisher-ready");
                    let release_path = root.join("publisher-release");
                    publish_legacy_migration_with_rename(
                        &store,
                        &legacy_path,
                        source,
                        |from, to| {
                            std::fs::write(&ready_path, &connection_id)
                                .expect("publish connection id signal");
                            wait_for_process_signal(&release_path);
                            std::fs::rename(from, to)
                        },
                    )
                    .expect("publish legacy migration");
                }
                "disconnect" => {
                    let connection_id =
                        std::env::var("MDREADER_CONNECTED_CATDB_RACE_CONNECTION_ID")
                            .expect("disconnect connection id");
                    let vault = MemoryBearerVault::default();
                    assert!(matches!(
                        disconnect_connected_catdb_operation(
                            &store,
                            &vault,
                            &legacy_path,
                            &connection_id,
                        ),
                        Ok(ConnectedCatdbState::Disconnected { .. })
                    ));
                    std::fs::write(root.join("disconnect-finished"), b"finished")
                        .expect("signal disconnect completion");
                }
                unexpected => panic!("unexpected race role {unexpected}"),
            }
            return;
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let legacy_path = temp.path().join("mobile-enterprise.json");
        let retired_path = temp.path().join("mobile-enterprise.json.migrated");
        let source = connected_source();
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "version": 1,
                "active_connection_id": "legacy-connection",
                "connections": [{
                    "id": "legacy-connection",
                    "label": source.label,
                    "server_origin": source.server_origin,
                    "credential_reference": source.credential_reference
                }]
            })
            .to_string(),
        )
        .expect("write legacy registry");

        let mut publisher = spawn_race_participant("publisher", temp.path(), None);
        let ready_path = temp.path().join("publisher-ready");
        wait_for_process_signal(&ready_path);
        let connection_id =
            std::fs::read_to_string(&ready_path).expect("read published connection id");

        let mut disconnect =
            spawn_race_participant("disconnect", temp.path(), Some(&connection_id));
        wait_for_process_signal(&temp.path().join("disconnect-started"));
        let observation_deadline = std::time::Instant::now() + Duration::from_millis(750);
        while std::time::Instant::now() < observation_deadline {
            assert!(
                !temp.path().join("disconnect-finished").exists(),
                "the production disconnect crossed the publisher's process lease"
            );
            assert!(
                disconnect.try_wait().expect("poll disconnect").is_none(),
                "the production disconnect exited instead of waiting for the publisher lease"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        disconnect
            .kill()
            .expect("stop the disconnect participant while it is lease-blocked");
        disconnect.wait().expect("reap disconnect participant");
        std::fs::write(temp.path().join("publisher-release"), b"release")
            .expect("release publisher");
        assert!(
            publisher.wait().expect("wait for publisher").success(),
            "publisher subprocess failed"
        );

        assert!(!legacy_path.exists(), "the legacy source is retired");
        assert!(
            retired_path.exists(),
            "the legacy source is retained as history"
        );
        assert!(
            LibraryStore::new(temp.path().join("library.json"))
                .read_connected_catdb()
                .expect("read final connected membership")
                .is_some(),
            "killing a blocked independent disconnect must not leave neither legacy nor unified authority"
        );
    }

    #[test]
    fn failed_retirement_preserves_legacy_and_a_retry_publishes_unified_membership() {
        let temp = tempfile::tempdir().expect("tempdir");
        let legacy_path = temp.path().join("mobile-enterprise.json");
        let retired_path = temp.path().join("mobile-enterprise.json.migrated");
        let store = LibraryStore::new(temp.path().join("library.json"));
        let source = connected_source();
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "version": 1,
                "active_connection_id": "legacy-connection",
                "connections": [{
                    "id": "legacy-connection",
                    "label": source.label,
                    "server_origin": source.server_origin,
                    "credential_reference": source.credential_reference
                }]
            })
            .to_string(),
        )
        .expect("write legacy registry");

        assert_eq!(
            publish_legacy_migration_transaction_with_rename(
                &store,
                &legacy_path,
                source.clone(),
                |_from, _to| Err(std::io::Error::other("injected rename failure")),
            ),
            Err(MobileEnterpriseError::PersistFailed)
        );
        assert!(
            legacy_path.exists(),
            "failed retirement keeps legacy authoritative"
        );
        assert!(!retired_path.exists());
        assert!(
            store
                .read_connected_catdb()
                .expect("read rolled-back membership")
                .is_none(),
            "rollback must not leave a second authoritative membership"
        );

        assert_eq!(
            publish_legacy_migration(&store, &legacy_path, source.clone())
                .expect("retry migration"),
            source
        );
        assert!(!legacy_path.exists());
        assert!(retired_path.exists());
        assert!(
            store
                .read_connected_catdb()
                .expect("read retried membership")
                .is_some(),
            "retry must publish unified membership before retiring legacy"
        );
    }

    #[tokio::test]
    async fn connected_query_refuses_a_changed_authenticated_deployment_before_querying() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept session");
            let mut request = vec![0_u8; 16 * 1024];
            let count = stream.read(&mut request).await.expect("read session");
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /v1/session "));
            let body = serde_json::json!({
                "principal_kind": "service",
                "deployment_id": "00000000-0000-0000-0000-000000000099",
                "workspace_ids": ["00000000-0000-0000-0000-000000000003"],
                "scopes": ["query", "catalog_read"]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write session");
            assert!(
                tokio::time::timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "entity query ran after deployment identity changed"
            );
        });
        let mut source = connected_source();
        source.server_origin = format!("http://{address}/");
        let request: catdb_api_types::EntityQueryRequest =
            serde_json::from_value(serde_json::json!({
                "workspace_id": "00000000-0000-0000-0000-000000000099",
                "catalog_revision_id": null,
                "schema_version_id": null,
                "entity": "document.Document",
                "select": ["title"],
                "filters": [],
                "order_by": [],
                "source_ids": null,
                "max_staleness_ms": null,
                "require_complete": true,
                "page_size": 10,
                "limit": 10,
                "cursor": null
            }))
            .expect("request");

        assert_eq!(
            execute_connected_query(&source, &BearerToken("native-secret".to_string()), request)
                .await,
            Err(MobileEnterpriseError::ConnectionIdentityChanged)
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn enrollment_uses_session_then_authorized_catalog_descriptors() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            for (expected_path, body) in [
                (
                    "/v1/session",
                    serde_json::json!({
                        "principal_kind": "service",
                        "deployment_id": "00000000-0000-0000-0000-000000000002",
                        "workspace_ids": ["00000000-0000-0000-0000-000000000003"],
                        "scopes": ["query", "catalog_read"]
                    }),
                ),
                (
                    "/v1/control/workspaces/00000000-0000-0000-0000-000000000003/catalog",
                    serde_json::json!({
                        "workspace_id": "00000000-0000-0000-0000-000000000003",
                        "resolver_id": "00000000-0000-0000-0000-000000000005",
                        "revision": {
                            "revision_id": "00000000-0000-0000-0000-000000000004",
                            "parent_revision_id": null,
                            "actor": "test",
                            "timestamp_ms": 1
                        },
                        "default_schema_version_id": "00000000-0000-0000-0000-000000000006",
                        "schemas": [{
                            "schema_version_id": "00000000-0000-0000-0000-000000000006",
                            "name": "document",
                            "content_id": "00000000-0000-0000-0000-000000000007",
                            "compatibility": "compatible",
                            "spec": {}
                        }],
                        "mappings": [],
                        "sources": [{
                            "source_id": "00000000-0000-0000-0000-000000000011",
                            "source_version_id": "00000000-0000-0000-0000-000000000012",
                            "checksum": "checksum",
                            "runtime_bound": true,
                            "connector_id": "postgres-primary"
                        }],
                        "policies": []
                    }),
                ),
            ] {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = vec![0_u8; 16 * 1024];
                let count = stream.read(&mut request).await.expect("read");
                let request = String::from_utf8_lossy(&request[..count]);
                assert!(request.starts_with(&format!("GET {expected_path} ")));
                assert!(request
                    .to_ascii_lowercase()
                    .contains("\r\nauthorization: bearer native-secret\r\n"));
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.expect("write");
            }
        });

        let source = discover_connected_catdb(
            url::Url::parse(&format!("http://{address}")).expect("origin"),
            &BearerToken("native-secret".to_string()),
        )
        .await
        .expect("discover");

        assert_eq!(source.workspace_id, uuid::Uuid::from_u128(3));
        assert_eq!(source.catalog_revision_id, uuid::Uuid::from_u128(4));
        assert_eq!(source.permitted_sources.len(), 1);
        assert!(source.selected_source_ids.is_empty());
        server.await.expect("server");
    }

    #[tokio::test]
    async fn enrollment_rejects_distinct_sources_with_one_version_authority() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            for (expected_path, body) in [
                (
                    "/v1/session",
                    serde_json::json!({
                        "principal_kind": "service",
                        "deployment_id": "00000000-0000-0000-0000-000000000002",
                        "workspace_ids": ["00000000-0000-0000-0000-000000000003"],
                        "scopes": ["query", "catalog_read"]
                    }),
                ),
                (
                    "/v1/control/workspaces/00000000-0000-0000-0000-000000000003/catalog",
                    serde_json::json!({
                        "workspace_id": "00000000-0000-0000-0000-000000000003",
                        "resolver_id": "00000000-0000-0000-0000-000000000005",
                        "revision": {
                            "revision_id": "00000000-0000-0000-0000-000000000004",
                            "parent_revision_id": null,
                            "actor": "test",
                            "timestamp_ms": 1
                        },
                        "default_schema_version_id": "00000000-0000-0000-0000-000000000006",
                        "schemas": [{
                            "schema_version_id": "00000000-0000-0000-0000-000000000006",
                            "name": "document",
                            "content_id": "00000000-0000-0000-0000-000000000007",
                            "compatibility": "compatible",
                            "spec": {}
                        }],
                        "mappings": [],
                        "sources": [
                            {
                                "source_id": "00000000-0000-0000-0000-000000000011",
                                "source_version_id": "00000000-0000-0000-0000-000000000012",
                                "checksum": "checksum-a",
                                "runtime_bound": true,
                                "connector_id": "postgres-primary"
                            },
                            {
                                "source_id": "00000000-0000-0000-0000-000000000013",
                                "source_version_id": "00000000-0000-0000-0000-000000000012",
                                "checksum": "checksum-b",
                                "runtime_bound": true,
                                "connector_id": "snowflake-secondary"
                            }
                        ],
                        "policies": []
                    }),
                ),
            ] {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = vec![0_u8; 16 * 1024];
                let count = stream.read(&mut request).await.expect("read");
                assert!(String::from_utf8_lossy(&request[..count])
                    .starts_with(&format!("GET {expected_path} ")));
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.expect("write");
            }
        });

        assert_eq!(
            discover_connected_catdb(
                url::Url::parse(&format!("http://{address}")).expect("origin"),
                &BearerToken("native-secret".to_string()),
            )
            .await,
            Err(MobileEnterpriseError::InvalidResponse)
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn enrollment_refuses_a_session_without_catalog_read_scope() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0_u8; 16 * 1024];
            let count = stream.read(&mut request).await.expect("read");
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /v1/session "));
            let body = serde_json::json!({
                "principal_kind": "service",
                "deployment_id": "00000000-0000-0000-0000-000000000002",
                "workspace_ids": ["00000000-0000-0000-0000-000000000003"],
                "scopes": ["query"]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.expect("write");
            assert!(
                tokio::time::timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "catalog discovery ran without catalog_read"
            );
        });

        assert_eq!(
            discover_connected_catdb(
                url::Url::parse(&format!("http://{address}")).expect("origin"),
                &BearerToken("native-secret".to_string()),
            )
            .await,
            Err(MobileEnterpriseError::PermissionDenied)
        );
        server.await.expect("server");
    }

    #[tokio::test]
    async fn native_transport_does_not_follow_server_redirects() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0_u8; 16 * 1024];
            let count = stream.read(&mut request).await.expect("read");
            let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
            assert!(request.contains("\r\nauthorization: bearer native-secret\r\n"));
            let body = r#"{"redirected":false}"#;
            let response = format!(
                "HTTP/1.1 302 Found\r\nlocation: http://{address}/credential-leak\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.expect("write");
            assert!(
                tokio::time::timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err(),
                "native transport followed a redirect"
            );
        });
        let response = execute_request(
            ConnectionRecord {
                id: "connection-1".to_string(),
                label: "Test".to_string(),
                server_origin: format!("http://{address}"),
                credential_reference: "native-reference".to_string(),
            },
            BearerToken("native-secret".to_string()),
            MobileEnterpriseOperation::Session,
            None,
        )
        .await
        .expect("request");
        assert_eq!(response.status, 302);
        server.await.expect("server");
    }
}
