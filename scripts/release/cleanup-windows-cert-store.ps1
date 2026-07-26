$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$storeName = $env:WINDOWS_CERT_STORE
$storeOwned = $env:WINDOWS_CERT_STORE_OWNED
$certificatePath = $env:WINDOWS_CERTIFICATE_PATH

if ([string]::IsNullOrWhiteSpace($certificatePath)) {
  throw 'Windows signing certificate path is missing'
}
if (
  $env:GITHUB_RUN_ID -notmatch '^[0-9]+$' -or
  $env:GITHUB_RUN_ATTEMPT -notmatch '^[0-9]+$'
) {
  throw 'Windows release run identity is invalid'
}

$expectedStorePattern =
  "^MDReaderRelease-$($env:GITHUB_RUN_ID)-$($env:GITHUB_RUN_ATTEMPT)-[0-9A-F]{32}$"

try {
  if ([string]::IsNullOrWhiteSpace($storeName)) {
    if (-not [string]::IsNullOrWhiteSpace($storeOwned)) {
      throw 'Windows certificate-store ownership is present without a store capability'
    }
  } else {
    if ($storeName -notmatch $expectedStorePattern) {
      throw 'Windows certificate-store capability is invalid'
    }
    $storePath = "Cert:\CurrentUser\$storeName"
    if ([string]::IsNullOrWhiteSpace($storeOwned)) {
      if (Test-Path -LiteralPath $storePath) {
        throw 'refusing to delete an unproven Windows certificate store'
      }
    } elseif ($storeOwned -eq 'true') {
      if (-not (Test-Path -LiteralPath $storePath)) {
        throw 'owned Windows certificate store is missing'
      }
      $certificates = @(Get-ChildItem -LiteralPath $storePath -ErrorAction Stop)
      foreach ($certificate in $certificates) {
        if ($certificate.Thumbprint -notmatch '^[0-9A-F]{40}$') {
          throw 'Windows owned certificate identity is invalid'
        }
        $ownedCertificatePath = "$storePath\$($certificate.Thumbprint)"
        $ownedCertificate = Get-Item -LiteralPath $ownedCertificatePath -ErrorAction Stop
        if ($ownedCertificate.Thumbprint -ne $certificate.Thumbprint) {
          throw 'Windows owned certificate identity mismatch'
        }
        Remove-Item `
          -LiteralPath $ownedCertificatePath `
          -DeleteKey `
          -Force `
          -ErrorAction Stop
      }
      if (@(Get-ChildItem -LiteralPath $storePath -ErrorAction Stop).Count -ne 0) {
        throw 'Windows certificate residue remains in the owned store'
      }
      Remove-Item -LiteralPath $storePath -Force -ErrorAction Stop
      if (Test-Path -LiteralPath $storePath) {
        throw 'Windows owned certificate-store residue remains after cleanup'
      }
    } else {
      throw 'Windows certificate-store ownership proof is invalid'
    }
  }
} finally {
  if (Test-Path -LiteralPath $certificatePath) {
    Remove-Item -LiteralPath $certificatePath -Force -ErrorAction Stop
  }
  if (Test-Path -LiteralPath $certificatePath) {
    throw 'Windows signing file residue remains after cleanup'
  }
}
