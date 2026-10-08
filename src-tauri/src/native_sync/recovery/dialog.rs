//! Owned desktop system-dialog processes. Scripts contain only public copy; secrets cross
//! private stdin/stdout pipes, never argv, environment, shell or WebView IPC.
//! Rust buffers are zeroized; transient OS dialog/process copies cannot be
//! erased by Rust and exist only for the owned process lifetime.
use super::owner::Cancel;
use mdreader_service::accounts::AccountError;
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, AccountError>;
const CANCELLED: &[u8] = b"MDREADER_CANCELLED";
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;
#[derive(Clone, Copy)]
pub(super) enum Kind {
    PickImport,
    EnterSecret,
    PickExport,
    ShowSecret,
}
pub(super) struct Platform {
    command: tokio::process::Command,
    cancelled_exit: Option<i32>,
    empty_confirmation: bool,
}
pub(super) fn available() -> bool {
    platform::available()
}

pub(super) async fn run(
    kind: Kind,
    input: Option<Zeroizing<String>>,
    cancel: &Arc<Cancel>,
    bound: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    cancel.check()?;
    let Platform {
        mut command,
        cancelled_exit,
        empty_confirmation,
    } = platform::command(kind)?;
    let mut child = command
        .kill_on_drop(true)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| AccountError::Unsupported)?;
    let result = {
        let operation = async {
            if let Some(input) = input {
                let mut stdin = child.stdin.take().ok_or(AccountError::Interrupted)?;
                stdin
                    .write_all(input.as_bytes())
                    .await
                    .map_err(|_| AccountError::Interrupted)?;
                stdin
                    .shutdown()
                    .await
                    .map_err(|_| AccountError::Interrupted)?;
            }
            let mut bytes = Zeroizing::new(Vec::new());
            child
                .stdout
                .take()
                .ok_or(AccountError::Interrupted)?
                .take(bound as u64 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| AccountError::Interrupted)?;
            if bytes.len() > bound {
                return Err(AccountError::Limit);
            }
            let status = child.wait().await.map_err(|_| AccountError::Interrupted)?;
            if !status.success() {
                return Err(
                    if status.code().is_some() && status.code() == cancelled_exit {
                        AccountError::Cancelled
                    } else {
                        AccountError::Interrupted
                    },
                );
            }
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            if bytes.is_empty() && empty_confirmation && matches!(kind, Kind::ShowSecret) {
                bytes.extend_from_slice(b"confirmed");
            }
            if bytes.as_slice() == CANCELLED {
                return Err(AccountError::Cancelled);
            }
            Ok(bytes)
        };
        tokio::pin!(operation);
        tokio::select! {
            biased;
            _ = cancel.wait() => Err(AccountError::Cancelled),
            _ = tokio::time::sleep(Duration::from_secs(180)) => Err(AccountError::Interrupted),
            value = &mut operation => value,
        }
    };
    if result.is_err() {
        let _ = child.start_kill();
        // Actual owned task awaits cleanup; actor cancellation does not detach
        // the child or mistake dropping its request for a settled dialog.
        let _ = child.wait().await;
    }
    cancel.check()?;
    result
}
