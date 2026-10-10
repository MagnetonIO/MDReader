# Portable compiler-metadata admission only: no Cargo, Windows DLLs or profiles.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-test-features.ps1')
$workspace = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) 'mdreader synthetic compiler workspace'))
function Metadata([string[]]$ProfileFeatures) {
  # Roundtrip Cargo's public JSON shape, including an unrelated build-script row.
  $records = @(
    @{ reason = 'build-script-executed' },
    @{ reason = 'compiler-artifact'; target = @{name = 'mdreader_app_lib'; kind = @('rlib', 'cdylib', 'staticlib')};
       profile = @{ test = $true }; executable = (Join-Path $workspace 'target/debug/deps/mdreader_app_lib-0123456789abcdef.exe');
       manifest_path = (Join-Path $workspace 'src-tauri/Cargo.toml'); features = @('default', 'google-drive-connector', 'dropbox-connector') },
    @{ reason = 'compiler-artifact'; target = @{name = 'mdreader_profile'; kind = @('lib')};
       profile = @{test = $false}; executable = $null;
       manifest_path = (Join-Path $workspace 'crates/mdreader-profile/Cargo.toml'); features = @($ProfileFeatures) }
  )
  return @($records | ConvertTo-Json -Depth 8 | ConvertFrom-Json -Depth 8)
}
function Reject([scriptblock]$Operation) {
  $failed = $false
  try { & $Operation | Out-Null } catch { $failed = $true }
  if (!$failed) { throw 'Wrong compiler feature/artifact provenance was accepted.' }
}
$normal = Metadata @()
$diagnostic = Metadata @('admission-test-diagnostics')
$receipt = Assert-WindowsTestFeatures -Messages $normal -Workspace $workspace
if ($receipt.admissionDiagnostics) { throw 'Default harness was labelled diagnostic.' }
$receipt = Assert-WindowsTestFeatures -Messages $diagnostic -Workspace $workspace -AdmissionDiagnostics
if (!$receipt.admissionDiagnostics -or $receipt.profileFeatures.Count -ne 1) { throw 'Exact diagnostic feature was lost.' }
Reject { Assert-WindowsTestFeatures -Messages $normal -Workspace $workspace -AdmissionDiagnostics }
Reject { Assert-WindowsTestFeatures -Messages $diagnostic -Workspace $workspace }
Reject { Assert-WindowsTestFeatures -Messages @($normal | Select-Object -First 2) -Workspace $workspace }
Reject { Assert-WindowsTestFeatures -Messages @($normal + @($normal[2])) -Workspace $workspace }
$unknown = Metadata @('admission-test-diagnostics', 'unsupported-feature')
Reject { Assert-WindowsTestFeatures -Messages $unknown -Workspace $workspace -AdmissionDiagnostics }
$foreign = Metadata @()
$foreign[2].manifest_path = Join-Path $workspace 'other-profile/Cargo.toml'
Reject { Assert-WindowsTestFeatures -Messages $foreign -Workspace $workspace }
$changed = Metadata @()
$changed[1].features = @('default', 'google-drive-connector')
Reject { Assert-WindowsTestFeatures -Messages $changed -Workspace $workspace }
$extra = Metadata @()
$extra[1].features += 'e2e-build'
Reject { Assert-WindowsTestFeatures -Messages $extra -Workspace $workspace }
Write-Output 'Compiler feature provenance: 2 exact modes and 8 refusals passed.'
