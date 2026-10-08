use super::identity::verify;
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

/// Native-reviewed endpoints. No Deserialize and no implicit discovery from a callback/token.
pub struct OidcEndpoints {
    pub authorization: String,
    pub token: String,
    pub jwks: String,
}
pub struct ReviewedOidcRegistration {
    pub(super) issuer: String,
    pub(super) client_id: String,
    pub(super) authorization: Url,
    pub(super) token: Url,
    pub(super) jwks: Url,
    pub(super) sync: Url,
    pub(super) digest: String,
}
impl std::fmt::Debug for ReviewedOidcRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("ReviewedOidcRegistration", f)
    }
}
impl ReviewedOidcRegistration {
    pub fn new(
        issuer: &str,
        client_id: &str,
        endpoints: OidcEndpoints,
        sync_origin: &str,
    ) -> AuthorityResult<Self> {
        http::https(issuer, false)?;
        if client_id.is_empty()
            || client_id.len() > 512
            || client_id
                .bytes()
                .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        {
            return Err(AuthorityError::Configuration);
        }
        let authorization = http::https(&endpoints.authorization, false)?;
        let token = http::https(&endpoints.token, false)?;
        let jwks = http::https(&endpoints.jwks, false)?;
        let sync = http::https(sync_origin, true)?;
        let digest = crate::blob_digest(
            &serde_json::to_vec(&(
                "mdreader-native-oidc",
                1,
                issuer,
                client_id,
                authorization.as_str(),
                token.as_str(),
                jwks.as_str(),
                sync.as_str(),
            ))
            .map_err(|_| AuthorityError::Configuration)?,
        );
        Ok(Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            authorization,
            token,
            jwks,
            sync,
            digest,
        })
    }
}
/// Implement only in the native system-browser owner. No embedded webview or renderer URL log.
pub trait NativeBrowser: Send + Sync {
    fn open<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = AuthorityResult<()>> + Send + 'a>>;
}
struct LoginGuard;
static LOGIN: AtomicBool = AtomicBool::new(false);
impl Drop for LoginGuard {
    fn drop(&mut self) {
        LOGIN.store(false, Ordering::Release);
    }
}
pub struct PendingLogin {
    registration: Arc<ReviewedOidcRegistration>,
    redirect: Url,
    state: Zeroizing<String>,
    verifier: Zeroizing<String>,
    nonce: Zeroizing<String>,
    created: u64,
    expires: Instant,
    opened: AtomicBool,
    _guard: LoginGuard,
}
impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("PendingLogin", f)
    }
}
pub struct OidcClient {
    pub(super) registration: Arc<ReviewedOidcRegistration>,
    pub(super) http: http::Http,
}
impl std::fmt::Debug for OidcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("OidcClient", f)
    }
}
fn random() -> AuthorityResult<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0; 32]);
    getrandom::fill(bytes.as_mut()).map_err(|_| AuthorityError::Custody)?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}
