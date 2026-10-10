# Real portable files + Cargo's public build-script JSON. No Cargo mutation.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-ort-artifacts.ps1')
foreach ($name in @('windows-ort-artifacts.ps1', 'windows-app-test-compile.ps1')) {
  $tokens = $null
  $parseErrors = $null
  $ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name), [ref]$tokens, [ref]$parseErrors)
  if ($parseErrors.Count -ne 0) { throw 'ORT helper syntax is invalid.' }
  if (@($ast.FindAll({ param($node)
    $node -is [Management.Automation.Language.AssignmentStatementAst] -and
      $node.Left -is [Management.Automation.Language.VariableExpressionAst] -and
      $node.Left.VariablePath.UserPath -imatch '^(Error|HOME|Host|PID|PSVersionTable|IsWindows|IsLinux|IsMacOS)$'
  }, $true)).Count -ne 0) { throw 'ORT helper assigns an automatic variable.' }
}
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('mdreader-ort-metadata-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $temporary | Out-Null
$root = Join-Path $temporary 'artifact-root'
$messages = Join-Path $temporary 'cargo.jsonl'
$digest = 'b' * 64
$exact = Join-Path $root $digest
function Assert([bool]$Condition, [string]$Message) { if (!$Condition) { throw $Message } }
function Metadata([string[]]$Paths, [int]$Copies = 1) {
  $json = @{ reason = 'build-script-executed'; package_id = 'registry+https://github.com/rust-lang/crates.io-index#ort-sys@2.0.0-rc.12'; linked_paths = $Paths } | ConvertTo-Json -Compress
  [IO.File]::WriteAllText($messages, (($json + "`n") * $Copies))
}
function Reject([scriptblock]$Operation) {
  $failed = $false
  try { & $Operation } catch { $failed = $true }
  Assert $failed 'Malformed or unbounded ORT artifact authority was accepted.'
}
try {
  Metadata @('native=' + $exact)
  $paths = @(Read-WindowsOrtDirectories $messages $root)
  Assert ($paths.Count -eq 1 -and $paths[0] -ceq $exact) 'Actual linked artifact identity changed.'
  Assert (@(Get-MissingWindowsOrtDirectories $paths).Count -eq 1) 'Warm metadata with missing download was not detected.'
  New-Item -ItemType Directory -Path $exact -Force | Out-Null
  Reject { Get-MissingWindowsOrtDirectories $paths }
  [IO.File]::WriteAllText((Join-Path $exact 'actual.lib'), 'synthetic exact native bytes')
  Assert (@(Get-MissingWindowsOrtDirectories $paths).Count -eq 0) 'Present download caused unnecessary invalidation.'
  Metadata @('native=' + $exact) 2
  Reject { Read-WindowsOrtDirectories $messages $root }
  Metadata @('native=' + (Join-Path $temporary $digest))
  Reject { Read-WindowsOrtDirectories $messages $root }
  Metadata @('native=' + (Join-Path ($root + '-other') $digest))
  Reject { Read-WindowsOrtDirectories $messages $root }
  Metadata @('native=' + (Join-Path $root 'tmp.unverified'))
  Reject { Read-WindowsOrtDirectories $messages $root }
  Metadata @('framework=unrelated')
  Reject { Read-WindowsOrtDirectories $messages $root }
  Metadata (1..9 | ForEach-Object { 'native=' + (Join-Path $root ($_.ToString('x64'))) })
  Reject { Read-WindowsOrtDirectories $messages $root }
  [IO.File]::WriteAllText($messages, 'a' * (256KB + 1))
  Reject { Read-WindowsOrtDirectories $messages $root }
  Metadata @('native=' + $exact)
  Remove-Item -LiteralPath $exact -Recurse
  [IO.File]::WriteAllText($exact, 'file instead of directory')
  Reject { Get-MissingWindowsOrtDirectories @(Read-WindowsOrtDirectories $messages $root) }
  Write-Output 'ORT artifact metadata: exact/missing/present cases and 9 refusal cases passed.'
} finally { Remove-Item -LiteralPath $temporary -Recurse }
