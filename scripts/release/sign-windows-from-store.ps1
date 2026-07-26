param(
  [Parameter(Mandatory = $true, Position = 0)]
  [string]$Path
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$storeName = $env:WINDOWS_CERT_STORE
$storeOwned = $env:WINDOWS_CERT_STORE_OWNED
$thumbprint = $env:WINDOWS_CERT_THUMBPRINT
if (
  $env:GITHUB_RUN_ID -notmatch '^[0-9]+$' -or
  $env:GITHUB_RUN_ATTEMPT -notmatch '^[0-9]+$'
) {
  throw 'Windows release run identity is invalid'
}
$expectedStorePattern =
  "^MDReaderRelease-$($env:GITHUB_RUN_ID)-$($env:GITHUB_RUN_ATTEMPT)-[0-9A-F]{32}$"
if ($storeOwned -ne 'true' -or $storeName -notmatch $expectedStorePattern) {
  throw 'Windows signing store is not the exact per-run owned store'
}
if ($thumbprint -notmatch '^[0-9A-F]{40}$') {
  throw 'Windows signing certificate thumbprint is invalid'
}
if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
  throw "Windows signing target does not exist: $Path"
}

$certificatePath = "Cert:\CurrentUser\$storeName\$thumbprint"
$certificate = Get-Item -LiteralPath $certificatePath -ErrorAction Stop
if ($certificate.Thumbprint -ne $thumbprint -or -not $certificate.HasPrivateKey) {
  throw 'Windows per-run signing certificate identity or private key is invalid'
}

$signTool = Get-Command signtool.exe -ErrorAction Stop
$signToolPath = $signTool.Source
if (-not [IO.Path]::IsPathFullyQualified($signToolPath)) {
  throw 'signtool did not resolve to an absolute executable path'
}
& $signToolPath sign `
  /sha1 $thumbprint `
  /s $storeName `
  /fd SHA256 `
  /tr 'http://timestamp.digicert.com' `
  /td SHA256 `
  $Path
if ($LASTEXITCODE -ne 0) {
  throw "signtool failed for $Path with exit code $LASTEXITCODE"
}
