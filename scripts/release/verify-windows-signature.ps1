param(
  [Parameter(Mandatory = $true)]
  [string]$BundleRoot
)

$ErrorActionPreference = 'Stop'
$installers = @(Get-ChildItem $BundleRoot -Recurse -Filter '*.exe')
if ($installers.Count -ne 1) {
  throw "expected exactly one NSIS installer, found $($installers.Count)"
}
$signature = Get-AuthenticodeSignature $installers[0].FullName
if ($signature.Status -ne 'Valid') {
  throw "invalid Authenticode status: $($signature.Status)"
}
