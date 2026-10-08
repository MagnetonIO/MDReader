//! Bounded native-only transport. Authority/custody and typed owner projection are injected.
//! Receiving authenticated root envelopes does not apply their content or acknowledge them.
use crate::*;
use reqwest::{
    header::{AUTHORIZATION, CONTENT_TYPE},
    Method,
};
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("sync authority unavailable or changed")]
    Authority,
    #[error("sync scope already running")]
    Busy,
    #[error("sync device authentication rejected")]
    Authentication,
    #[error("sync device revoked or access denied")]
    Forbidden,
    #[error("sync remote conflict")]
    Conflict,
    #[error("sync quota or response bound exceeded")]
    Quota,
    #[error("sync request interrupted or temporarily unavailable")]
    Retryable,
    #[error("sync protocol rejected")]
    Protocol,
    #[error("sync local persistence or encrypted data rejected")]
    Local(LocalFailure),
}
/// Diagnostic classification only: never retain SQLite/custom owner error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFailure {
    Invalid,
    Authentication,
    Bounds,
    Scope,
    Reused,
    NotReady,
    Schema,
    Persistence,
    Entropy,
}
impl From<SyncError> for TransportError {
    fn from(value: SyncError) -> Self {
        Self::Local(match value {
            SyncError::Invalid => LocalFailure::Invalid,
            SyncError::Authentication => LocalFailure::Authentication,
            SyncError::Bounds => LocalFailure::Bounds,
            SyncError::Scope => LocalFailure::Scope,
            SyncError::Reused => LocalFailure::Reused,
            SyncError::NotReady => LocalFailure::NotReady,
            SyncError::Schema => LocalFailure::Schema,
            SyncError::Database(_) => LocalFailure::Persistence,
            SyncError::Entropy => LocalFailure::Entropy,
        })
    }
}
pub type TransportResult<T> = std::result::Result<T, TransportError>;

/// Remote authentication failures contain no response body or credential material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityRejection {
    Authentication,
    Forbidden,
}
/// Native implementation binds verified issuer/subject, enrolled device and current
/// custody generation. Generation changes invalidate the owner binding under its
/// barrier as well. Never use unverified token decoding. No RPC accepts this capability.
pub trait Authority: Send + Sync {
    /// Called by transport on every remote 401/403, including late stale responses.
    /// Implementations must invalidate only this capability, never a replacement session.
    fn rejected(&self, _reason: AuthorityRejection) {}
    fn scope(&self) -> TransportResult<SyncScope>;
    /// Changes on sign-out, key/session replacement or revocation even if scope is equal.
    fn generation(&self) -> TransportResult<u64>;
    /// Create fresh ES256 mdreader-proof+jwt bound to method, exact URL including query,
    /// SHA256(body), SHA256(bearer), iat and random single-use UUID jti.
    fn authorize(
        &self,
        method: &str,
        exact_url: &str,
        body: &[u8],
    ) -> TransportResult<Authorization>;
    /// Authenticate envelope/context using a native-held library key. Return ciphertext only.
    fn authenticate_root(
        &self,
        scope: &SyncScope,
        change: &RemoteChange,
        body: &[u8],
    ) -> TransportResult<PreparedRevision>;
}
mod authorization;
pub use authorization::Authorization;
mod foreground;
mod graph;
#[cfg(test)]
mod graph_upload_tests;
mod upload;
pub use graph::DownloadedResource;

/// Only short synchronous closures borrow the ONE reader-owner connection. Implementations
/// validate expected scope while holding the same barrier that changes their active binding.
pub trait Owner: Send + Sync {
    fn with_store<T>(
        &self,
        expected: &SyncScope,
        action: impl FnOnce(&mut SyncStore<'_>) -> Result<T>,
    ) -> TransportResult<T>;
}
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub outgoing: usize,
    pub pages: usize,
    pub page_size: usize,
    pub transfer_bytes: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            outgoing: 10,
            pages: 5,
            page_size: 10,
            transfer_bytes: 8 * 1024 * 1024,
        }
    }
}
impl Budget {
    fn validate(self) -> TransportResult<()> {
        if self.outgoing > 20
            || self.pages > 10
            || self.page_size == 0
            || self.page_size > 20
            || self.transfer_bytes == 0
            || self.transfer_bytes > 32 * 1024 * 1024
        {
            Err(TransportError::Quota)
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Default)]
pub struct RunReport {
    pub committed: usize,
    /// Number of committed operations whose durable receipt retains concurrent heads.
    pub conflicts: usize,
    pub received: u64,
    pub applied: u64,
    pub acknowledged: u64,
    pub remote_pending: bool,
    pub local_pending: bool,
}

struct Session<'a, A: Authority> {
    authority: &'a A,
    scope: SyncScope,
    generation: u64,
    remaining: usize,
}

