//! Native-only fresh OAuth + narrow selection. This module never writes a vault,
//! serializes a grant for the renderer, or upgrades/reuses a legacy broad grant.
//! The caller owns registration/configuration and vault-before-service admission.
//!
//! Protocol references (checked 2026-10-07):
//! https://developers.google.com/workspace/drive/picker/guides/desktop-mobile-picker
//! https://developers.google.com/identity/protocols/oauth2/resources/loopback-migration
//! https://docs.dropboxapi.com/dropbox-api/docs/get-started/authorization

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mdreader_credentials::{
    random_key, DriveFile, NativeSelection, ProviderGrant, Registration, SecretBundle,
};
use mdreader_provider::{
    transport::{self, Method, Request, Transport},
    AccountBinding, DisplayText, IoContext, ProviderError, ProviderKind, RecordKey, Result,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::Url;
use zeroize::{Zeroize, Zeroizing};
#[path = "provider_oauth/diagnostics.rs"]
mod diagnostics;
pub(crate) use diagnostics::AuthorizationFailure;
pub use diagnostics::{OAuthDiagnostic, OAuthPhase};
#[cfg(test)]
use diagnostics::OAuthRejection;
type AuthorizationResult<T> = std::result::Result<T, AuthorizationFailure>;

#[cfg(mobile)]
#[path = "provider_oauth/mobile.rs"]
mod mobile;
#[cfg(mobile)]
pub(crate) use mobile::{
    authorize as authorize_mobile, validate_redirect as validate_mobile_redirect,
};

const DRIVE_SCOPE: &str = mdreader_credentials::DRIVE_FILE_SCOPE;
const GOOGLE_AUTHORIZATION_ISSUER: &str = "https://accounts.google.com";
const DROPBOX_SCOPES: &str = mdreader_credentials::DROPBOX_READ_SCOPES;
const MAX_CALLBACK_BYTES: usize = 16 * 1024;
const MAX_OAUTH_RESPONSE_BYTES: usize = 64 * 1024;
// A renderer cannot accumulate pending listeners/browser flows in this process.
static AUTHORIZATION_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Implemented by the native system-browser opener. No embedded webview OAuth.
#[async_trait::async_trait]
pub(crate) trait SystemBrowser: Send + Sync {
    async fn open(&self, url: &Url) -> Result<()>;
}

/// Constructed by native configuration, never deserialized from renderer input.
/// Dropbox's redirect must be registered exactly in the reviewed app console.
pub(crate) struct OAuthConfig {
    pub(crate) dropbox_registered_redirect: Option<String>,
}

/// Native-only result. Caller writes a fresh v3 vault reference before service
/// admission. A confirmed pre-admission failure may clean that reference; after
/// lost IPC/timeout, retain it and reconcile idempotently by credential reference
/// and configuration digest because admission may have committed. No legacy
/// vault is consulted.
pub(crate) struct CompletedAuthorization {
    pub(crate) grant: ProviderGrant,
    pub(crate) name: DisplayText,
}
impl std::fmt::Debug for CompletedAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompletedAuthorization([redacted])")
    }
}

struct PendingAuthorization {
    registration: Registration,
    redirect: Url,
    state: Zeroizing<String>,
    verifier: Zeroizing<String>,
    preselected: Option<NativeSelection>,
    allow_editing: bool,
}
struct VerifiedCallback {
    code: Zeroizing<String>,
    selection: NativeSelection,
}
impl std::fmt::Debug for VerifiedCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedCallback([redacted])")
    }
}

pub(crate) fn validate_redirect(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| ProviderError::Invalid)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_none_or(|port| port == 0)
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "/oauth/callback" | "/oauth/dropbox")
    {
        return Err(ProviderError::Invalid);
    }
    Ok(url)
}

