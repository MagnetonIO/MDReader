//! TLS trust is scoped to one child test process, never a permissive HTTP client.
use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
};
pub(super) const CASE: &str =
    "accounts::acceptance::actual_actor_pkce_recovery_pump_and_retained_projection_cancel";
pub(super) struct Fixture {
    child: Child,
    _temp: tempfile::TempDir,
}
impl Fixture {
    pub fn child_test() {
        let web = std::env::var_os("MDREADER_SYNC_WEB_WORKTREE")
            .expect("explicit reviewed web fixture required");
        let temp = tempfile::tempdir().unwrap();
        let mut child = Command::new("node")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../mdreader-sync/tests/support/worker-bridge.mjs"
            ))
            .arg(web)
            .arg(temp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let guard = Self { child, _temp: temp };
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(output).take(4096).read_line(&mut line);
            let _ = send.send(result.map(|_| line));
        });
        let ready = receive
            .recv_timeout(Duration::from_secs(15))
            .expect("local Worker startup deadline")
            .unwrap();
        let ready: serde_json::Value =
            serde_json::from_str(&ready).expect("fixture ready metadata");
        let empty = guard._temp.path().join("empty-certs");
        std::fs::create_dir(&empty).unwrap();
        let mut test = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CASE, "--ignored", "--nocapture"])
            .env(
                "MDREADER_ACTOR_CHILD_ORIGIN",
                ready["origin"].as_str().unwrap(),
            )
            .env("SSL_CERT_FILE", ready["cert"].as_str().unwrap())
            .env("SSL_CERT_DIR", empty)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(90);
        let status = loop {
            if let Some(status) = test.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= until {
                test.kill().unwrap();
                let _ = test.wait();
                panic!("actor acceptance deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "actor subprocess failed");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
        }
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
// URL, callbacks and encrypted fixture body travel only on stdin. No shell,
// token-bearing process arguments, redirects, proxy or secret diagnostics.
pub(super) fn curl(url: &str, body: Option<&str>, headers: bool) -> Vec<u8> {
    let quote = |s: &str| serde_json::to_string(s).unwrap();
    let mut config = format!(
        "url = {}\ncacert = {}\n",
        quote(url),
        quote(&std::env::var("SSL_CERT_FILE").unwrap())
    );
    if let Some(body) = body {
        config.push_str(&format!("request = POST\ndata-binary = {}\n", quote(body)));
    }
    if headers {
        config.push_str("dump-header = -\noutput = /dev/null\n");
    }
    let mut child = Command::new("curl")
        .args([
            "--config",
            "-",
            "--silent",
            "--fail",
            "--noproxy",
            "*",
            "--max-time",
            "8",
            "--max-filesize",
            "262144",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(config.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "synthetic HTTP request failed");
    assert!(output.stdout.len() <= 262144);
    output.stdout
}
pub(super) fn peer(origin: &str, action: &str) -> serde_json::Value {
    serde_json::from_slice(&curl(
        &format!("{origin}/__synthetic/account-peer"),
        Some(&format!("{{\"action\":\"{action}\"}}")),
        false,
    ))
    .unwrap()
}
pub(super) struct Browser;
impl NativeBrowser for Browser {
    fn open<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = AuthorityResult<()>> + Send + 'a>> {
        let url = url.to_owned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let headers = curl(&url, None, true);
                let text = std::str::from_utf8(&headers).map_err(|_| AuthorityError::Identity)?;
                let location = text
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("location: ")
                            .or_else(|| line.strip_prefix("Location: "))
                    })
                    .ok_or(AuthorityError::Identity)?;
                let callback =
                    url::Url::parse(location.trim()).map_err(|_| AuthorityError::Identity)?;
                let authorize = url::Url::parse(&url).map_err(|_| AuthorityError::Identity)?;
                let registered = authorize
                    .query_pairs()
                    .find(|(k, _)| k == "redirect_uri")
                    .ok_or(AuthorityError::Identity)?
                    .1
                    .into_owned();
                let mut base = callback.clone();
                base.set_query(None);
                if base.as_str() != registered
                    || callback.host_str() != Some("127.0.0.1")
                    || callback.scheme() != "http"
                    || callback.fragment().is_some()
                {
                    return Err(AuthorityError::Identity);
                }
                let mut stream = TcpStream::connect((
                    "127.0.0.1",
                    callback.port().ok_or(AuthorityError::Identity)?,
                ))
                .map_err(|_| AuthorityError::Interrupted)?;
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                write!(
                    stream,
                    "GET {}?{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                    callback.path(),
                    callback.query().ok_or(AuthorityError::Identity)?,
                    callback.port().unwrap()
                )
                .map_err(|_| AuthorityError::Interrupted)?;
                // Browser launch returns before the callback listener receives.
                // Request is already buffered; native code verifies it normally.
                Ok(())
            })
            .await
            .map_err(|_| AuthorityError::Interrupted)?
        })
    }
}
