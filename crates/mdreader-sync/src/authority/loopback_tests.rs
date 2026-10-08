use super::*;
use crate::authority::{custody::test_support::MemoryVault, tests::LOGIN_TEST};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn factory() -> NativeAuthorityFactory {
    let issuer = "https://issuer.example.test/";
    NativeAuthorityFactory::new(
        OidcClient::new(
            ReviewedOidcRegistration::new(
                issuer,
                "synthetic-native",
                OidcEndpoints {
                    authorization: format!("{issuer}authorize"),
                    token: format!("{issuer}token"),
                    jwks: format!("{issuer}jwks"),
                },
                "https://sync.example.test",
                "https://mdreader.dev/sync",
            )
            .unwrap(),
        )
        .unwrap(),
        SyncVaultClient::new(Arc::new(MemoryVault::new())).unwrap(),
        SessionController::new(),
    )
}
struct Browser {
    calls: AtomicUsize,
    logout: Option<Arc<SessionController>>,
}
impl NativeBrowser for Browser {
    fn open<'a>(
        &'a self,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = AuthorityResult<()>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(controller) = &self.logout {
                controller.logout()?;
            }
            Ok(())
        })
    }
}
fn listener() -> (std::net::TcpListener, String) {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let redirect = format!(
        "http://127.0.0.1:{}/mdreader-sync/callback",
        listener.local_addr().unwrap().port()
    );
    (listener, redirect)
}
#[tokio::test]
async fn collision_precedes_browser_and_logout_closes_listener_and_consumes_login() {
    let _serial = LOGIN_TEST.lock().await;
    let factory = factory();
    let browser = Browser {
        calls: AtomicUsize::new(0),
        logout: Some(factory.controller.clone()),
    };
    let (occupied, redirect) = listener();
    assert_eq!(
        factory
            .login_desktop(&redirect, &browser, Instant::now() + Duration::from_secs(5))
            .await
            .unwrap_err(),
        AuthorityError::Busy
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 0);
    drop(occupied);
    assert_eq!(
        factory
            .login_desktop(&redirect, &browser, Instant::now() + Duration::from_secs(5))
            .await
            .unwrap_err(),
        AuthorityError::Inactive
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 1);
    let rebound = TcpListener::bind(
        oidc::redirect(&redirect)
            .unwrap()
            .socket_addrs(|| None)
            .unwrap()[0],
    )
    .await
    .unwrap();
    drop(rebound);
    drop(factory.begin_login(&redirect).unwrap());
}
#[test]
fn callback_http_rejects_wrong_host_framing_duplicates_and_pipelining() {
    let redirect = oidc::redirect("http://127.0.0.1:43891/mdreader-sync/callback").unwrap();
    let valid = "GET /mdreader-sync/callback?state=synthetic&code=synthetic HTTP/1.1\r\nHost: 127.0.0.1:43891\r\n\r\n";
    assert_eq!(
        parse(valid.as_bytes(), &redirect).unwrap(),
        "/mdreader-sync/callback?state=synthetic&code=synthetic"
    );
    for invalid in [
        valid.replace("GET ", "POST "),
        valid.replace("HTTP/1.1", "HTTP/1.0"),
        valid.replace("Host:", "Host: attacker.invalid\r\nHost:"),
        valid.replace("127.0.0.1:43891", "localhost:43891"),
        valid.replace("\r\n\r\n", "\r\nContent-Length: 0\r\n\r\n"),
        valid.replace("\r\n\r\n", "\r\nTransfer-Encoding: chunked\r\n\r\n"),
        format!("{valid}{valid}"),
        valid.replace("/mdreader-sync/callback?", "https://attacker.invalid/?"),
        valid.replace("state=synthetic", "state=synthetic#fragment"),
        valid.trim_end().to_owned(),
    ] {
        assert!(parse(invalid.as_bytes(), &redirect).is_err());
    }
    assert_eq!(
        parse(&vec![b'x'; MAX_HTTP + 1], &redirect).unwrap_err(),
        AuthorityError::Bounds
    );
}

#[tokio::test]
async fn deadline_consumes_pending_attempt_and_releases_port() {
    let _serial = LOGIN_TEST.lock().await;
    let factory = factory();
    let browser = Browser {
        calls: AtomicUsize::new(0),
        logout: None,
    };
    let (occupied, redirect) = listener();
    drop(occupied);
    assert_eq!(
        factory
            .login_desktop(
                &redirect,
                &browser,
                Instant::now() + Duration::from_millis(100)
            )
            .await
            .unwrap_err(),
        AuthorityError::Interrupted
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 1);
    let rebound = TcpListener::bind(
        oidc::redirect(&redirect)
            .unwrap()
            .socket_addrs(|| None)
            .unwrap()[0],
    )
    .await
    .unwrap();
    drop(rebound);
    drop(factory.begin_login(&redirect).unwrap());
}
