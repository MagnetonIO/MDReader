//! Native, bearer-authenticated transport for the mobile Enterprise source.
//!
//! The webview names an admitted connection and supplies only a generated
//! CatDB request body. The native layer resolves the origin and bearer token,
//! restricts calls to the session/entity-query contract, and bounds the
//! response before returning JSON to the renderer.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use catdb_api_types::{ControlPlaneSnapshot, EntityQueryRequest, SessionResponse};
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
const LEGACY_REGISTRY_VERSION: u32 = 1;
const REQUEST_DEADLINE: Duration = Duration::from_secs(15);

#[cfg(feature = "e2e-build")]
const E2E_CONNECTION_ID: &str = "mdreader-e2e";
#[cfg(feature = "e2e-build")]
const E2E_CONNECTION_LABEL: &str = "Isolated Enterprise fixture";
#[cfg(feature = "e2e-build")]
const E2E_ORIGIN: &str = "http://127.0.0.1:47865";
#[cfg(feature = "e2e-build")]
const E2E_BEARER: &str = "mdreader-e2e-bearer";

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

#[derive(Default)]
pub(crate) struct MobileEnterpriseState(tokio::sync::Mutex<()>);

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
fn e2e_record() -> ConnectionRecord {
    ConnectionRecord {
        id: E2E_CONNECTION_ID.to_string(),
        label: E2E_CONNECTION_LABEL.to_string(),
        server_origin: E2E_ORIGIN.to_string(),
        credential_reference: "e2e-compiled-isolated-token".to_string(),
    }
}

