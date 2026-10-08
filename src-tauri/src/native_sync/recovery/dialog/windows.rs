//! System Windows PowerShell hosts WPF dialogs. Only fixed public scripts enter
//! argv; independent secrets cross private process pipes. No execution-policy
//! bypass, user profile scripts, WebView prompt or clipboard fallback.
use super::{Kind, Platform};
use mdreader_service::accounts::AccountError;
use std::path::PathBuf;
fn executable() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SystemRoot")?);
    if !root.is_absolute() {
        return None;
    }
    Some(root.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
}
pub(super) fn available() -> bool {
    executable().is_some_and(|path| path.is_file())
}
const PRELUDE: &str = r#"$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName PresentationFramework
"#;
const PICK_IMPORT: &str = r#"$dialog = [Microsoft.Win32.OpenFileDialog]::new()
$dialog.Title = 'Choose encrypted MDReader recovery file'
$dialog.Filter = 'MDReader recovery JSON|*.json'
$dialog.Multiselect = $false
if ($dialog.ShowDialog() -ne $true) { [Console]::Out.Write('MDREADER_CANCELLED'); exit 0 }
[Console]::Out.Write($dialog.FileName)
"#;
const PICK_EXPORT: &str = r#"$dialog = [Microsoft.Win32.SaveFileDialog]::new()
$dialog.Title = 'Save MDReader recovery - choose a new file'
$dialog.FileName = 'mdreader-recovery.json'
$dialog.DefaultExt = '.json'
$dialog.Filter = 'MDReader recovery JSON|*.json'
if ($dialog.ShowDialog() -ne $true) { [Console]::Out.Write('MDREADER_CANCELLED'); exit 0 }
[Console]::Out.Write($dialog.FileName)
"#;
const WINDOW: &str = r#"$window = [System.Windows.Window]::new()
$window.Title = 'MDReader recovery key'
$window.Width = 620; $window.SizeToContent = 'Height'; $window.ResizeMode = 'NoResize'
$window.WindowStartupLocation = 'CenterScreen'; $window.Topmost = $true
$panel = [System.Windows.Controls.StackPanel]::new(); $panel.Margin = '20'
$window.Content = $panel
$label = [System.Windows.Controls.TextBlock]::new(); $label.TextWrapping = 'Wrap'; $label.Margin = '0,0,0,12'
[void]$panel.Children.Add($label)
"#;
const ENTER_SECRET: &str = r#"$label.Text = 'Enter the separate 43-character recovery key. It stays in native memory.'
$entry = [System.Windows.Controls.PasswordBox]::new(); $entry.MaxLength = 43; $entry.Margin = '0,0,0,12'
[void]$panel.Children.Add($entry)
$ok = [System.Windows.Controls.Button]::new(); $ok.Content = 'Recover'; $ok.IsDefault = $true; $ok.Margin = '0,0,0,8'
$ok.Add_Click({ $window.DialogResult = $true })
[void]$panel.Children.Add($ok)
$cancel = [System.Windows.Controls.Button]::new(); $cancel.Content = 'Cancel'; $cancel.IsCancel = $true
[void]$panel.Children.Add($cancel)
if ($window.ShowDialog() -ne $true) { $entry.Clear(); [Console]::Out.Write('MDREADER_CANCELLED'); exit 0 }
[Console]::Out.Write($entry.Password)
$entry.Clear()
"#;
const SHOW_SECRET: &str = r#"$label.Text = 'Store this key separately from the encrypted file. Anyone with BOTH can recover this library. Copy the key to your secure password manager before continuing.'
$entry = [System.Windows.Controls.TextBox]::new(); $entry.IsReadOnly = $true; $entry.Margin = '0,0,0,12'
$entry.Text = [Console]::In.ReadToEnd()
[void]$panel.Children.Add($entry)
$confirmed = [System.Windows.Controls.CheckBox]::new(); $confirmed.Content = 'I stored the key separately'; $confirmed.Margin = '0,0,0,12'
[void]$panel.Children.Add($confirmed)
$ok = [System.Windows.Controls.Button]::new(); $ok.Content = 'Continue'; $ok.IsEnabled = $false; $ok.Margin = '0,0,0,8'
$confirmed.Add_Checked({ $ok.IsEnabled = $true }); $confirmed.Add_Unchecked({ $ok.IsEnabled = $false })
$ok.Add_Click({ $window.DialogResult = $true }); [void]$panel.Children.Add($ok)
$cancel = [System.Windows.Controls.Button]::new(); $cancel.Content = 'Cancel'; $cancel.IsCancel = $true
[void]$panel.Children.Add($cancel)
if ($window.ShowDialog() -ne $true) { $entry.Clear(); [Console]::Out.Write('MDREADER_CANCELLED'); exit 0 }
$entry.Clear()
[Console]::Out.Write('confirmed')
"#;
pub(super) fn command(kind: Kind) -> super::Result<Platform> {
    let executable = executable().ok_or(AccountError::Unsupported)?;
    let script = match kind {
        Kind::PickImport => format!("{PRELUDE}{PICK_IMPORT}"),
        Kind::PickExport => format!("{PRELUDE}{PICK_EXPORT}"),
        Kind::EnterSecret => format!("{PRELUDE}{WINDOW}{ENTER_SECRET}"),
        Kind::ShowSecret => format!("{PRELUDE}{WINDOW}{SHOW_SECRET}"),
    };
    let mut command = tokio::process::Command::new(executable);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-STA",
        "-Command",
        &script,
    ]);
    Ok(Platform {
        command,
        cancelled_exit: None,
        empty_confirmation: false,
    })
}
