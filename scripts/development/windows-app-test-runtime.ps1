# Resolve only the actual locked Cargo test and its linked ORT artifact. Never
# search installed profiles, change PATH/system DLLs, or remove build caches.
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$CargoMessages,
  [switch]$AdmissionDiagnostics
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (!$IsWindows -or !$env:RUNNER_TEMP -or !$env:GITHUB_WORKSPACE) {
  throw 'This diagnostic runs only in the disposable Windows candidate runner.'
}
$messagesFile = Get-Item -LiteralPath $CargoMessages
if ($messagesFile.Length -gt 16MB) { throw 'Cargo artifact metadata exceeded its bound.' }
$messages = @(Get-Content -LiteralPath $messagesFile.FullName | ForEach-Object {
  if ($_.Length -gt 256KB) { throw 'Cargo metadata line exceeded its bound.' }
  if ($_.Trim()) { $_ | ConvertFrom-Json -Depth 32 }
})
. (Join-Path $PSScriptRoot 'windows-test-features.ps1')
$featureReceipt = Assert-WindowsTestFeatures -Messages $messages -Workspace $env:GITHUB_WORKSPACE -AdmissionDiagnostics:$AdmissionDiagnostics
$tests = @($messages | Where-Object {
  $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'mdreader_app_lib' -and
    $_.profile.test -and $_.executable
})
if ($tests.Count -ne 1) { throw 'Expected exactly one actual mdreader-app library test executable.' }
$testFile = Get-Item -LiteralPath $tests[0].executable
$workspace = [IO.Path]::GetFullPath($env:GITHUB_WORKSPACE).TrimEnd('\') + '\'
if (!$testFile.FullName.StartsWith($workspace, [StringComparison]::OrdinalIgnoreCase) -or
    $testFile.Name -cnotmatch '^mdreader_app_lib-[a-f0-9]+\.exe$' -or
    ($testFile.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
  throw 'Test artifact is not a normal candidate-workspace executable.'
}
$report = Join-Path $env:RUNNER_TEMP ('mdreader-app-test-runtime-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $report | Out-Null
if ($env:GITHUB_ENV) { Add-Content -LiteralPath $env:GITHUB_ENV -Value ('MDREADER_WINDOWS_TEST_RUNTIME_REPORT=' + $report) }

function Invoke-Listing([string]$Phase) {
  $stdout = Join-Path $report ($Phase + '-stdout.txt')
  $stderr = Join-Path $report ($Phase + '-stderr.txt')
  # --list runs the real Rust harness without executing any test or app setup.
  try {
    $process = Start-Process -FilePath $testFile.FullName -ArgumentList @('--list') -PassThru -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr
  } catch {
    # Keep diagnosing a failed CreateProcess as well as a loader exit. Do not
    # turn either one into a successful zero-test harness listing.
    @{ phase = $Phase; started = $false; exceptionType = $_.Exception.GetType().FullName; hresult = $_.Exception.HResult } |
      ConvertTo-Json | Set-Content -LiteralPath (Join-Path $report ($Phase + '-probe.json'))
    return $null
  }
  if (!$process.WaitForExit(15000)) {
    # Only this exact newly spawned, synthetic CI test process is bounded here.
    $process.Kill()
    $process.WaitForExit()
    throw 'Actual test-harness listing timed out.'
  }
  $process.WaitForExit()
  if ((Get-Item $stdout).Length -gt 1MB -or (Get-Item $stderr).Length -gt 1MB) { throw 'Test listing exceeded its output bound.' }
  $code = [int]$process.ExitCode
  @{ phase = $Phase; started = $true; exitCode = $code; exitHex = $code.ToString('x8') } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $report ($Phase + '-probe.json'))
  Write-Host ('Actual test harness ' + $Phase + ' exit code: 0x' + $code.ToString('x8'))
  return $code
}

# Preserve before/after startup evidence; a DLL hypothesis is not acceptance.
$before = Invoke-Listing 'before'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$installation = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if ($LASTEXITCODE -ne 0 -or $installation.Count -ne 1) { throw 'Exact Visual Studio tools installation unavailable.' }
$version = (Get-Content -LiteralPath (Join-Path $installation[0] 'VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt') -Raw).Trim()
if ($version -cnotmatch '^\d+\.\d+\.\d+$') { throw 'Visual Studio tool version is malformed.' }
$dumpbin = Join-Path $installation[0] ('VC\Tools\MSVC\' + $version + '\bin\Hostx64\x64\dumpbin.exe')
function Write-BinaryReport([string]$Binary, [string]$Option, [string]$Name) {
  $lines = @(& $dumpbin /nologo $Option $Binary 2>&1)
  if ($LASTEXITCODE -ne 0 -or $lines.Count -gt 16384 -or ($lines | Out-String).Length -gt 1MB) {
    throw 'Binary dependency/export report failed or exceeded its bound.'
  }
  $lines | Set-Content -LiteralPath (Join-Path $report $Name)
}
Write-BinaryReport $testFile.FullName /imports 'test-imports.txt'
Write-BinaryReport $testFile.FullName /dependents 'test-dependents.txt'
$ort = @($messages | Where-Object { $_.reason -eq 'build-script-executed' -and $_.package_id -match '(^|[#/ ])ort-sys(@| )' })
if ($ort.Count -ne 1) { throw 'Expected one actual locked ORT build-script artifact.' }
$directories = @($ort[0].linked_paths | Where-Object { $_ -like 'native=*' } | ForEach-Object {
  [IO.Path]::GetFullPath($_.Substring(7))
} | Sort-Object -Unique)
if ($directories.Count -lt 1 -or $directories.Count -gt 8) { throw 'ORT native link directories are missing or ambiguous.' }
$dlls = @($directories | ForEach-Object { Get-ChildItem -LiteralPath $_ -Filter '*.dll' -File } | Sort-Object FullName -Unique)
if ($dlls.Count -gt 16) { throw 'ORT runtime DLL artifact count exceeded its bound.' }
$seen = @{}
$staged = @()
foreach ($dll in $dlls) {
  if ($dll.Length -gt 512MB -or $dll.Name -cnotmatch '^[A-Za-z0-9_.-]+\.dll$' -or $seen.ContainsKey($dll.Name.ToLowerInvariant())) {
    throw 'ORT linked runtime artifacts are oversized or ambiguous.'
  }
  $seen[$dll.Name.ToLowerInvariant()] = $true
  $sourceHash = (Get-FileHash -LiteralPath $dll.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  $destination = Join-Path $testFile.DirectoryName $dll.Name
  $previousHash = $null
  if (Test-Path -LiteralPath $destination) {
    $existing = Get-Item -LiteralPath $destination
    $previousHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($previousHash -ne $sourceHash -and ($existing.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
      throw 'A replaced test-adjacent runtime link requires explicit cache reconciliation.'
    }
  }
  if ($previousHash -ne $sourceHash) { [IO.File]::Copy($dll.FullName, $destination, $true) }
  if ((Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sourceHash) {
    throw 'Copied runtime differs from the actual linked artifact.'
  }
  Write-BinaryReport $destination /imports ($dll.Name + '-imports.txt')
  Write-BinaryReport $destination /exports ($dll.Name + '-exports.txt')
  $systemCandidate = Join-Path $env:SystemRoot ('System32\' + $dll.Name)
  $systemHash = if (Test-Path -LiteralPath $systemCandidate) { (Get-FileHash -LiteralPath $systemCandidate -Algorithm SHA256).Hash.ToLowerInvariant() } else { $null }
  if ($systemHash) {
    # Public candidate exports can identify missing imported symbols without
    # claiming that the historical failing process actually loaded this DLL.
    Write-BinaryReport $systemCandidate /exports ($dll.Name + '-system-candidate-exports.txt')
  }
  $staged += @{ name = $dll.Name; sha256 = $sourceHash; previousAdjacentSha256 = $previousHash; systemCandidateSha256 = $systemHash }
  Write-Host ('Linked runtime adjacent to actual test: ' + $dll.Name + ' SHA256=' + $sourceHash)
}
$after = Invoke-Listing 'after'
. (Join-Path $PSScriptRoot 'windows-imports.ps1')
$imports = [Collections.Generic.List[object]]::new()
foreach ($image in @('test') + @($staged | ForEach-Object { $_.name })) {
  foreach ($entry in Read-WindowsImports (Join-Path $report ($image + '-imports.txt'))) {
    $entry.image = $image
    $imports.Add($entry)
  }
}
if ($imports.Count -gt 16384) { throw 'Combined actual import count exceeded its bound.' }
$probeInput = Join-Path $report 'symbol-probe-input.json'
@{ testDirectory = $testFile.DirectoryName; imports = $imports.ToArray() } |
  ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $probeInput
$probeOutput = Join-Path $report 'symbol-probe.json'
$probeScript = Join-Path $PSScriptRoot 'windows-loader-symbol-probe.ps1'
$probeStdout = Join-Path $report 'symbol-probe-stdout.txt'
$probeStderr = Join-Path $report 'symbol-probe-stderr.txt'
$probeProcess = Start-Process -FilePath (Join-Path $PSHOME 'pwsh.exe') -PassThru -NoNewWindow -ArgumentList @(
  '-NoLogo', '-NoProfile', '-NonInteractive', '-File', ('"' + $probeScript + '"'),
  '-InputFile', ('"' + $probeInput + '"'), '-OutputFile', ('"' + $probeOutput + '"')
) -RedirectStandardOutput $probeStdout -RedirectStandardError $probeStderr
$probeDeadline = [DateTime]::UtcNow.AddSeconds(45)
$probeBoundFailure = $false
while (!$probeProcess.WaitForExit(100)) {
  if ([DateTime]::UtcNow -gt $probeDeadline -or (Get-Item $probeStdout).Length -gt 1MB -or (Get-Item $probeStderr).Length -gt 1MB) {
    # This handle names only the fresh owned diagnostic child, never an app/service.
    $probeProcess.Kill()
    $probeProcess.WaitForExit()
    $probeBoundFailure = $true
    break
  }
}
$probeProcess.WaitForExit()
$probeBoundFailure = $probeBoundFailure -or (Get-Item $probeStdout).Length -gt 1MB -or (Get-Item $probeStderr).Length -gt 1MB
$probeSucceeded = !$probeBoundFailure -and $probeProcess.ExitCode -eq 0 -and (Test-Path -LiteralPath $probeOutput)
if ($probeSucceeded) {
  if ((Get-Item $probeOutput).Length -gt 4MB) { throw 'Symbol probe output exceeded its bound.' }
  $symbolReceipt = Get-Content -LiteralPath $probeOutput -Raw | ConvertFrom-Json -Depth 16
  $exportedPaths = @{}
  foreach ($module in $symbolReceipt.modules) {
    if (!$module.loadedProbeModulePath) {
      Write-Host ('Imported module did not load in diagnostic child: ' + $module.declaredModule + '; Win32=' + $module.loadWin32Error)
    }
    # Actual probe-loaded exports bind ordinal observations to real module bytes.
    # GetProcAddress nonzero alone does not prove sparse ordinal exports are valid.
    $loaded = [string]$module.loadedProbeModulePath
    if ($loaded -and !$exportedPaths.ContainsKey($loaded)) {
      if ($exportedPaths.Count -ge 128) { throw 'Loaded export report count exceeded its bound.' }
      $filename = ('loaded-module-' + $exportedPaths.Count + '-exports.txt')
      Write-BinaryReport $loaded /exports $filename
      $exportedPaths[$loaded] = $filename
    }
    if ($loaded) { $module | Add-Member -NotePropertyName loadedExportsReport -NotePropertyValue $exportedPaths[$loaded] }
    foreach ($entry in $module.entries) {
      if (!$entry.addressObserved) {
        $symbol = if ($null -ne $entry.ordinal) { 'ordinal ' + $entry.ordinal } else { $entry.name }
        Write-Host ('Declared import unavailable in diagnostic child: ' + $entry.image + ' -> ' + $module.declaredModule + '!' + $symbol + '; Win32=' + $entry.win32Error + '; probe-loaded=' + $loaded)
      }
    }
  }
  $symbolReceipt | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $probeOutput
}
if ((Get-ChildItem -LiteralPath $report -File | Measure-Object Length -Sum).Sum -gt 32MB) { throw 'Combined runtime diagnostics exceeded their bound.' }
@{ kind = 'actual-windows-test-runtime'; executableSha256 = (Get-FileHash -LiteralPath $testFile.FullName -Algorithm SHA256).Hash.ToLowerInvariant();
   beforeListingExit = $before; afterListingExit = $after; linkedRuntimeDlls = $staged; compilerFeatures = $featureReceipt;
   symbolProbeSucceeded = $probeSucceeded; symbolProbeExitCode = $probeProcess.ExitCode; symbolProbeBoundFailure = $probeBoundFailure;
   acceptsMigrationTests = $false; identifiesPreviouslyLoadedDll = $false } |
  ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $report 'receipt.json')
if (!$probeSucceeded) { throw 'Bounded imported-symbol diagnostic failed; inspect its exact child exit/stdout/stderr.' }
if ($null -eq $after -or $after -ne 0) { throw 'Actual desktop test harness cannot start; inspect bounded imports/exports and before/after receipt.' }
