use super::*;
use crate::transport::Authorization;
use reqwest::{
    header::{AUTHORIZATION, CONTENT_TYPE},
    Method,
};
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

pub(super) struct Http {
    client: reqwest::Client,
}
impl Http {
    pub(super) fn new(extra_root: Option<reqwest::Certificate>) -> AuthorityResult<Self> {
        let mut b = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .retry(reqwest::retry::never())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15));
        if let Some(root) = extra_root {
            b = b.add_root_certificate(root);
        }
        Ok(Self {
            client: b.build().map_err(|_| AuthorityError::Configuration)?,
        })
    }
    pub(super) async fn send(
        &self,
        method: Method,
        url: Url,
        body: Zeroizing<Vec<u8>>,
        auth: Option<Authorization>,
        content_type: &str,
        limit: usize,
    ) -> AuthorityResult<Zeroizing<Vec<u8>>> {
        if url.scheme() != "https" || body.len() > 64 * 1024 || limit > 64 * 1024 {
            return Err(AuthorityError::Bounds);
        }
        let mut request = self
            .client
            .request(method, url)
            .header(CONTENT_TYPE, content_type);
        if let Some(auth) = auth {
            let (bearer, proof, login) = auth.into_headers();
            request = request
                .header(AUTHORIZATION, bearer)
                .header("mdreader-proof", proof);
            if let Some(login) = login {
                request = request.header("mdreader-login-token", login);
            }
        }
        // reqwest owns a transient copy; never claim comprehensive library-buffer wiping.
        let mut response = request
            .body(body.to_vec())
            .send()
            .await
            .map_err(|_| AuthorityError::Interrupted)?;
        match response.status().as_u16() {
            200 | 201 => {}
            401 | 403 => return Err(AuthorityError::Enrollment),
            408 | 429 | 500..=599 => return Err(AuthorityError::Interrupted),
            _ => return Err(AuthorityError::Identity),
        }
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(AuthorityError::Bounds);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AuthorityError::Interrupted)?
        {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(AuthorityError::Bounds);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
pub(super) fn https(value: &str, origin_only: bool) -> AuthorityResult<Url> {
    if value.len() > 2048 || value.bytes().any(|b| b.is_ascii_control()) {
        return Err(AuthorityError::Configuration);
    }
    let url = Url::parse(value).map_err(|_| AuthorityError::Configuration)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (origin_only && url.path() != "/")
    {
        return Err(AuthorityError::Configuration);
    }
    Ok(url)
}
