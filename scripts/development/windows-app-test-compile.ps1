# CI-only repair of cached link metadata whose verified download was not cached.
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$CargoMessages)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-ort-artifacts.ps1')
if (!$IsWindows -or $env:GITHUB_ACTIONS -cne 'true' -or !$env:RUNNER_TEMP -or !$env:GITHUB_WORKSPACE -or
    !$env:MDREADER_WINDOWS_ORT_ARTIFACT_ROOT) {
  throw 'ORT reconciliation runs only in the disposable Windows candidate runner.'
}
foreach ($name in @('ORT_CACHE_DIR', 'ORT_LIB_PATH', 'ORT_LIB_LOCATION', 'ORT_SKIP_DOWNLOAD', 'ORT_OFFLINE', 'CARGO_NET_OFFLINE', 'CARGO_TARGET_DIR')) {
  if ([Environment]::GetEnvironmentVariable($name)) { throw 'Unexpected native cache/link override in the candidate runner.' }
}
$workspace = [IO.Path]::GetFullPath($env:GITHUB_WORKSPACE)
if ([IO.Path]::GetFullPath((Get-Location).Path) -cne $workspace) { throw 'Cargo reconciliation requires the exact candidate workspace.' }
$target = Join-Path $workspace 'target'
if ((Test-Path -LiteralPath $target) -and ((Get-Item -LiteralPath $target).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
  throw 'Cargo target is not a normal disposable candidate directory.'
}
$temporary = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\') + '\'
$messages = [IO.Path]::GetFullPath($CargoMessages)
if (!$messages.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $messages)) {
  throw 'Cargo messages require a fresh runner-temporary file.'
}
$localData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
$root = Join-Path $localData 'ort.pyke.io/dfbin/x86_64-pc-windows-msvc'
if ([IO.Path]::GetFullPath($env:MDREADER_WINDOWS_ORT_ARTIFACT_ROOT) -cne [IO.Path]::GetFullPath($root)) {
  throw 'ORT artifact cache does not match the native Windows KnownFolder.'
}
foreach ($path in @((Join-Path $localData 'ort.pyke.io'), (Join-Path $localData 'ort.pyke.io/dfbin'), $root)) {
  if (Test-Path -LiteralPath $path) {
    $directory = Get-Item -LiteralPath $path
    if (!$directory.PSIsContainer -or ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
      throw 'ORT cache ancestry is not a normal disposable runner directory.'
    }
  }
}
# Preserve Cargo's features and dependency graph. Missing directories in an
# otherwise warm cache require one package-scoped rebuild, never a cold target.
cargo test --locked -p mdreader-app --lib --no-run --message-format=json > $messages
$compileCode = $LASTEXITCODE
$directories = @(Read-WindowsOrtDirectories $messages $root)
$missing = @(Get-MissingWindowsOrtDirectories $directories)
if ($missing.Count -gt 0) {
  Write-Host ('Reconcile missing locked ORT download directories: ' + $missing.Count + '; rebuild only ort-sys.')
  cargo clean --locked --package ort-sys --target-dir $target
  if ($LASTEXITCODE -ne 0) { throw 'Package-scoped ORT cache reconciliation failed.' }
  cargo test --locked -p mdreader-app --lib --no-run --message-format=json > $messages
  $compileCode = $LASTEXITCODE
  $directories = @(Read-WindowsOrtDirectories $messages $root)
  if (@(Get-MissingWindowsOrtDirectories $directories).Count -gt 0) { throw 'Rebuilt ORT still names an absent runtime artifact.' }
}
if ($compileCode -ne 0) { throw 'Actual Windows app test compilation failed.' }
# Cache the real artifacts even when the subsequent loader diagnostic fails.
# Its hash checks and the full minimum desktop-test gate remain independent.
Add-Content -LiteralPath $env:GITHUB_ENV -Value 'MDREADER_WINDOWS_ORT_CACHE_READY=true'
