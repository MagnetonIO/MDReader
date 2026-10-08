//! OS Standard Additions UI. Scripts contain only public copy; secrets cross
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
pub(super) const PICK_IMPORT: &str = r#"try
return POSIX path of (choose file with prompt "Choose your encrypted MDReader recovery file" of type {"json"})
on error number -128
return "MDREADER_CANCELLED"
end try"#;
pub(super) const ENTER_SECRET: &str = r#"try
return text returned of (display dialog "Enter the separate 43-character MDReader recovery key. It stays in native memory." with title "Recover MDReader library" default answer "" with hidden answer buttons {"Cancel", "Recover"} default button "Recover" cancel button "Cancel")
on error number -128
return "MDREADER_CANCELLED"
end try"#;
pub(super) const PICK_EXPORT: &str = r#"try
return POSIX path of (choose file name with prompt "Save encrypted MDReader recovery file — choose a new file" default name "mdreader-recovery.json")
on error number -128
return "MDREADER_CANCELLED"
end try"#;
pub(super) const SHOW_SECRET: &str = r#"try
set recoveryKey to read POSIX file "/dev/stdin"
display dialog "Store this recovery key separately from the encrypted file. Anyone with BOTH can recover this library. Copy the key to your secure password manager before continuing." with title "MDReader recovery key" default answer recoveryKey buttons {"Cancel", "I stored the key separately"} default button "I stored the key separately" cancel button "Cancel"
set recoveryKey to ""
return "confirmed"
on error number -128
return "MDREADER_CANCELLED"
end try"#;

pub(super) async fn run(
    script: &'static str,
    input: Option<Zeroizing<String>>,
    cancel: &Arc<Cancel>,
    bound: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    cancel.check()?;
    let mut child = tokio::process::Command::new("/usr/bin/osascript")
        .args(["-e", script])
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
            if !child
                .wait()
                .await
                .map_err(|_| AccountError::Interrupted)?
                .success()
            {
                return Err(AccountError::Interrupted);
            }
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
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
