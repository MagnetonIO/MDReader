# Fresh disposable-runner process. Loads only exact staged/system DLL candidates,
# resolves imported entries without calling them, and never changes PATH/system files.
[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$InputFile,
      [Parameter(Mandatory = $true)][string]$OutputFile)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (!$IsWindows -or !$env:RUNNER_TEMP -or !$env:GITHUB_WORKSPACE) {
  throw 'Loader probing requires the disposable Windows candidate runner.'
}
$temporary = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\') + '\'
$inputItem = Get-Item -LiteralPath $InputFile
$outputPath = [IO.Path]::GetFullPath($OutputFile)
if (!$inputItem.FullName.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase) -or
    !$outputPath.StartsWith($temporary, [StringComparison]::OrdinalIgnoreCase) -or
    $inputItem.DirectoryName -cne [IO.Path]::GetDirectoryName($outputPath) -or
    ($inputItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
    $inputItem.Length -gt 4MB -or (Test-Path -LiteralPath $outputPath)) {
  throw 'Loader probe input/output must be new bounded runner diagnostic artifacts.'
}
$manifest = Get-Content -LiteralPath $inputItem.FullName -Raw | ConvertFrom-Json -Depth 16
$workspace = [IO.Path]::GetFullPath($env:GITHUB_WORKSPACE).TrimEnd('\') + '\'
$directory = [IO.Path]::GetFullPath($manifest.testDirectory).TrimEnd('\') + '\'
if (!$directory.StartsWith($workspace, [StringComparison]::OrdinalIgnoreCase) -or $manifest.imports.Count -gt 16384) {
  throw 'Loader probe is not bound to the candidate artifact directory.'
}
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class ReaderLoaderProbe {
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true, ExactSpelling=true)]
  public static extern IntPtr LoadLibraryExW(string name, IntPtr file, uint flags);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true, ExactSpelling=true)]
  public static extern IntPtr GetModuleHandleW(string name);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true, ExactSpelling=true)]
  public static extern uint GetModuleFileNameW(IntPtr handle, StringBuilder path, int size);
  [DllImport("kernel32.dll", CharSet=CharSet.Ansi, SetLastError=true, ExactSpelling=true)]
  public static extern IntPtr GetProcAddress(IntPtr handle, string name);
  [DllImport("kernel32.dll", EntryPoint="GetProcAddress", SetLastError=true, ExactSpelling=true)]
  public static extern IntPtr GetProcOrdinal(IntPtr handle, IntPtr ordinal);
  [DllImport("kernel32.dll", SetLastError=true, ExactSpelling=true)]
  public static extern bool FreeLibrary(IntPtr handle);
  [DllImport("kernel32.dll", ExactSpelling=true)]
  public static extern uint SetErrorMode(uint mode);
}
'@
function Module-Path([IntPtr]$Handle) {
  if ($Handle -eq [IntPtr]::Zero) { return $null }
  $path = [Text.StringBuilder]::new(32768)
  $length = [ReaderLoaderProbe]::GetModuleFileNameW($Handle, $path, $path.Capacity)
  if ($length -eq 0 -or $length -ge $path.Capacity) { throw 'Loaded module path unavailable or truncated.' }
  return $path.ToString()
}
$previousErrorMode = [ReaderLoaderProbe]::SetErrorMode(0x8001)
$observed = [Collections.Generic.List[object]]::new()
try {
  $groups = @($manifest.imports | Group-Object module)
  if ($groups.Count -lt 1 -or $groups.Count -gt 128) { throw 'Imported DLL count exceeded its bound.' }
  foreach ($group in $groups) {
    $name = [string]$group.Name
    if ($name -cnotmatch '^[A-Za-z0-9_.-]+\.[dD][lL][lL]$' -or $group.Count -gt 4096) { throw 'Invalid imported DLL identity or symbol count.' }
    $adjacent = Join-Path $directory $name
    $preexistingPath = Module-Path ([ReaderLoaderProbe]::GetModuleHandleW($name))
    # Prefer the exact candidate-adjacent artifact, otherwise resolve the
    # declared system/API-set name through System32 only, never PATH/CWD.
    $request = $name
    $flags = [uint32]0x800
    $candidateHash = $null
    if (Test-Path -LiteralPath $adjacent) {
      $candidate = Get-Item -LiteralPath $adjacent
      if (($candidate.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $candidate.Length -gt 512MB) { throw 'Probe candidate DLL is not a bounded normal artifact.' }
      $request = $candidate.FullName
      $flags = [uint32]0x900
      $candidateHash = (Get-FileHash -LiteralPath $request -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $handle = [ReaderLoaderProbe]::LoadLibraryExW($request, [IntPtr]::Zero, $flags)
    $loadError = if ($handle -eq [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::GetLastWin32Error() } else { 0 }
    $path = Module-Path $handle
    $loadedHash = if ($path) { (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() } else { $null }
    $entries = [Collections.Generic.List[object]]::new()
    try {
      foreach ($entry in $group.Group) {
        if ($null -eq $entry.ordinal) {
          if (!$entry.name -or $entry.name.Length -gt 1024 -or $entry.name -cnotmatch '^[A-Za-z_?@$][A-Za-z0-9_?@$:.]*$') { throw 'Invalid exact imported name.' }
        } elseif ([int]$entry.ordinal -lt 1 -or [int]$entry.ordinal -gt 65535 -or $null -ne $entry.name) { throw 'Invalid exact imported ordinal.' }
        $address = [IntPtr]::Zero
        $error = $loadError
        if ($handle -ne [IntPtr]::Zero) {
          $address = if ($null -ne $entry.ordinal) { [ReaderLoaderProbe]::GetProcOrdinal($handle, [IntPtr][int]$entry.ordinal) } else { [ReaderLoaderProbe]::GetProcAddress($handle, [string]$entry.name) }
          $error = if ($address -eq [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::GetLastWin32Error() } else { 0 }
        }
        $entries.Add(@{ image = $entry.image; name = $entry.name; ordinal = $entry.ordinal;
          addressObserved = ($address -ne [IntPtr]::Zero); win32Error = $error })
      }
      $observed.Add(@{ declaredModule = $name; requestedModule = $request; loadFlags = $flags;
        preexistingProbeModulePath = $preexistingPath; loadedProbeModulePath = $path;
        requestedAdjacentSha256 = $candidateHash; loadedProbeModuleSha256 = $loadedHash;
        loadWin32Error = $loadError; entries = $entries.ToArray() })
    } finally {
      if ($handle -ne [IntPtr]::Zero -and ![ReaderLoaderProbe]::FreeLibrary($handle)) { throw 'Probe DLL reference could not be released.' }
    }
  }
  $receipt = @{ kind = 'isolated-windows-import-symbol-probe'; version = 1; modules = $observed.ToArray();
    osVersion = [Environment]::OSVersion.VersionString; powershellVersion = $PSVersionTable.PSVersion.ToString();
    pointerBytes = [IntPtr]::Size;
    resolvesPreviouslyFailingHarness = $false; acceptsMigrationTests = $false;
    ordinalAddressAloneProvesExport = $false; invokesImportedFunctions = $false }
  $json = $receipt | ConvertTo-Json -Depth 16
  if ($json.Length -gt 4MB) { throw 'Loader symbol receipt exceeded its bound.' }
  [IO.File]::WriteAllText($outputPath, $json)
} finally { [void][ReaderLoaderProbe]::SetErrorMode($previousErrorMode) }
