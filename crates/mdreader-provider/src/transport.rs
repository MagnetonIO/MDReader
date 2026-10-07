//! Backend-only bounded HTTP transport. These values are never reader DTOs or
//! durable configuration. Request Debug never includes headers, URLs or bodies.
use crate::*;
use async_trait::async_trait;
use std::collections::BTreeMap;
pub struct Request {
    method: Method,
    url: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
}
impl Request {
    pub fn new(
        method: Method,
        url: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<Self> {
        if !url.starts_with("https://")
            || url.len() > 16384
            || url.chars().any(char::is_control)
            || body.len() > MAX_BODY_BYTES
        {
            return Err(ProviderError::Invalid);
        }
        validate_headers(&headers)?;
        Ok(Self {
            method,
            url,
            headers,
            body,
        })
    }
    pub fn method(&self) -> Method {
        self.method
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for value in self.headers.values_mut() {
            value.zeroize();
        }
    }
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}
pub struct Response {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
fn validate_headers(headers: &BTreeMap<String, String>) -> Result<()> {
    if headers.len() > 128
        || headers.iter().any(|(k, v)| {
            k.is_empty()
                || k.len() > 256
                || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || v.len() > 16384
                || v.chars().any(char::is_control)
        })
        || headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 64 * 1024
    {
        return Err(ProviderError::Protocol);
    }
    Ok(())
}
impl Response {
    pub fn new(status: u16, headers: BTreeMap<String, String>, body: Vec<u8>) -> Result<Self> {
        if !(100..=599).contains(&status) || body.len() > 8 * 1024 * 1024 {
            return Err(ProviderError::TooLarge);
        }
        validate_headers(&headers)?;
        let mut normalized = BTreeMap::new();
        for (k, v) in headers {
            if normalized.insert(k.to_ascii_lowercase(), v).is_some() {
                return Err(ProviderError::Protocol);
            }
        }
        Ok(Self {
            status,
            headers: normalized,
            body,
        })
    }
    pub fn status(&self) -> u16 {
        self.status
    }
    pub fn headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }
}
impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}
#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, request: Request) -> Result<Response>;
}
/// Deadline/cancellation after mutation dispatch conservatively means Unknown.
/// The function never retries, even after transport/HTTP/authentication failure.
pub async fn send(
    transport: &dyn Transport,
    request: Request,
    context: &IoContext,
    mutation: Option<uuid::Uuid>,
) -> Result<Response> {
    context.check()?;
    let result = tokio::select! {
        biased;
        _=context.cancellation().cancelled()=>Err(ProviderError::Cancelled),
        result=tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),transport.send(request))=>match result{Ok(result)=>result,Err(_)=>Err(ProviderError::Timeout)},
    };
    let response = result.map_err(|error| {
        mutation
            .map(|operation_id| ProviderError::UnknownWrite { operation_id })
            .unwrap_or(error)
    })?;
    if response.body.len()
        + response
            .headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
        > context.byte_limit()
    {
        return Err(mutation
            .map(|operation_id| ProviderError::UnknownWrite { operation_id })
            .unwrap_or(ProviderError::TooLarge));
    }
    context.check().map_err(|error| {
        mutation
            .map(|operation_id| ProviderError::UnknownWrite { operation_id })
            .unwrap_or(error)
    })?;
    Ok(response)
}
pub fn read_failure(response: &Response) -> ProviderError {
    match response.status {
        401 => ProviderError::ReconnectRequired,
        403 => ProviderError::Unauthorized,
        404 => ProviderError::Unavailable,
        429 => ProviderError::RateLimited {
            retry_after_ms: response
                .headers
                .get("retry-after")
                .and_then(|s| s.parse::<u64>().ok())
                .and_then(|n| n.checked_mul(1000)),
        },
        _ => ProviderError::Protocol,
    }
}
#[cfg(feature = "http-client")]
pub struct ReqwestTransport(reqwest::Client);
#[cfg(feature = "http-client")]
impl ReqwestTransport {
    pub fn new() -> Result<Self> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map(Self)
            .map_err(|_| ProviderError::Unavailable)
    }
}
#[cfg(feature = "http-client")]
#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, mut request: Request) -> Result<Response> {
        use futures_util::StreamExt;
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Patch => reqwest::Method::PATCH,
        };
        let mut builder = self.0.request(method, &request.url);
        for (k, v) in std::mem::take(&mut request.headers) {
            let value = zeroize::Zeroizing::new(v);
            let mut header = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| ProviderError::Invalid)?;
            if k.eq_ignore_ascii_case("authorization") {
                header.set_sensitive(true);
            }
            builder = builder.header(k, header);
        }
        let response = builder
            .body(std::mem::take(&mut request.body))
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        if response
            .content_length()
            .is_some_and(|n| n > 8 * 1024 * 1024)
        {
            return Err(ProviderError::TooLarge);
        }
        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (k, v) in response.headers() {
            let value = v.to_str().map_err(|_| ProviderError::Protocol)?.to_string();
            if headers.insert(k.as_str().to_string(), value).is_some() {
                return Err(ProviderError::Protocol);
            }
        }
        validate_headers(&headers)?;
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ProviderError::Unavailable)?;
            if body.len().saturating_add(chunk.len()) > 8 * 1024 * 1024 {
                return Err(ProviderError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Response::new(status, headers, body)
    }
}