pub struct Transport {
    endpoint: Url,
    http: reqwest::Client,
    running: &'static Mutex<HashSet<SyncScope>>,
}
struct RunGuard<'a> {
    running: &'a Mutex<HashSet<SyncScope>>,
    scope: SyncScope,
}
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut scopes) = self.running.lock() {
            scopes.remove(&self.scope);
        }
    }
}
impl Transport {
    /// Exact HTTPS origin only: no credentials, path, query or fragment. No redirect,
    /// environment proxy, cookies, referer, implicit retries or unbounded response body.
    pub fn new(origin: &str) -> TransportResult<Self> {
        Self::build(origin, None)
    }
    #[cfg(all(test, feature = "native-authority"))]
    pub(crate) fn test_with_root(
        origin: &str,
        root: reqwest::Certificate,
    ) -> TransportResult<Self> {
        Self::build(origin, Some(root))
    }
    fn build(origin: &str, extra_root: Option<reqwest::Certificate>) -> TransportResult<Self> {
        let endpoint = Url::parse(origin).map_err(|_| TransportError::Protocol)?;
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(TransportError::Protocol);
        }
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(5));
        if let Some(root) = extra_root {
            builder = builder.add_root_certificate(root);
        }
        Ok(Self {
            endpoint,
            http: builder.build().map_err(|_| TransportError::Protocol)?,
            running: {
                static GATES: OnceLock<Mutex<HashSet<SyncScope>>> = OnceLock::new();
                GATES.get_or_init(|| Mutex::new(HashSet::new()))
            },
        })
    }
    fn check<A: Authority>(
        &self,
        authority: &A,
        expected: &SyncScope,
        generation: u64,
    ) -> TransportResult<()> {
        if authority.scope()? != *expected || authority.generation()? != generation {
            Err(TransportError::Authority)
        } else {
            Ok(())
        }
    }
    fn url(&self, path: &str, query: &[(&str, String)]) -> TransportResult<Url> {
        let mut url = self
            .endpoint
            .join(path)
            .map_err(|_| TransportError::Protocol)?;
        if url.origin() != self.endpoint.origin() || !path.starts_with("/v1/") || url.path() != path
        {
            return Err(TransportError::Protocol);
        }
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
        Ok(url)
    }
    async fn request<O: Owner, A: Authority>(
        &self,
        owner: &O,
        session: &mut Session<'_, A>,
        method: Method,
        url: Url,
        body: Vec<u8>,
        limit: usize,
    ) -> TransportResult<(u16, Vec<u8>)> {
        self.check(session.authority, &session.scope, session.generation)?;
        owner.with_store(&session.scope, |_| Ok(()))?;
        if body.len() > session.remaining || body.len() > crypto::MAX_ENVELOPE_BYTES {
            return Err(TransportError::Quota);
        }
        session.remaining -= body.len();
        let auth = session
            .authority
            .authorize(method.as_str(), url.as_str(), &body)?;
        self.check(session.authority, &session.scope, session.generation)?;
        let mut response = self
            .http
            .request(method, url)
            .header(AUTHORIZATION, auth.bearer)
            .header("mdreader-proof", auth.proof)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|_| TransportError::Retryable)?;
        let status = response.status().as_u16();
        // Never retain or diagnose remote error bodies, URLs, tokens or raw reqwest errors.
        match status {
            200 | 201 => {}
            401 => {
                session
                    .authority
                    .rejected(AuthorityRejection::Authentication);
                return Err(TransportError::Authentication);
            }
            403 => {
                session.authority.rejected(AuthorityRejection::Forbidden);
                return Err(TransportError::Forbidden);
            }
            409 => return Err(TransportError::Conflict),
            413 | 507 => return Err(TransportError::Quota),
            408 | 429 | 500..=599 => return Err(TransportError::Retryable),
            _ => return Err(TransportError::Protocol),
        }
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(TransportError::Quota);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| TransportError::Retryable)?
        {
            if bytes.len().saturating_add(chunk.len()) > limit || chunk.len() > session.remaining {
                return Err(TransportError::Quota);
            }
            session.remaining -= chunk.len();
            bytes.extend_from_slice(&chunk);
        }
        self.check(session.authority, &session.scope, session.generation)?;
        owner.with_store(&session.scope, |_| Ok(()))?;
        self.check(session.authority, &session.scope, session.generation)?;
        Ok((status, bytes))
    }
}
#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
