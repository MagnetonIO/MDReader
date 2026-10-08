//! Bounded system-browser dispatch shared by native OAuth owners.
//! No shell, URL logging, detached blocking task or embedded-webview login.
use std::{process::Stdio, time::Duration};
use tokio::{process::Child, sync::Semaphore};
use url::Url;

static DISPATCH: Semaphore = Semaphore::const_new(1);

#[derive(Clone, Copy)]
pub(crate) enum Error {
    Busy,
    Invalid,
    Unavailable,
}

pub(crate) async fn open(url: &Url) -> Result<(), Error> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 16384
    {
        return Err(Error::Invalid);
    }
    let _permit = DISPATCH.try_acquire().map_err(|_| Error::Busy)?;
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut command = tokio::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(Error::Unavailable);
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let child = command
            .arg(url.as_str())
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        wait(child, Duration::from_secs(5)).await
    }
}

async fn wait(mut child: Child, budget: Duration) -> Result<(), Error> {
    match tokio::time::timeout(budget, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        // Kill and reap on deadline; cancellation also kills through kill_on_drop.
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            Err(Error::Unavailable)
        }
        _ => Err(Error::Unavailable),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn launcher_deadline_kills_and_reaps_the_owned_process() {
        let child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        assert!(wait(child, Duration::from_millis(10)).await.is_err());
        assert!(!std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        // Rejected input never reaches the real system browser.
        assert!(matches!(
            open(&Url::parse("http://example.invalid/").unwrap()).await,
            Err(Error::Invalid)
        ));
    }
}
