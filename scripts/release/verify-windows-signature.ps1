param(
  [Parameter(Mandatory = $true)]
  [string]$BundleRoot,
  [Parameter(Mandatory = $true)]
  [string]$ExpectedThumbprint
)

$ErrorActionPreference = 'Stop'
if ($ExpectedThumbprint -cnotmatch '^[0-9A-F]{40}$') {
  throw 'reviewed Windows certificate thumbprint is invalid'
}
$installers = @(Get-ChildItem $BundleRoot -Recurse -Filter '*.exe')
if ($installers.Count -ne 1) {
  throw "expected exactly one NSIS installer, found $($installers.Count)"
}
$signature = Get-AuthenticodeSignature $installers[0].FullName
if ($signature.Status -ne 'Valid') {
  throw "invalid Authenticode status: $($signature.Status)"
}
if (
  $null -eq $signature.SignerCertificate -or
  $signature.SignerCertificate.Thumbprint.ToUpperInvariant() -cne $ExpectedThumbprint
) {
  throw 'NSIS installer signer does not match the reviewed publisher identity'
}
