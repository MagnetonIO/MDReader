//! Native, bearer-authenticated transport for the mobile Enterprise source.
//!
//! The webview names an admitted connection and supplies only a generated
//! CatDB request body. The native layer resolves the origin and bearer token,
//! restricts calls to the session/entity-query contract, and bounds the
//! response before returning JSON to the renderer.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use url::Url;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use zeroize::Zeroizing;
use zeroize::{Zeroize, ZeroizeOnDrop};

const REGISTRY_VERSION: u32 = 1;
const MAX_CONNECTIONS: usize = 16;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 3 * 1024 * 1024;
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

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum MobileEnterpriseError {
    NotConfigured,
    ConnectionNotFound,
    CredentialUnavailable,
    InvalidConfiguration,
    InvalidRequest,
    TransportUnavailable,
    TimedOut,
    ResponseTooLarge,
    InvalidResponse,
}

impl fmt::Display for MobileEnterpriseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotConfigured => "mobile-enterprise-not-configured",
            Self::ConnectionNotFound => "mobile-enterprise-connection-not-found",
            Self::CredentialUnavailable => "mobile-enterprise-credential-unavailable",
            Self::InvalidConfiguration => "mobile-enterprise-invalid-configuration",
            Self::InvalidRequest => "mobile-enterprise-invalid-request",
            Self::TransportUnavailable => "mobile-enterprise-transport-unavailable",
            Self::TimedOut => "mobile-enterprise-timeout",
            Self::ResponseTooLarge => "mobile-enterprise-response-too-large",
            Self::InvalidResponse => "mobile-enterprise-invalid-response",
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionRegistry {
    version: u32,
    active_connection_id: Option<String>,
    connections: Vec<ConnectionRecord>,
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

fn read_registry(path: &Path) -> Result<ConnectionRegistry, MobileEnterpriseError> {
    let bytes = std::fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            MobileEnterpriseError::NotConfigured
        } else {
            MobileEnterpriseError::InvalidConfiguration
        }
    })?;
    if bytes.len() > 64 * 1024 {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    let registry: ConnectionRegistry =
        serde_json::from_slice(&bytes).map_err(|_| MobileEnterpriseError::InvalidConfiguration)?;
    if registry.version != REGISTRY_VERSION || registry.connections.len() > MAX_CONNECTIONS {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    for connection in &registry.connections {
        validate_record(connection)?;
    }
    let unique = registry
        .connections
        .iter()
        .map(|connection| connection.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != registry.connections.len()
        || registry
            .active_connection_id
            .as_ref()
            .is_some_and(|active| {
                registry
                    .connections
                    .iter()
                    .filter(|connection| &connection.id == active)
                    .count()
                    != 1
            })
    {
        return Err(MobileEnterpriseError::InvalidConfiguration);
    }
    Ok(registry)
}

fn registry_path(app: &AppHandle) -> Result<PathBuf, MobileEnterpriseError> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("mobile-enterprise.json"))
        .map_err(|_| MobileEnterpriseError::InvalidConfiguration)
}

fn active_record(path: &Path) -> Result<ConnectionRecord, MobileEnterpriseError> {
    let registry = read_registry(path)?;
    let active = registry
        .active_connection_id
        .ok_or(MobileEnterpriseError::NotConfigured)?;
    registry
        .connections
        .into_iter()
        .find(|connection| connection.id == active)
        .ok_or(MobileEnterpriseError::InvalidConfiguration)
}

fn record_for(path: &Path, connection_id: &str) -> Result<ConnectionRecord, MobileEnterpriseError> {
    if !valid_identifier(connection_id, 200) {
        return Err(MobileEnterpriseError::ConnectionNotFound);
    }
    let registry = read_registry(path)?;
    registry
        .connections
        .into_iter()
        .find(|connection| connection.id == connection_id)
        .ok_or(MobileEnterpriseError::ConnectionNotFound)
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

async fn execute_request(
    record: ConnectionRecord,
    token: BearerToken,
    operation: MobileEnterpriseOperation,
    body_json: Option<String>,
) -> Result<MobileEnterpriseResponse, MobileEnterpriseError> {
    validate_record(&record)?;
    validate_bearer(&token.0)?;
    let (path, method, body) = match (operation, body_json) {
        (MobileEnterpriseOperation::Session, None) => ("/v1/session", reqwest::Method::GET, None),
        (MobileEnterpriseOperation::EntityQuery, Some(body))
            if !body.is_empty() && body.len() <= MAX_REQUEST_BYTES =>
        {
            let parsed: serde_json::Value =
                serde_json::from_str(&body).map_err(|_| MobileEnterpriseError::InvalidRequest)?;
            if !parsed.is_object() {
                return Err(MobileEnterpriseError::InvalidRequest);
            }
            ("/v1/entity/query", reqwest::Method::POST, Some(body))
        }
        _ => return Err(MobileEnterpriseError::InvalidRequest),
    };
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
    let record = match active_record(&registry_path(&app)?) {
        Ok(record) => record,
        Err(MobileEnterpriseError::NotConfigured) => return Ok(None),
        Err(error) => return Err(error),
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
    let (record, token) = {
        if connection_id != E2E_CONNECTION_ID {
            return Err(MobileEnterpriseError::ConnectionNotFound);
        }
        (e2e_record(), BearerToken(E2E_BEARER.to_string()))
    };
    #[cfg(not(feature = "e2e-build"))]
    let (record, token) = {
        let record = record_for(&registry_path(&app)?, &connection_id)?;
        let vault = KeyringBearerVault::new(&app.config().identifier)?;
        let token = vault
            .get(&record.credential_reference)?
            .ok_or(MobileEnterpriseError::CredentialUnavailable)?;
        (record, token)
    };
    let _ = app;
    execute_request(record, token, operation, body_json).await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        execute_request, BearerToken, ConnectionRecord, MobileEnterpriseOperation,
        MobileEnterpriseResponse,
    };
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

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
