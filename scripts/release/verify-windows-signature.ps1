param(
  [Parameter(Mandatory = $true)]
  [string]$BundleRoot,
  [Parameter(Mandatory = $true)]
  [string]$ExpectedThumbprint,
  [Parameter(Mandatory = $true)]
  [string]$ExpectedVersion
)

$ErrorActionPreference = 'Stop'
if ($ExpectedThumbprint -cnotmatch '^[0-9A-F]{40}$') {
  throw 'reviewed Windows certificate thumbprint is invalid'
}
if ($ExpectedVersion -cnotmatch '^\d+\.\d+\.\d+$') {
  throw 'reviewed Windows release version is invalid'
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
$version = $installers[0].VersionInfo
if (
  $version.ProductVersion -cne $ExpectedVersion -or
  $version.FileVersion -cne $ExpectedVersion
) {
  throw 'NSIS installer product/file version does not match the reviewed release'
}
