# Portable parser regression using the public dumpbin layout, including actual
# Windows app import names. This never loads a DLL or launches the app.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-imports.ps1')
# PowerShell automatic variables are case-insensitive. Inspect assignments in
# the actual loader probe so a reserved variable cannot abort symbol observation.
$probeTokens = $null
$probeParseErrors = $null
$probeAst = [System.Management.Automation.Language.Parser]::ParseFile(
  (Join-Path $PSScriptRoot 'windows-loader-symbol-probe.ps1'),
  [ref]$probeTokens, [ref]$probeParseErrors)
if ($probeParseErrors.Count -ne 0) { throw 'Loader probe syntax is invalid.' }
$reservedAssignments = @($probeAst.FindAll({ param($node)
  $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
    $node.Left -is [System.Management.Automation.Language.VariableExpressionAst] -and
    $node.Left.VariablePath.UserPath -imatch '^(Error|HOME|Host|PID|PSVersionTable|IsWindows|IsLinux|IsMacOS)$'
}, $true))
if ($reservedAssignments.Count -ne 0) { throw 'Loader probe assigns an automatic variable.' }
$directory = Join-Path ([IO.Path]::GetTempPath()) ('mdreader-import-parser-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $directory | Out-Null
$path = Join-Path $directory 'imports.txt'
function Assert([bool]$Condition, [string]$Message) {
  if (!$Condition) { throw $Message }
}
function Reject([string]$Text) {
  [IO.File]::WriteAllText($path, $Text)
  $rejected = $false
  try { $null = Read-WindowsImports $path } catch { $rejected = $true }
  Assert $rejected 'Malformed/unbounded import report was accepted.'
}
try {
  [IO.File]::WriteAllText($path, @'
  Section contains the following imports:

    kernel32.dll
             1433B30A8 Import Address Table
             14443F258 Import Name Table
                     0 time date stamp
                     0 Index of first forwarder reference
                           0 GetProcAddress
                           0 GetFinalPathNameByHandleW

    DirectML.dll
                       Ordinal     2

    MSVCP140.dll
                          37 ?_Xbad_alloc@std@@YAXXZ

    api-ms-win-crt-runtime-l1-1-0.dll
                          D8 _initterm

  Summary
                    1000 .data
'@)
  $rows = @(Read-WindowsImports $path)
  Assert ($rows.Count -eq 5) 'Headers or summary were confused with imported entries.'
  Assert ($rows[0].name -ceq 'GetProcAddress' -and $rows[0].module -ceq 'kernel32.dll') 'Named identity changed.'
  Assert ($rows[2].ordinal -eq 2 -and $null -eq $rows[2].name) 'Exact ordinal changed.'
  Assert ($rows[3].name -ceq '?_Xbad_alloc@std@@YAXXZ') 'Decorated name changed.'
  Assert ($rows[4].module -ceq 'api-ms-win-crt-runtime-l1-1-0.dll') 'API-set identity changed.'
  Reject "    test.dll`n  Ordinal 65536"
  Reject "    test.dll`n  Ordinal invalid"
  Reject "    test.dll`n  0 invalid/name"
  Reject ("    test.dll`n  0 " + ('a' * 1025))
  Reject '  Summary'
  Write-Output 'Windows imports parser: valid entries and 5 refusal cases passed.'
} finally { Remove-Item -LiteralPath $directory -Recurse }