async fn execute_http(
    record: &ConnectionRecord,
    token: &BearerToken,
    method: reqwest::Method,
    path: &str,
    body: Option<String>,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    validate_record(&record)?;
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
        || catalog.sources.len() > MAX_CONNECTED_SOURCES
    {
        return Err(MobileEnterpriseError::InvalidResponse);
    }
    let mut source_ids = std::collections::HashSet::new();
    let permitted_sources = catalog
        .sources
        .into_iter()
        .map(|source| {
            if source.source_id.is_nil()
                || source.source_version_id.is_nil()
                || !source_ids.insert(source.source_id)
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
        session.deployment_id,
        workspace_id,
        catalog.revision.revision_id,
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

fn pin_connected_query(
    source: &crate::library::ConnectedCatdbSourceRecord,
    request: &mut EntityQueryRequest,
) -> Result<String, MobileEnterpriseError> {
    if source.selected_source_ids().is_empty() {
        return Err(MobileEnterpriseError::NoSourcesSelected);
    }
    request.workspace_id = source.workspace_id;
    request.catalog_revision_id = Some(source.catalog_revision_id);
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
    mut request: EntityQueryRequest,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    let body = pin_connected_query(source, &mut request)?;
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

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn connected_catdb_state(
    app: AppHandle,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    let _operation = state.0.lock().await;
    #[cfg(mobile)]
    {
        let _ = app;
        return Ok(disconnected_state());
    }
    #[cfg(not(mobile))]
    {
        connected_source_or_migrate(&app).await.map(|source| {
            source
                .as_ref()
                .map_or_else(disconnected_state, connected_state)
        })
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn enroll_connected_catdb(
    app: AppHandle,
    server_origin: String,
    bearer_token: String,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    let _operation = state.0.lock().await;
    #[cfg(mobile)]
    {
        let _ = (app, server_origin, bearer_token);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let server_origin = exact_origin(&server_origin)?;
        let token = BearerToken(bearer_token);
        validate_bearer(&token.0)?;
        if connected_source_or_migrate(&app).await?.is_some() {
            return Err(MobileEnterpriseError::AlreadyConfigured);
        }
        let source = discover_connected_catdb(server_origin, &token).await?;
        let store = library_store(&app)?;
        let vault = KeyringBearerVault::new(&app.config().identifier)?;
        tokio::task::spawn_blocking(move || persist_connected_catdb(&store, &vault, source, &token))
            .await
            .map_err(|_| MobileEnterpriseError::PersistFailed)?
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
    let _operation = state.0.lock().await;
    #[cfg(mobile)]
    {
        let _ = (app, connection_id, source_id, enabled);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let store = library_store(&app)?;
        tokio::task::spawn_blocking(move || {
            store
                .set_connected_catdb_source_enabled(&connection_id, source_id, enabled)
                .map_err(map_library_error)
                .map(|source| connected_state(&source))
        })
        .await
        .map_err(|_| MobileEnterpriseError::PersistFailed)?
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn disconnect_connected_catdb(
    app: AppHandle,
    connection_id: String,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<ConnectedCatdbState, MobileEnterpriseError> {
    let _operation = state.0.lock().await;
    #[cfg(mobile)]
    {
        let _ = (app, connection_id);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let store = library_store(&app)?;
        let vault = KeyringBearerVault::new(&app.config().identifier)?;
        let legacy_path = legacy_registry_path(&app)?;
        tokio::task::spawn_blocking(move || {
            with_legacy_migration_lock(&legacy_path, || {
                disconnect_connected_catdb_from(&store, &vault, &connection_id)
            })
        })
        .await
        .map_err(|_| MobileEnterpriseError::PersistFailed)?
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
    let _operation = state.0.lock().await;
    #[cfg(mobile)]
    {
        let _ = (app, connection_id, request);
        return Err(MobileEnterpriseError::InvalidRequest);
    }
    #[cfg(not(mobile))]
    {
        let identifier = app.config().identifier.clone();
        let source = connected_source_or_migrate(&app)
            .await?
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
        execute_connected_query(&source, &token, request).await
    }
}

#[tauri::command(async)]
#[specta::specta]
pub(crate) async fn mobile_enterprise_connection(
    app: AppHandle,
    state: tauri::State<'_, MobileEnterpriseState>,
) -> Result<Option<MobileEnterpriseConnection>, MobileEnterpriseError> {
    let _operation = state.0.lock().await;
    #[cfg(feature = "e2e-build")]
    let record = e2e_record();
    #[cfg(not(feature = "e2e-build"))]
    #[cfg(mobile)]
    let record: ConnectionRecord = return Ok(None);
    #[cfg(not(feature = "e2e-build"))]
    #[cfg(not(mobile))]
    let record = {
        let source = connected_source_or_migrate(&app).await?;
        let Some(source) = source else {
            return Ok(None);
        };
        ConnectionRecord::from(&source)
    };
    let _ = app;
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
    let _operation = state.0.lock().await;
    #[cfg(feature = "e2e-build")]
    let (record, token, connected_source) = {
        if connection_id != E2E_CONNECTION_ID {
            return Err(MobileEnterpriseError::ConnectionNotFound);
        }
        (
            e2e_record(),
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
            let source = connected_source_or_migrate(&app)
                .await?
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
            return execute_connected_query(&source, &token, request).await;
        }
    }
    execute_request(record, token, operation, body_json).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        disconnect_connected_catdb_from, discover_connected_catdb, execute_connected_query,
        execute_request, migrate_legacy_connection, persist_connected_catdb, pin_connected_query,
        publish_legacy_migration, publish_legacy_migration_transaction_with_rename,
        publish_legacy_migration_with_rename, retire_matching_legacy_after_publication,
        with_legacy_migration_lock, BearerToken, BearerVault, ConnectedCatdbState,
        ConnectionRecord, MobileEnterpriseError, MobileEnterpriseOperation,
        MobileEnterpriseResponse,
    };
    use crate::library::{
        ConnectedCatdbSourceDescriptor, ConnectedCatdbSourceRecord, LibraryStore,
    };
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

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

    fn connected_source() -> ConnectedCatdbSourceRecord {
        let mut source = ConnectedCatdbSourceRecord::new(
            "catdb.example.com".to_string(),
            "https://catdb.example.com/".to_string(),
            "credential-reference-1".to_string(),
            uuid::Uuid::from_u128(2),
            uuid::Uuid::from_u128(3),
            uuid::Uuid::from_u128(4),
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
        assert_eq!(forwarded["select"], serde_json::json!(["title"]));
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
                        "schemas": [],
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
                        "schemas": [],
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
