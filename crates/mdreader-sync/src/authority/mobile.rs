//! Native mobile callback consumption. No loopback server, embedded WebView,
//! renderer callback, issuer discovery or relaxation of token/PKCE verification.
use super::*;
#[cfg(any(target_os = "ios", target_os = "android"))]
use std::time::Duration;
use std::time::Instant;
use url::Url;
#[cfg(any(target_os = "ios", target_os = "android"))]
use zeroize::Zeroizing;

pub(super) fn redirect(value: &str) -> AuthorityResult<Url> {
    if value.len() > 2048 || value.bytes().any(|b| b.is_ascii_control()) {
        return Err(AuthorityError::Configuration);
    }
    let url = Url::parse(value).map_err(|_| AuthorityError::Configuration)?;
    let scheme = url.scheme();
    if !scheme.ends_with(".auth")
        || !scheme.contains('.')
        || scheme.starts_with('.')
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
        || url.host_str() != Some("callback")
        || url.path() != "/mdreader-sync"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AuthorityError::Configuration);
    }
    Ok(url)
}
#[cfg(any(target_os = "ios", target_os = "android"))]
fn target(callback: &str, expected: &Url) -> AuthorityResult<Zeroizing<String>> {
    if callback.len() > 16384 || callback.bytes().any(|b| b.is_ascii_control()) {
        return Err(AuthorityError::Identity);
    }
    let actual = Url::parse(callback).map_err(|_| AuthorityError::Identity)?;
    if actual.scheme() != expected.scheme()
        || actual.host_str() != expected.host_str()
        || actual.port() != expected.port()
        || actual.path() != expected.path()
        || !actual.username().is_empty()
        || actual.password().is_some()
        || actual.fragment().is_some()
    {
        return Err(AuthorityError::Identity);
    }
    Ok(Zeroizing::new(format!(
        "{}?{}",
        actual.path(),
        actual.query().ok_or(AuthorityError::Identity)?
    )))
}
impl NativeAuthorityFactory {
    pub async fn login_native(
        &self,
        registered_redirect: &str,
        browser: &dyn NativeBrowser,
        deadline: Instant,
    ) -> AuthorityResult<VerifiedLogin> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            self.login_desktop(registered_redirect, browser, deadline)
                .await
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            self.login_mobile(registered_redirect, browser, deadline, None)
                .await
        }
    }
    pub async fn login_enrollment_native(
        &self,
        enrollment: &EnrollmentLogin,
        registered_redirect: &str,
        browser: &dyn NativeBrowser,
        deadline: Instant,
    ) -> AuthorityResult<VerifiedLogin> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            self.login_enrollment_desktop(enrollment, registered_redirect, browser, deadline)
                .await
        }
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            self.login_mobile(registered_redirect, browser, deadline, Some(enrollment))
                .await
        }
    }
    #[cfg(any(target_os = "ios", target_os = "android"))]
    async fn login_mobile(
        &self,
        registered_redirect: &str,
        browser: &dyn NativeBrowser,
        deadline: Instant,
        enrollment: Option<&EnrollmentLogin>,
    ) -> AuthorityResult<VerifiedLogin> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_secs(300) {
            return Err(AuthorityError::Configuration);
        }
        let redirect = redirect(registered_redirect)?;
        let pending = match enrollment {
            Some(binding) => self.begin_enrollment_login(binding, registered_redirect)?,
            None => self.begin_login(registered_redirect)?,
        };
        let epoch = pending.epoch;
        let mut changes = self.controller.changes.subscribe();
        tokio::select! {
            biased;
            _ = changes.wait_for(|current| *current != epoch) => Err(AuthorityError::Inactive),
            result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                pending.open(browser).await?;
                let callback = browser.callback().await?;
                let target = target(&callback, &redirect)?;
                self.verify_login(pending, &target).await
            }) => result.map_err(|_| AuthorityError::Interrupted)?,
        }
    }
}
