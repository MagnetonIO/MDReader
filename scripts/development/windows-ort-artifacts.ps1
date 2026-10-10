# Bind cache reconciliation to Cargo's actual locked ort-sys link metadata.
# Portable functions are tested without running Cargo or loading native DLLs.
Set-StrictMode -Version Latest
function Read-WindowsOrtDirectories([string]$CargoMessages, [string]$ArtifactRoot) {
  $metadata = Get-Item -LiteralPath $CargoMessages
  if ($metadata.Length -gt 16MB) { throw 'Cargo artifact metadata exceeded its bound.' }
  $records = @(Get-Content -LiteralPath $metadata.FullName | ForEach-Object {
    if ($_.Length -gt 256KB) { throw 'Cargo metadata line exceeded its bound.' }
    if ($_.Trim()) { $_ | ConvertFrom-Json -Depth 32 }
  })
  $ort = @($records | Where-Object {
    $_.reason -eq 'build-script-executed' -and $_.package_id -match '(^|[#/ ])ort-sys(@| )'
  })
  if ($ort.Count -ne 1) { throw 'Expected one actual locked ort-sys build-script record.' }
  $root = [IO.Path]::GetFullPath($ArtifactRoot).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
  $paths = @($ort[0].linked_paths | Where-Object { $_ -like 'native=*' } | ForEach-Object {
    $path = [IO.Path]::GetFullPath($_.Substring(7))
    if (!$path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) -or
        $path.Substring($root.Length) -cnotmatch '^[0-9a-f]{64}$') {
      throw 'ORT link metadata is outside the exact runner download cache.'
    }
    $path
  } | Sort-Object -Unique)
  if ($paths.Count -lt 1 -or $paths.Count -gt 8) { throw 'ORT link directories are missing or ambiguous.' }
  return $paths
}

function Get-MissingWindowsOrtDirectories([string[]]$Directories) {
  foreach ($path in $Directories) {
    if (!(Test-Path -LiteralPath $path)) { $path; continue }
    $item = Get-Item -LiteralPath $path
    if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
      throw 'ORT artifact directory is not a normal runner directory.'
    }
    # A partial/empty extraction must not be silently blessed or removed. The
    # crate downloader only retries an absent hash directory, not partial ones.
    $files = @(Get-ChildItem -LiteralPath $path -Force | Select-Object -First 513)
    if ($files.Count -eq 0 -or $files.Count -gt 512 -or
        @($files | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count -gt 0) {
      throw 'ORT artifact directory is empty, linked or exceeds its bound.'
    }
  }
}