impl PendingAuthorization {
    #[cfg(test)]
    fn new(
        registration: Registration,
        redirect: &str,
        selection: Option<NativeSelection>,
    ) -> Result<Self> {
        Self::with_permissions(registration, redirect, selection, false)
    }
    fn with_permissions(
        registration: Registration,
        redirect: &str,
        selection: Option<NativeSelection>,
        allow_editing: bool,
    ) -> Result<Self> {
        Self::with_redirect(
            registration,
            validate_redirect(redirect)?,
            selection,
            allow_editing,
        )
    }
    fn with_redirect(
        registration: Registration,
        redirect: Url,
        selection: Option<NativeSelection>,
        allow_editing: bool,
    ) -> Result<Self> {
        if allow_editing
            && registration.provider() == ProviderKind::GoogleDrive
            && !matches!(selection, Some(NativeSelection::DriveFolder { .. }))
        {
            return Err(ProviderError::Invalid);
        }
        match registration.provider() {
            ProviderKind::GoogleDrive => {
                if let Some(selected) = &selection {
                    selected.validate()?;
                    if !matches!(
                        selected,
                        NativeSelection::DriveReadOnlyFolder { .. }
                            | NativeSelection::DriveFolder { .. }
                    ) {
                        return Err(ProviderError::Invalid);
                    }
                }
            }
            ProviderKind::Dropbox => {
                let selected = selection.as_ref().ok_or(ProviderError::Invalid)?;
                selected.validate()?;
                if selected.provider() != ProviderKind::Dropbox {
                    return Err(ProviderError::IdentityMismatch);
                }
            }
        }
        Ok(Self {
            registration,
            redirect,
            state: Zeroizing::new(URL_SAFE_NO_PAD.encode(random_key()?)),
            verifier: Zeroizing::new(URL_SAFE_NO_PAD.encode(random_key()?)),
            preselected: selection,
            allow_editing,
        })
    }
    fn expected_scopes(&self) -> &'static str {
        match self.registration.provider() {
            ProviderKind::GoogleDrive if self.allow_editing => {
                mdreader_credentials::DRIVE_EDIT_SCOPE
            }
            ProviderKind::GoogleDrive if self.preselected.is_some() => {
                mdreader_credentials::DRIVE_READONLY_SCOPE
            }
            ProviderKind::GoogleDrive => DRIVE_SCOPE,
            ProviderKind::Dropbox if self.allow_editing => {
                mdreader_credentials::DROPBOX_EDIT_SCOPES
            }
            ProviderKind::Dropbox => DROPBOX_SCOPES,
        }
    }
    fn authorization_url(&self) -> Result<Url> {
        let (base, scope) = match self.registration.provider() {
            ProviderKind::GoogleDrive => (
                "https://accounts.google.com/o/oauth2/v2/auth",
                self.expected_scopes(),
            ),
            ProviderKind::Dropbox => (
                "https://www.dropbox.com/oauth2/authorize",
                self.expected_scopes(),
            ),
        };
        let mut url = Url::parse(base).map_err(|_| ProviderError::Invalid)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()));
        let mut query = url.query_pairs_mut();
        query
            .append_pair("client_id", self.registration.client_id())
            .append_pair("redirect_uri", self.redirect.as_str())
            .append_pair("response_type", "code")
            .append_pair("scope", scope)
            .append_pair("state", &self.state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        match self.registration.provider() {
            ProviderKind::GoogleDrive => {
                query
                    .append_pair("access_type", "offline")
                    .append_pair("prompt", "consent")
                    .append_pair("include_granted_scopes", "false");
                if self.preselected.is_none() {
                    query
                        .append_pair("trigger_onepick", "true")
                        .append_pair("allow_multiple", "true")
                        .append_pair("allow_folder_selection", "false");
                }
            }
            ProviderKind::Dropbox => {
                query
                    .append_pair("token_access_type", "offline")
                    .append_pair("force_reapprove", "true");
            }
        }
        drop(query);
        Ok(url)
    }
    fn verify_callback(&self, target: &str) -> Result<VerifiedCallback> {
        if target.len() > MAX_CALLBACK_BYTES
            || !target.starts_with('/')
            || target.starts_with("//")
            || target.contains('#')
            || target.bytes().any(|b| b.is_ascii_control())
            || !valid_percent_encoding(target.as_bytes())
        {
            return Err(ProviderError::Protocol);
        }
        let (path, query) = target.split_once('?').ok_or(ProviderError::Protocol)?;
        if path != self.redirect.path() {
            return Err(ProviderError::Protocol);
        }
        let mut pairs = BTreeMap::new();
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if !matches!(
                key.as_ref(),
                "state"
                    | "code"
                    | "scope"
                    | "picked_file_ids"
                    | "error"
                    | "error_description"
                    | "error_uri"
                    | "authuser"
                    | "prompt"
                    | "iss"
            ) || pairs
                .insert(key.into_owned(), Zeroizing::new(value.into_owned()))
                .is_some()
            {
                return Err(ProviderError::Protocol);
            }
        }
        let state = pairs.remove("state").ok_or(ProviderError::Unauthorized)?;
        if !same_secret(state.as_bytes(), self.state.as_bytes()) {
            return Err(ProviderError::Unauthorized);
        }
        // Google adds the RFC 9207 authorization-server issuer to its redirect.
        // It is an exact provider binding, never an ignored query extension or
        // an issuer URL chosen by the callback. Older Picker responses omit it.
        // https://developers.google.com/identity/openid-connect/reference#response-parameters
        if let Some(issuer) = pairs.remove("iss") {
            if self.registration.provider() != ProviderKind::GoogleDrive
                || issuer.as_str() != GOOGLE_AUTHORIZATION_ISSUER
            {
                return Err(ProviderError::Unauthorized);
            }
        }
        if pairs.contains_key("error") {
            return Err(ProviderError::Unauthorized);
        }
        let code = pairs.remove("code").ok_or(ProviderError::Protocol)?;
        if code.is_empty() || code.len() > 8192 || code.chars().any(char::is_control) {
            return Err(ProviderError::Protocol);
        }
        let selection = match self.registration.provider() {
            ProviderKind::GoogleDrive => {
                if !exact_scopes(
                    pairs.get("scope").map(|s| s.as_str()).unwrap_or(""),
                    self.expected_scopes(),
                ) {
                    return Err(ProviderError::Unauthorized);
                }
                if let Some(selected) = &self.preselected {
                    if pairs.contains_key("picked_file_ids") {
                        return Err(ProviderError::Protocol);
                    }
                    return Ok(VerifiedCallback {
                        code,
                        selection: selected.clone(),
                    });
                }
                let picked = pairs
                    .remove("picked_file_ids")
                    .ok_or(ProviderError::Unauthorized)?;
                let files = picked
                    .split(',')
                    .map(|id| {
                        Ok(DriveFile {
                            id: RecordKey::new(id)?,
                            resource_key: None,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let selected = NativeSelection::DriveFiles { files };
                selected.validate()?;
                selected
            }
            ProviderKind::Dropbox => {
                if pairs.contains_key("picked_file_ids") {
                    return Err(ProviderError::Protocol);
                }
                if let Some(scope) = pairs.get("scope") {
                    if !exact_scopes(scope, self.expected_scopes()) {
                        return Err(ProviderError::Unauthorized);
                    }
                }
                self.preselected.clone().ok_or(ProviderError::Invalid)?
            }
        };
        Ok(VerifiedCallback { code, selection })
    }
    /// Consuming pending state ensures a successful callback is exchanged once.
    #[cfg(test)]
    async fn exchange(
        self,
        callback: VerifiedCallback,
        transport: &dyn Transport,
        context: &IoContext,
    ) -> Result<CompletedAuthorization> {
        self.exchange_detailed(callback, transport, context)
            .await
            .map_err(|failure| failure.error)
    }
    async fn exchange_detailed(
        self,
        callback: VerifiedCallback,
        transport: &dyn Transport,
        context: &IoContext,
    ) -> AuthorizationResult<CompletedAuthorization> {
        let endpoint = match self.registration.provider() {
            ProviderKind::GoogleDrive => "https://oauth2.googleapis.com/token",
            ProviderKind::Dropbox => "https://api.dropboxapi.com/oauth2/token",
        };
        let form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "authorization_code")
            .append_pair("client_id", self.registration.client_id())
            .append_pair("redirect_uri", self.redirect.as_str())
            .append_pair("code", &callback.code)
            .append_pair("code_verifier", &self.verifier)
            .finish();
        let request = Request::new(
            Method::Post,
            endpoint.into(),
            BTreeMap::from([(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )]),
            form.into_bytes(),
        )?;
        let body = oauth_response(transport, request, context, OAuthPhase::TokenRequest).await?;
        let mut token: TokenResponse = serde_json::from_slice(&body)
            .map_err(|_| diagnostics::failure(OAuthPhase::TokenPayload, ProviderError::Protocol))?;
        if token.refresh_token.is_empty() {
            return Err(diagnostics::failure(
                OAuthPhase::RefreshTokenMissing,
                ProviderError::ReconnectRequired,
            ));
        }
        let expected = self.expected_scopes();
        if !token.token_type.eq_ignore_ascii_case("Bearer")
            || !exact_scopes(&token.scope, expected)
            || token.expires_in == 0
            || token.expires_in > 86400
        {
            return Err(diagnostics::failure(
                OAuthPhase::TokenPermissions,
                ProviderError::Unauthorized,
            ));
        }
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ProviderError::Invalid)?
            .as_secs()
            .checked_add(token.expires_in)
            .ok_or(ProviderError::Invalid)?;
        let secret = SecretBundle::new(
            std::mem::take(&mut token.access_token),
            std::mem::take(&mut token.refresh_token),
            expires,
            random_key()?,
        )
        .map_err(|error| diagnostics::failure(OAuthPhase::TokenPayload, error))?;
        let (method, endpoint, request_body) = match self.registration.provider() {
            ProviderKind::GoogleDrive => (
                Method::Get,
                "https://www.googleapis.com/drive/v3/about?fields=user(permissionId,displayName)",
                Vec::new(),
            ),
            ProviderKind::Dropbox => (
                Method::Post,
                "https://api.dropboxapi.com/2/users/get_current_account",
                b"null".to_vec(),
            ),
        };
        let headers = BTreeMap::from([
            (
                "authorization".into(),
                format!("Bearer {}", secret.access_token()),
            ),
            ("content-type".into(), "application/json".into()),
        ]);
        let body = oauth_response(
            transport,
            Request::new(method, endpoint.into(), headers, request_body)?,
            context,
            OAuthPhase::AccountRequest,
        )
        .await?;
        let (account, name) = match self.registration.provider() {
            ProviderKind::GoogleDrive => {
                let about: DriveAbout = serde_json::from_slice(&body).map_err(|_| {
                    diagnostics::failure(OAuthPhase::AccountPayload, ProviderError::Protocol)
                })?;
                (
                    AccountBinding::new(about.user.permission_id)
                        .map_err(|error| diagnostics::failure(OAuthPhase::AccountPayload, error))?,
                    DisplayText::new(about.user.display_name)
                        .map_err(|error| diagnostics::failure(OAuthPhase::AccountPayload, error))?,
                )
            }
            ProviderKind::Dropbox => {
                let about: DropboxAbout = serde_json::from_slice(&body).map_err(|_| {
                    diagnostics::failure(OAuthPhase::AccountPayload, ProviderError::Protocol)
                })?;
                if token
                    .account_id
                    .as_ref()
                    .is_some_and(|id| id != &about.account_id)
                {
                    return Err(diagnostics::failure(
                        OAuthPhase::AccountPayload,
                        ProviderError::IdentityMismatch,
                    ));
                }
                (
                    AccountBinding::new(about.account_id)
                        .map_err(|error| diagnostics::failure(OAuthPhase::AccountPayload, error))?,
                    DisplayText::new(about.name.display_name)
                        .map_err(|error| diagnostics::failure(OAuthPhase::AccountPayload, error))?,
                )
            }
        };
        Ok(CompletedAuthorization {
            grant: ProviderGrant::new(&self.registration, account, callback.selection, secret)
                .and_then(|grant| grant.with_verified_scopes(&token.scope))
                .map_err(|error| diagnostics::failure(OAuthPhase::Grant, error))?,
            name,
        })
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    token_type: String,
    scope: String,
    expires_in: u64,
    account_id: Option<String>,
}
impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.token_type.zeroize();
        self.scope.zeroize();
        self.account_id.zeroize();
    }
}
#[derive(Deserialize)]
struct DriveAbout {
    user: DriveUser,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveUser {
    permission_id: String,
    display_name: String,
}
#[derive(Deserialize)]
struct DropboxAbout {
    account_id: String,
    name: DropboxName,
}
#[derive(Deserialize)]
struct DropboxName {
    display_name: String,
}

async fn oauth_response(
    transport: &dyn Transport,
    request: Request,
    context: &IoContext,
    phase: OAuthPhase,
) -> AuthorizationResult<Zeroizing<Vec<u8>>> {
    let response = transport::send(transport, request, context, None)
        .await
        .map_err(|error| diagnostics::failure(phase.clone(), error))?;
    let response_status = response.status();
    let failure =
        (!(200..300).contains(&response.status())).then(|| transport::read_failure(&response));
    let body = Zeroizing::new(response.into_body());
    if body.len() > MAX_OAUTH_RESPONSE_BYTES {
        return Err(diagnostics::failure(phase, ProviderError::TooLarge));
    }
    if let Some(error) = failure {
        return Err(diagnostics::rejected(phase, response_status, &body, error));
    }
    Ok(body)
}
fn exact_scopes(actual: &str, expected: &str) -> bool {
    let actual: Vec<_> = actual.split_ascii_whitespace().collect();
    let expected: BTreeSet<_> = expected.split_ascii_whitespace().collect();
    !actual.is_empty()
        && actual.len() == expected.len()
        && actual.into_iter().collect::<BTreeSet<_>>() == expected
}
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    let mut difference = a.len() ^ b.len();
    for (left, right) in a.iter().zip(b) {
        difference |= usize::from(left ^ right)
    }
    difference == 0
}
fn valid_percent_encoding(bytes: &[u8]) -> bool {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1..=index + 2]
                    .iter()
                    .all(u8::is_ascii_hexdigit)
            {
                return false;
            }
            index += 2
        }
        index += 1
    }
    true
}