fn redirect(value: &str) -> AuthorityResult<Url> {
    let url = Url::parse(value).map_err(|_| AuthorityError::Configuration)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none_or(|p| p == 0)
        || url.path() != "/mdreader-sync/callback"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AuthorityError::Configuration);
    }
    Ok(url)
}
impl OidcClient {
    pub fn new(registration: ReviewedOidcRegistration) -> AuthorityResult<Self> {
        Ok(Self {
            registration: Arc::new(registration),
            http: http::Http::new(None)?,
        })
    }
    pub fn begin(&self, native_loopback_redirect: &str) -> AuthorityResult<PendingLogin> {
        self.begin_nonce(native_loopback_redirect, random()?)
    }
    fn begin_nonce(
        &self,
        native_loopback_redirect: &str,
        nonce: Zeroizing<String>,
    ) -> AuthorityResult<PendingLogin> {
        let redirect = redirect(native_loopback_redirect)?;
        if LOGIN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(AuthorityError::Busy);
        }
        let guard = LoginGuard;
        Ok(PendingLogin {
            registration: self.registration.clone(),
            redirect,
            state: random()?,
            verifier: random()?,
            nonce,
            created: now()?,
            expires: Instant::now() + Duration::from_secs(300),
            opened: AtomicBool::new(false),
            _guard: guard,
        })
    }
    /// Consumes the attempt even on invalid callback/failed exchange. The native callback
    /// owner must bind/listen on the exact loopback address before opening the browser.
    pub async fn complete(
        &self,
        pending: PendingLogin,
        callback_target: &str,
    ) -> AuthorityResult<VerifiedIdentity> {
        if pending.registration.digest != self.registration.digest
            || Instant::now() >= pending.expires
        {
            return Err(AuthorityError::Identity);
        }
        let code = pending.callback(callback_target)?;
        let form = Zeroizing::new(
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("grant_type", "authorization_code")
                .append_pair("client_id", &self.registration.client_id)
                .append_pair("redirect_uri", pending.redirect.as_str())
                .append_pair("code", &code)
                .append_pair("code_verifier", &pending.verifier)
                .finish()
                .into_bytes(),
        );
        let response = self
            .http
            .send(
                reqwest::Method::POST,
                self.registration.token.clone(),
                form,
                None,
                "application/x-www-form-urlencoded",
                64 * 1024,
            )
            .await?;
        let mut wire: TokenResponse =
            serde_json::from_slice(&response).map_err(|_| AuthorityError::Identity)?;
        if !wire.token_type.eq_ignore_ascii_case("Bearer")
            || wire.access_token.is_empty()
            || wire.access_token.len() > 16 * 1024
            || wire.id_token.len() >= 16 * 1024
        {
            return Err(AuthorityError::Identity);
        }
        let token = Zeroizing::new(std::mem::take(&mut wire.id_token));
        let jwks = self
            .http
            .send(
                reqwest::Method::GET,
                self.registration.jwks.clone(),
                Zeroizing::new(Vec::new()),
                None,
                "application/json",
                64 * 1024,
            )
            .await?;
        if Instant::now() >= pending.expires {
            return Err(AuthorityError::Identity);
        }
        verify(
            &self.registration,
            token,
            &jwks,
            &pending.nonce,
            pending.created,
        )
    }
    #[cfg(test)]
    pub(super) fn test_with_root(
        registration: ReviewedOidcRegistration,
        root: reqwest::Certificate,
    ) -> AuthorityResult<Self> {
        Ok(Self {
            registration: Arc::new(registration),
            http: http::Http::new(Some(root))?,
        })
    }
    #[cfg(test)]
    pub(super) fn test_begin_nonce(
        &self,
        redirect: &str,
        nonce: &str,
    ) -> AuthorityResult<PendingLogin> {
        self.begin_nonce(redirect, Zeroizing::new(nonce.into()))
    }
}
impl PendingLogin {
    pub async fn open(&self, browser: &dyn NativeBrowser) -> AuthorityResult<()> {
        if Instant::now() >= self.expires
            || self
                .opened
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(AuthorityError::Identity);
        }
        let mut url = self.registration.authorization.clone();
        url.query_pairs_mut()
            .append_pair("client_id", &self.registration.client_id)
            .append_pair("redirect_uri", self.redirect.as_str())
            .append_pair("response_type", "code")
            .append_pair("scope", "openid")
            .append_pair("state", &self.state)
            .append_pair("nonce", &self.nonce)
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes())),
            )
            .append_pair("code_challenge_method", "S256");
        let text: Zeroizing<String> = Zeroizing::new(url.into());
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(self.expires),
            browser.open(&text),
        )
        .await
        .map_err(|_| AuthorityError::Interrupted)?
    }
    fn callback(&self, target: &str) -> AuthorityResult<Zeroizing<String>> {
        if target.len() > 16 * 1024
            || !target.starts_with('/')
            || target.starts_with("//")
            || target.contains('#')
            || target.bytes().any(|b| b.is_ascii_control())
        {
            return Err(AuthorityError::Identity);
        }
        let bytes = target.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'%'
                && (i + 2 >= bytes.len() || !bytes[i + 1..=i + 2].iter().all(u8::is_ascii_hexdigit))
            {
                return Err(AuthorityError::Identity);
            }
        }
        let (path, query) = target.split_once('?').ok_or(AuthorityError::Identity)?;
        if path != self.redirect.path() {
            return Err(AuthorityError::Identity);
        }
        let mut state = None;
        let mut code = None;
        let mut issuer = None;
        let mut seen = HashSet::new();
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            if !seen.insert(k.to_string()) {
                return Err(AuthorityError::Identity);
            }
            match k.as_ref() {
                "state" => state = Some(Zeroizing::new(v.into_owned())),
                "code" => code = Some(Zeroizing::new(v.into_owned())),
                "iss" => issuer = Some(v.into_owned()),
                "error" | "error_description" | "error_uri" => {
                    return Err(AuthorityError::Identity)
                }
                "authuser" | "prompt" | "scope" => {}
                _ => return Err(AuthorityError::Identity),
            }
        }
        if !secret_equal(
            state.as_ref().ok_or(AuthorityError::Identity)?.as_bytes(),
            self.state.as_bytes(),
        ) || issuer.is_some_and(|i| i != self.registration.issuer)
        {
            return Err(AuthorityError::Identity);
        }
        let code = code.ok_or(AuthorityError::Identity)?;
        if code.is_empty() || code.len() > 8192 || code.chars().any(char::is_control) {
            return Err(AuthorityError::Identity);
        }
        Ok(code)
    }
}
#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: String,
    token_type: String,
    #[serde(default)]
    refresh_token: Option<String>,
}
impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.id_token.zeroize();
        self.access_token.zeroize();
        if let Some(refresh) = &mut self.refresh_token {
            refresh.zeroize();
        }
    }
}
