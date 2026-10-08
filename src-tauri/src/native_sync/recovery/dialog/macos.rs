//! System-owned Standard Additions dialogs; no user secret in script arguments.
use super::{Kind, Platform};
const PICK_IMPORT: &str = r#"try
return POSIX path of (choose file with prompt "Choose your encrypted MDReader recovery file" of type {"json"})
on error number -128
return "MDREADER_CANCELLED"
end try"#;
const ENTER_SECRET: &str = r#"try
return text returned of (display dialog "Enter the separate 43-character MDReader recovery key. It stays in native memory." with title "Recover MDReader library" default answer "" with hidden answer buttons {"Cancel", "Recover"} default button "Recover" cancel button "Cancel")
on error number -128
return "MDREADER_CANCELLED"
end try"#;
const PICK_EXPORT: &str = r#"try
return POSIX path of (choose file name with prompt "Save encrypted MDReader recovery file — choose a new file" default name "mdreader-recovery.json")
on error number -128
return "MDREADER_CANCELLED"
end try"#;
const SHOW_SECRET: &str = r#"try
set recoveryKey to read POSIX file "/dev/stdin"
display dialog "Store this recovery key separately from the encrypted file. Anyone with BOTH can recover this library. Copy the key to your secure password manager before continuing." with title "MDReader recovery key" default answer recoveryKey buttons {"Cancel", "I stored the key separately"} default button "I stored the key separately" cancel button "Cancel"
set recoveryKey to ""
return "confirmed"
on error number -128
return "MDREADER_CANCELLED"
end try"#;

pub(super) fn available() -> bool {
    std::path::Path::new("/usr/bin/osascript").is_file()
}
pub(super) fn command(kind: Kind) -> super::Result<Platform> {
    let script = match kind {
        Kind::PickImport => PICK_IMPORT,
        Kind::EnterSecret => ENTER_SECRET,
        Kind::PickExport => PICK_EXPORT,
        Kind::ShowSecret => SHOW_SECRET,
    };
    let mut command = tokio::process::Command::new("/usr/bin/osascript");
    command.args(["-e", script]);
    Ok(Platform {
        command,
        cancelled_exit: None,
        empty_confirmation: false,
    })
}
