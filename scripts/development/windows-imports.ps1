# Pure bounded parsing of the actual dumpbin /imports report, not guessed APIs.
Set-StrictMode -Version Latest
function Read-WindowsImports([string]$Path) {
  $file = Get-Item -LiteralPath $Path
  if ($file.Length -gt 1MB) { throw 'Import report exceeded its bound.' }
  $module = $null
  $rows = [Collections.Generic.List[object]]::new()
  foreach ($line in Get-Content -LiteralPath $file.FullName) {
    if ($line.Length -gt 4096) { throw 'Import report line exceeded its bound.' }
    if ($line -match '^\s{4}([A-Za-z0-9_.-]+\.dll)\s*$') {
      $module = $Matches[1]
      continue
    }
    if ($line -match '^\s*Summary\s*$') { $module = $null; continue }
    if (!$module) { continue }
    if ($line -match '^\s+Ordinal\s+(\d+)\s*$') {
      $ordinal = [int]$Matches[1]
      if ($ordinal -lt 1 -or $ordinal -gt 65535) { throw 'Imported ordinal is outside the PE bound.' }
      $rows.Add(@{ module = $module; ordinal = $ordinal; name = $null })
    } elseif ($line -match '^\s+[0-9A-Fa-f]+\s+([A-Za-z_?@$][A-Za-z0-9_?@$:.]*)\s*$') {
      $name = $Matches[1]
      if ($name.Length -gt 1024) { throw 'Imported symbol exceeded its bound.' }
      $rows.Add(@{ module = $module; ordinal = $null; name = $name })
    } elseif ($line -match '^\s+(Ordinal\s+\S+|[0-9A-Fa-f]+\s+\S+)\s*$') {
      throw 'An exact imported entry could not be represented safely.'
    }
    if ($rows.Count -gt 8192) { throw 'Imported symbol count exceeded its bound.' }
  }
  if ($rows.Count -lt 1) { throw 'No actual imported symbols were parsed.' }
  return $rows.ToArray()
}
