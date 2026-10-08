//! Desktop-only callback ownership; bind before browser and consume on any outcome.
use super::*;
use std::{
    collections::HashSet,
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::watch,
};
use zeroize::Zeroizing;

const MAX_HTTP: usize = 16 * 1024;
impl NativeAuthorityFactory {
    /// Native configuration supplies the exact registered IPv4 loopback redirect.
    /// Dropping this future closes the listener and pending PKCE attempt. Logout
    /// or session replacement cancels browser/callback/exchange work immediately.
    /// Mobile needs its system authentication adapter and fails before binding.
    pub async fn login_desktop(
        &self,
        registered_redirect: &str,
        browser: &dyn NativeBrowser,
        deadline: Instant,
    ) -> AuthorityResult<VerifiedLogin> {
        if cfg!(any(target_os = "ios", target_os = "android")) {
            return Err(AuthorityError::Unsupported);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_secs(300) {
            return Err(AuthorityError::Configuration);
        }
        let redirect = oidc::redirect(registered_redirect)?;
        let listener = TcpListener::bind((
            Ipv4Addr::LOCALHOST,
            redirect.port().ok_or(AuthorityError::Configuration)?,
        ))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                AuthorityError::Busy
            } else {
                AuthorityError::Interrupted
            }
        })?;
        let changes = self.controller.changes.subscribe();
        let pending = self.begin_login(registered_redirect)?;
        let epoch = pending.epoch;
        tokio::select! {
            biased;
            _ = changed(changes, epoch) => Err(AuthorityError::Inactive),
            result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                pending.open(browser).await?;
                let target = receive(&listener, &redirect).await?;
                drop(listener);
                self.verify_login(pending, &target).await
            }) => result.map_err(|_| AuthorityError::Interrupted)?,
        }
    }
}

async fn changed(mut receiver: watch::Receiver<u64>, epoch: u64) {
    let _ = receiver.wait_for(|current| *current != epoch).await;
}
async fn receive(
    listener: &TcpListener,
    redirect: &url::Url,
) -> AuthorityResult<Zeroizing<String>> {
    let (mut stream, peer) = listener
        .accept()
        .await
        .map_err(|_| AuthorityError::Interrupted)?;
    if peer.ip() != std::net::IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(AuthorityError::Identity);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        let mut chunk = Zeroizing::new([0; 1024]);
        let count = stream
            .read(&mut *chunk)
            .await
            .map_err(|_| AuthorityError::Interrupted)?;
        if count == 0 {
            return Err(AuthorityError::Identity);
        }
        if bytes.len().saturating_add(count) > MAX_HTTP {
            return Err(AuthorityError::Bounds);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            break;
        }
    }
    let target = parse(&bytes, redirect).map(|value| Zeroizing::new(value.to_owned()));
    let (status, body) = if target.is_ok() {
        (
            "200 OK",
            "Callback received. Return to MDReader to finish signing in.",
        )
    } else {
        (
            "400 Bad Request",
            "Callback rejected. Return to MDReader and retry.",
        )
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    // A browser that stops reading must not delay token verification/cleanup.
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    })
    .await;
    target
}
fn parse<'a>(bytes: &'a [u8], redirect: &url::Url) -> AuthorityResult<&'a str> {
    if bytes.len() > MAX_HTTP {
        return Err(AuthorityError::Bounds);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AuthorityError::Identity)?;
    let text = text
        .strip_suffix("\r\n\r\n")
        .ok_or(AuthorityError::Identity)?;
    let mut lines = text.split("\r\n");
    let mut request = lines.next().ok_or(AuthorityError::Identity)?.split(' ');
    if request.next() != Some("GET") {
        return Err(AuthorityError::Identity);
    }
    let target = request.next().ok_or(AuthorityError::Identity)?;
    if request.next() != Some("HTTP/1.1")
        || request.next().is_some()
        || !target.starts_with("/mdreader-sync/callback?")
        || target.contains('#')
        || target.bytes().any(|b| b.is_ascii_control())
    {
        return Err(AuthorityError::Identity);
    }
    let expected = format!(
        "127.0.0.1:{}",
        redirect.port().ok_or(AuthorityError::Configuration)?
    );
    let mut host = None;
    let mut names = HashSet::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(AuthorityError::Identity)?;
        let name = name.to_ascii_lowercase();
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !names.insert(name.clone())
            || value.chars().any(|c| c.is_control() && c != '\t')
        {
            return Err(AuthorityError::Identity);
        }
        match name.as_str() {
            "host" => host = Some(value.trim_matches([' ', '\t'])),
            "content-length" | "transfer-encoding" => return Err(AuthorityError::Identity),
            _ => {}
        }
    }
    if host != Some(expected.as_str()) {
        return Err(AuthorityError::Identity);
    }
    Ok(target)
}

#[cfg(test)]
#[path = "loopback_tests.rs"]
mod tests;