fn parse_http_callback<'a>(bytes: &'a [u8], redirect: &Url) -> Result<&'a str> {
    if bytes.len() > MAX_CALLBACK_BYTES {
        return Err(ProviderError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ProviderError::Protocol)?;
    if !text.ends_with("\r\n\r\n") {
        return Err(ProviderError::Protocol);
    }
    let mut lines = text[..text.len() - 4].split("\r\n");
    let first = lines.next().ok_or(ProviderError::Protocol)?;
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3 || parts[0] != "GET" || parts[2] != "HTTP/1.1" {
        return Err(ProviderError::Protocol);
    }
    let expected = format!(
        "127.0.0.1:{}",
        redirect.port().ok_or(ProviderError::Invalid)?
    );
    let mut host = None;
    let mut names = BTreeSet::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(ProviderError::Protocol)?;
        let name = name.to_ascii_lowercase();
        if !names.insert(name.clone())
            || name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.chars().any(|c| c.is_control() && c != '\t')
        {
            return Err(ProviderError::Protocol);
        }
        match name.as_str() {
            "host" => host = Some(value.trim()),
            "content-length" | "transfer-encoding" => return Err(ProviderError::Protocol),
            _ => {}
        }
    }
    if host != Some(expected.as_str()) {
        return Err(ProviderError::Protocol);
    }
    Ok(parts[1])
}

