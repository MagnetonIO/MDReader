//! GNOME's secure native password dialog and read-only text export. No shell,
//! plaintext temporary file or clipboard automation. Missing Zenity stays gated.
use super::{Kind, Platform};
pub(super) fn available() -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata("/usr/bin/zenity")
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
pub(super) fn command(kind: Kind) -> super::Result<Platform> {
    let mut command = tokio::process::Command::new("/usr/bin/zenity");
    match kind {
        Kind::PickImport => {
            command.args([
                "--file-selection",
                "--title=Choose encrypted MDReader recovery file",
                "--file-filter=Recovery JSON | *.json",
            ]);
        }
        Kind::PickExport => {
            command.args([
                "--file-selection",
                "--save",
                "--filename=mdreader-recovery.json",
                "--title=Save MDReader recovery — choose a new file",
            ]);
        }
        Kind::EnterSecret => {
            command.args([
                "--password",
                "--title=Enter the separate 43-character MDReader recovery key",
                "--ok-label=Recover",
            ]);
        }
        Kind::ShowSecret => {
            command.args([
                "--text-info",
                "--filename=/dev/stdin",
                "--title=Store the recovery key separately from the encrypted file",
                "--checkbox=I stored this key separately in my password manager",
                "--ok-label=Continue",
                "--width=640",
                "--height=240",
            ]);
        }
    }
    Ok(Platform {
        command,
        cancelled_exit: Some(1),
        empty_confirmation: true,
    })
}