async fn bounded<T>(context: &IoContext, future: impl Future<Output = Result<T>>) -> Result<T> {
    context.check()?;
    tokio::select! {biased;_=context.cancellation().cancelled()=>Err(ProviderError::Cancelled),result=tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),future)=>result.map_err(|_|ProviderError::Timeout)?}
}
async fn receive_callback(
    listener: &TcpListener,
    pending: &PendingAuthorization,
    context: &IoContext,
) -> Result<VerifiedCallback> {
    bounded(context, async {
        let (mut stream, peer) = listener
            .accept()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        if !peer.ip().is_loopback() {
            return Err(ProviderError::Unauthorized);
        }
        let mut buffer = Zeroizing::new(Vec::new());
        loop {
            let mut chunk = [0; 1024];
            let count = stream
                .read(&mut chunk)
                .await
                .map_err(|_| ProviderError::Protocol)?;
            if count == 0 {
                return Err(ProviderError::Protocol);
            }
            if buffer.len() + count > MAX_CALLBACK_BYTES {
                return Err(ProviderError::TooLarge);
            }
            buffer.extend_from_slice(&chunk[..count]);
            chunk.zeroize();
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let result = parse_http_callback(&buffer, &pending.redirect)
            .and_then(|target| pending.verify_callback(target));
        send_browser_response(&mut stream, result.is_ok()).await;
        result
    })
    .await
}
async fn send_browser_response(stream: &mut TcpStream, success: bool) {
    let (status, body) = if success {
        ("200 OK", "Authorization received. Return to MDReader.")
    } else {
        (
            "400 Bad Request",
            "Authorization failed. Return to MDReader and retry.",
        )
    };
    let response=format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",body.len());
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// One deadline/cancellation bounds browser dispatch, callback and provider I/O.
/// Mobile fails before opening a browser/listener until system auth SDK support.
#[cfg(test)]
pub(crate) async fn authorize(
    registration: Registration,
    selection: Option<NativeSelection>,
    config: &OAuthConfig,
    browser: &dyn SystemBrowser,
    transport: &dyn Transport,
    context: &IoContext,
) -> Result<CompletedAuthorization> {
    authorize_with_permissions(
        registration,
        selection,
        false,
        config,
        browser,
        transport,
        context,
    )
    .await
}
#[cfg(test)]
pub(crate) async fn authorize_with_permissions(
    registration: Registration,
    selection: Option<NativeSelection>,
    allow_editing: bool,
    config: &OAuthConfig,
    browser: &dyn SystemBrowser,
    transport: &dyn Transport,
    context: &IoContext,
) -> Result<CompletedAuthorization> {
    authorize_detailed(
        registration,
        selection,
        allow_editing,
        config,
        browser,
        transport,
        context,
    )
    .await
    .map_err(|failure| failure.error)
}
pub(crate) async fn authorize_detailed(
    registration: Registration,
    selection: Option<NativeSelection>,
    allow_editing: bool,
    config: &OAuthConfig,
    browser: &dyn SystemBrowser,
    transport: &dyn Transport,
    context: &IoContext,
) -> AuthorizationResult<CompletedAuthorization> {
    if cfg!(any(target_os = "ios", target_os = "android")) {
        return Err(ProviderError::Unsupported.into());
    }
    context.check()?;
    let _permit = AUTHORIZATION_GATE
        .try_acquire()
        .map_err(|_| ProviderError::Unavailable)?;
    // A caller cannot create an indefinitely held authorization listener.
    if context.deadline().saturating_duration_since(Instant::now()) > Duration::from_secs(300) {
        return Err(ProviderError::Invalid.into());
    }
    let (listener, redirect) = match registration.provider() {
        ProviderKind::GoogleDrive => {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .map_err(|_| ProviderError::Unavailable)?;
            let port = listener
                .local_addr()
                .map_err(|_| ProviderError::Unavailable)?
                .port();
            (listener, format!("http://127.0.0.1:{port}/oauth/callback"))
        }
        ProviderKind::Dropbox => {
            let redirect = config
                .dropbox_registered_redirect
                .as_ref()
                .ok_or(ProviderError::Unsupported)?;
            let url = validate_redirect(redirect)?;
            let listener = TcpListener::bind((
                std::net::Ipv4Addr::LOCALHOST,
                url.port().ok_or(ProviderError::Invalid)?,
            ))
            .await
            .map_err(|_| ProviderError::Unavailable)?;
            (listener, redirect.clone())
        }
    };
    let pending =
        PendingAuthorization::with_permissions(registration, &redirect, selection, allow_editing)?;
    let url = pending.authorization_url()?;
    bounded(context, browser.open(&url))
        .await
        .map_err(|error| diagnostics::failure(OAuthPhase::Browser, error))?;
    let verified = receive_callback(&listener, &pending, context)
        .await
        .map_err(|error| diagnostics::failure(OAuthPhase::Callback, error))?;
    drop(listener);
    pending
        .exchange_detailed(verified, transport, context)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdreader_provider::transport::Response;
    use std::sync::Mutex;

    fn google() -> Registration {
        Registration::google_drive("synthetic.apps.googleusercontent.com".into()).unwrap()
    }
    fn context() -> IoContext {
        IoContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_secs(2),
            1,
            65536,
            Default::default(),
        )
        .unwrap()
    }
    fn callback(session: &PendingAuthorization, extra: &str) -> String {
        format!(
            "{}?state={}&code=synthetic-code&scope={}&picked_file_ids=file_A,file_B{}",
            session.redirect.path(),
            session.state.as_str(),
            DRIVE_SCOPE,
            extra
        )
    }
    #[test]
    fn readonly_drive_consent_is_distinct_and_never_uses_picker_or_inherited_scope() {
        let selected = NativeSelection::DriveReadOnlyFolder {
            folder: RecordKey::new("root").unwrap(),
        };
        let session = PendingAuthorization::new(
            google(),
            "http://127.0.0.1:34567/oauth/callback",
            Some(selected),
        )
        .unwrap();
        let url = session.authorization_url().unwrap();
        let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["scope"], mdreader_credentials::DRIVE_READONLY_SCOPE);
        assert_eq!(query["prompt"], "consent");
        assert_eq!(query["include_granted_scopes"], "false");
        assert!(!query.contains_key("trigger_onepick"));
        let callback = format!(
            "{}?state={}&code=synthetic-code&scope={}",
            session.redirect.path(),
            session.state.as_str(),
            mdreader_credentials::DRIVE_READONLY_SCOPE
        );
        assert!(
            matches!(session.verify_callback(&callback).unwrap().selection, NativeSelection::DriveReadOnlyFolder { folder } if folder.as_str() == "root")
        );
        assert!(session
            .verify_callback(
                &callback.replace(mdreader_credentials::DRIVE_READONLY_SCOPE, DRIVE_SCOPE)
            )
            .is_err());
        assert!(session
            .verify_callback(&(callback + "&picked_file_ids=file_A"))
            .is_err());
    }
    #[test]
    fn explicit_drive_folder_options_bind_fresh_consent_without_upgrading_picker_or_readonly_grants(
    ) {
        for editing in [false, true] {
            let session = PendingAuthorization::with_permissions(
                google(),
                "http://127.0.0.1:34567/oauth/callback",
                Some(NativeSelection::DriveFolder {
                    folder: RecordKey::new("notes").unwrap(),
                    recursive: true,
                }),
                editing,
            )
            .unwrap();
            let query: BTreeMap<_, _> = session
                .authorization_url()
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            let expected = if editing {
                mdreader_credentials::DRIVE_EDIT_SCOPE
            } else {
                mdreader_credentials::DRIVE_READONLY_SCOPE
            };
            assert_eq!(query["scope"], expected);
            assert_eq!(query["include_granted_scopes"], "false");
            assert!(!query.contains_key("trigger_onepick"));
            let target = format!(
                "/oauth/callback?state={}&code=synthetic-code&scope={expected}",
                session.state.as_str()
            );
            let verified = session.verify_callback(&target).unwrap();
            assert!(matches!(
                verified.selection,
                NativeSelection::DriveFolder {
                    recursive: true,
                    ..
                }
            ));
            let grant = ProviderGrant::new(
                &google(),
                AccountBinding::new("synthetic-account").unwrap(),
                verified.selection,
                SecretBundle::new(
                    "synthetic-access".into(),
                    "synthetic-refresh".into(),
                    1_900_000_000,
                    [7; 32],
                )
                .unwrap(),
            )
            .unwrap();
            assert!(!grant.permits_guarded_write());
            let grant = grant.with_verified_scopes(expected).unwrap();
            assert_eq!(grant.permits_guarded_write(), editing);
            let restored: NativeSelection =
                serde_json::from_value(serde_json::to_value(grant.selection()).unwrap()).unwrap();
            assert_eq!(
                restored.scope().unwrap(),
                grant.selection().scope().unwrap()
            );
        }
        assert!(PendingAuthorization::with_permissions(
            google(),
            "http://127.0.0.1:34567/oauth/callback",
            None,
            true
        )
        .is_err());
        assert!(PendingAuthorization::with_permissions(
            google(),
            "http://127.0.0.1:34567/oauth/callback",
            Some(NativeSelection::DriveReadOnlyFolder {
                folder: RecordKey::new("notes").unwrap()
            }),
            true
        )
        .is_err());
    }
    #[test]
    fn picker_url_is_fresh_narrow_pkce_and_callback_selection_only() {
        let session =
            PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                .unwrap();
        let url = session.authorization_url().unwrap();
        let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("scope").unwrap(), DRIVE_SCOPE);
        assert_eq!(query.get("prompt").unwrap(), "consent");
        assert_eq!(query.get("trigger_onepick").unwrap(), "true");
        assert_eq!(query.get("include_granted_scopes").unwrap(), "false");
        assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(
            query.get("code_challenge").unwrap(),
            &URL_SAFE_NO_PAD.encode(Sha256::digest(session.verifier.as_bytes()))
        );
        assert_ne!(session.state.as_str(), session.verifier.as_str());
        let target = callback(&session, "");
        let verified = session.verify_callback(&target).unwrap();
        assert!(
            matches!(verified.selection,NativeSelection::DriveFiles{ref files} if files.len()==2 && files[0].id.as_str()=="file_A")
        );
        assert!(!format!("{verified:?}").contains("synthetic-code"));
    }
    #[test]
    fn google_callback_accepts_exact_issuer_and_rejects_mixup_or_duplicate() {
        let session =
            PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                .unwrap();
        let target = callback(
            &session,
            "&iss=https%3A%2F%2Faccounts.google.com&authuser=0&prompt=consent",
        );
        let raw = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:34567\r\n\r\n");
        let parsed = parse_http_callback(raw.as_bytes(), &session.redirect).unwrap();
        let verified = session.verify_callback(parsed).unwrap();
        assert!(
            matches!(verified.selection, NativeSelection::DriveFiles { ref files }
            if files.len() == 2 && files[0].id.as_str() == "file_A")
        );
        for suffix in [
            "&iss=https%3A%2F%2Fattacker.invalid",
            "&iss=https%3A%2F%2Faccounts.google.com%2F",
            "&iss=https%3A%2F%2Faccounts.google.com&i%73s=https%3A%2F%2Faccounts.google.com",
        ] {
            assert!(session
                .verify_callback(&callback(&session, suffix))
                .is_err());
        }
        let wrong_state = target.replace(session.state.as_str(), "other-state");
        assert!(session.verify_callback(&wrong_state).is_err());
    }
    #[test]
    fn callback_rejects_csrf_duplicate_scope_broad_grants_and_path_tricks() {
        for suffix in [
            "&state=other",
            "&scope=https://www.googleapis.com/auth/drive",
            "&code=other",
            "&picked_file_ids=file_C",
        ] {
            let session =
                PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                    .unwrap();
            assert!(session
                .verify_callback(&callback(&session, suffix))
                .is_err());
        }
        for target in [
            "/oauth/callback?state=wrong&code=secret",
            "http://attacker.invalid/oauth/callback?code=x",
            "/elsewhere?code=x",
            "/oauth/callback?state=%ZZ",
            "/oauth/callback?state=x#ignored",
        ] {
            let session =
                PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                    .unwrap();
            assert!(session.verify_callback(target).is_err());
        }
        let session =
            PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                .unwrap();
        let target = callback(&session, "").replace("file_A,file_B", "file_A,file_A");
        assert!(session.verify_callback(&target).is_err());
    }
    #[test]
    fn loopback_http_and_owner_redirect_are_closed_and_bounded() {
        let redirect = validate_redirect("http://127.0.0.1:34567/oauth/callback").unwrap();
        let valid = b"GET /oauth/callback?state=x HTTP/1.1\r\nHost: 127.0.0.1:34567\r\n\r\n";
        assert_eq!(
            parse_http_callback(valid, &redirect).unwrap(),
            "/oauth/callback?state=x"
        );
        for raw in [
            b"GET / HTTP/1.1\r\nHost: evil\r\n\r\n".as_slice(),
            b"POST / HTTP/1.1\r\nHost: 127.0.0.1:34567\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: 127.0.0.1:34567\r\nHost: 127.0.0.1:34567\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: 127.0.0.1:34567\r\nContent-Length: 1\r\n\r\nx",
        ] {
            assert!(parse_http_callback(raw, &redirect).is_err());
        }
        assert!(parse_http_callback(&vec![b'x'; MAX_CALLBACK_BYTES + 1], &redirect).is_err());
        for uri in [
            "http://localhost:34567/callback",
            "http://127.0.0.1:0/callback",
            "https://127.0.0.1:34567/callback",
            "http://127.0.0.1:34567/callback?x=y",
            "http://user@127.0.0.1:34567/callback",
        ] {
            assert!(validate_redirect(uri).is_err());
        }
    }
    #[tokio::test]
    async fn rejected_exchange_preserves_kind_and_carries_only_public_diagnostics() {
        struct RejectedTransport(bool);
        #[async_trait::async_trait]
        impl Transport for RejectedTransport {
            async fn send(&self, _: Request) -> Result<Response> {
                let body = if self.0 {
                    br#"{"error":"invalid_client","error_description":"synthetic-private-token-and-account"}"#.to_vec()
                } else {
                    br#"{"error":"synthetic-private-token-and-account"}"#.to_vec()
                };
                Response::new(400, BTreeMap::new(), body)
            }
        }
        for (known, expected) in [
            (true, OAuthRejection::InvalidClient),
            (false, OAuthRejection::Other),
        ] {
            let pending =
                PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                    .unwrap();
            let verified = pending.verify_callback(&callback(&pending, "")).unwrap();
            let failure = pending
                .exchange_detailed(verified, &RejectedTransport(known), &context())
                .await
                .unwrap_err();
            assert_eq!(failure.error, ProviderError::Protocol);
            assert_eq!(failure.diagnostic.phase, OAuthPhase::TokenRequest);
            assert_eq!(failure.diagnostic.response_status, Some(400));
            assert_eq!(failure.diagnostic.rejection, Some(expected));
            let native = crate::native_providers::NativeProviderError::from(failure);
            let wire = serde_json::to_value(native).unwrap();
            assert_eq!(wire["kind"], "authorization_failed");
            assert_eq!(wire["error"]["kind"], "invalid_response");
            assert_eq!(wire["diagnostic"]["phase"], "token_request");
            assert!(!wire.to_string().contains("synthetic-private"));
        }
    }
    struct SyntheticTransport {
        calls: Mutex<Vec<(String, String, Vec<u8>)>>,
        broad: bool,
    }
    #[async_trait::async_trait]
    impl Transport for SyntheticTransport {
        async fn send(&self, request: Request) -> Result<Response> {
            self.calls.lock().unwrap().push((
                request.url().into(),
                request
                    .headers()
                    .get("authorization")
                    .cloned()
                    .unwrap_or_default(),
                request.body().to_vec(),
            ));
            let body = if request.url().contains("/token") {
                serde_json::json!({"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":3600,"token_type":"Bearer","scope":if self.broad {"https://www.googleapis.com/auth/drive"} else {DRIVE_SCOPE}})
            } else {
                serde_json::json!({"user":{"permissionId":"verified-account","displayName":"Synthetic account"}})
            };
            Response::new(200, BTreeMap::new(), serde_json::to_vec(&body).unwrap())
        }
    }
    #[tokio::test]
    async fn fresh_exchange_binds_account_selection_and_never_dispatches_broad_grant() {
        for broad in [false, true] {
            let transport = SyntheticTransport {
                calls: Mutex::new(vec![]),
                broad,
            };
            let pending =
                PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                    .unwrap();
            let verified = pending.verify_callback(&callback(&pending, "")).unwrap();
            let result = pending.exchange(verified, &transport, &context()).await;
            if broad {
                assert!(matches!(result, Err(ProviderError::Unauthorized)));
                assert_eq!(transport.calls.lock().unwrap().len(), 1);
            } else {
                let completed = result.unwrap();
                assert_eq!(completed.grant.account().as_str(), "verified-account");
                assert_eq!(completed.name.as_str(), "Synthetic account");
                assert!(
                    matches!(completed.grant.selection(),NativeSelection::DriveFiles{files} if files.len()==2)
                );
                assert!(!format!("{completed:?}").contains("synthetic-access"));
                let calls = transport.calls.lock().unwrap();
                assert_eq!(calls.len(), 2);
                let form: BTreeMap<_, _> = url::form_urlencoded::parse(&calls[0].2)
                    .into_owned()
                    .collect();
                assert_eq!(form.get("grant_type").unwrap(), "authorization_code");
                assert!(form.get("code_verifier").unwrap().len() >= 43);
                assert_eq!(calls[1].1, "Bearer synthetic-access");
            }
        }
    }
    #[test]
    fn dropbox_requires_owner_app_folder_registration_and_explicit_selection() {
        let registration =
            Registration::dropbox_app_folder("syntheticapp".into(), [7; 32]).unwrap();
        assert!(PendingAuthorization::new(
            registration.clone(),
            "http://127.0.0.1:34567/oauth/callback",
            None
        )
        .is_err());
        let selection = NativeSelection::DropboxRoot {
            path: RecordKey::new("/").unwrap(),
            recursive: true,
        };
        let session = PendingAuthorization::new(
            registration,
            "http://127.0.0.1:34567/oauth/callback",
            Some(selection),
        )
        .unwrap();
        let url = session.authorization_url().unwrap();
        let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("token_access_type").unwrap(), "offline");
        assert_eq!(query.get("scope").unwrap(), DROPBOX_SCOPES);
        assert!(!query.get("scope").unwrap().contains("write"));
        assert!(session
            .verify_callback(&format!(
                "/oauth/callback?state={}&code=x&picked_file_ids=evil",
                session.state.as_str()
            ))
            .is_err());
    }
    struct DropboxTransport {
        mismatch: bool,
    }
    #[async_trait::async_trait]
    impl Transport for DropboxTransport {
        async fn send(&self, request: Request) -> Result<Response> {
            let value = if request.url() == "https://api.dropboxapi.com/oauth2/token" {
                serde_json::json!({"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":14400,"token_type":"Bearer","scope":DROPBOX_SCOPES,"account_id":"verified-dropbox"})
            } else {
                assert_eq!(
                    request.url(),
                    "https://api.dropboxapi.com/2/users/get_current_account"
                );
                assert_eq!(request.body(), b"null");
                serde_json::json!({"account_id":if self.mismatch {"other-account"}else{"verified-dropbox"},"name":{"display_name":"Synthetic Dropbox"}})
            };
            Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap())
        }
    }
    #[tokio::test]
    async fn dropbox_fresh_exchange_preserves_explicit_root_and_rejects_account_switch() {
        for mismatch in [false, true] {
            let registration =
                Registration::dropbox_app_folder("syntheticapp".into(), [7; 32]).unwrap();
            let selection = NativeSelection::DropboxRoot {
                path: RecordKey::new("/notes").unwrap(),
                recursive: false,
            };
            let pending = PendingAuthorization::new(
                registration,
                "http://127.0.0.1:34567/oauth/callback",
                Some(selection),
            )
            .unwrap();
            let callback = pending
                .verify_callback(&format!(
                    "/oauth/callback?state={}&code=synthetic-code",
                    pending.state.as_str()
                ))
                .unwrap();
            let result = pending
                .exchange(callback, &DropboxTransport { mismatch }, &context())
                .await;
            if mismatch {
                assert!(matches!(result, Err(ProviderError::IdentityMismatch)))
            } else {
                let completed = result.unwrap();
                assert_eq!(completed.grant.account().as_str(), "verified-dropbox");
                assert_eq!(completed.name.as_str(), "Synthetic Dropbox");
                assert!(
                    matches!(completed.grant.selection(),NativeSelection::DropboxRoot{path,recursive:false}if path.as_str()=="/notes")
                );
            }
        }
    }
    struct CallbackBrowser {
        task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    }
    #[async_trait::async_trait]
    impl SystemBrowser for CallbackBrowser {
        async fn open(&self, url: &Url) -> Result<()> {
            assert!(
                AUTHORIZATION_GATE.try_acquire().is_err(),
                "only one authorization may be in flight"
            );
            let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
            let redirect = Url::parse(query.get("redirect_uri").unwrap()).unwrap();
            let state = query.get("state").unwrap().clone();
            let task = tokio::spawn(async move {
                let port = redirect.port().unwrap();
                let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                    .await
                    .unwrap();
                let request=format!("GET /oauth/callback?state={state}&code=synthetic-code&scope={DRIVE_SCOPE}&picked_file_ids=file_A HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
                stream.write_all(request.as_bytes()).await.unwrap();
                let mut response = String::new();
                stream.read_to_string(&mut response).await.unwrap();
                assert!(response.starts_with("HTTP/1.1 200 OK"));
                assert!(!response.contains("synthetic-code"));
                assert!(response.contains("Cache-Control: no-store"));
            });
            *self.task.lock().unwrap() = Some(task);
            Ok(())
        }
    }
    #[tokio::test]
    async fn actual_loopback_callback_exchanges_once_without_rendering_secrets() {
        let browser = CallbackBrowser {
            task: Mutex::new(None),
        };
        let transport = SyntheticTransport {
            calls: Mutex::new(vec![]),
            broad: false,
        };
        let result = authorize(
            google(),
            None,
            &OAuthConfig {
                dropbox_registered_redirect: None,
            },
            &browser,
            &transport,
            &context(),
        )
        .await
        .unwrap();
        assert_eq!(result.grant.account().as_str(), "verified-account");
        assert_eq!(transport.calls.lock().unwrap().len(), 2);
        assert!(
            AUTHORIZATION_GATE.try_acquire().is_ok(),
            "authorization releases the process gate"
        );
        let task = browser.task.lock().unwrap().take().unwrap();
        task.await.unwrap();
    }
    struct HungTransport;
    #[async_trait::async_trait]
    impl Transport for HungTransport {
        async fn send(&self, _: Request) -> Result<Response> {
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn deadline_and_cancellation_bound_exchange_without_retry() {
        let pending =
            PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                .unwrap();
        let verified = pending.verify_callback(&callback(&pending, "")).unwrap();
        let deadline = IoContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_millis(20),
            1,
            65536,
            Default::default(),
        )
        .unwrap();
        let started = Instant::now();
        assert!(matches!(
            pending.exchange(verified, &HungTransport, &deadline).await,
            Err(ProviderError::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        let pending =
            PendingAuthorization::new(google(), "http://127.0.0.1:34567/oauth/callback", None)
                .unwrap();
        let verified = pending.verify_callback(&callback(&pending, "")).unwrap();
        let context = context();
        context.cancellation().cancel();
        let transport = SyntheticTransport {
            calls: Mutex::new(vec![]),
            broad: false,
        };
        assert!(matches!(
            pending.exchange(verified, &transport, &context).await,
            Err(ProviderError::Cancelled)
        ));
        assert!(transport.calls.lock().unwrap().is_empty());
    }
}
