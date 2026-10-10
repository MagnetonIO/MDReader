# Bind a bounded compiler record to the only two supported fixture feature modes.
# Dependency features live on the dependency artifact, not the app's own list.
function Assert-WindowsTestFeatures([object[]]$Messages, [string]$Workspace, [switch]$AdmissionDiagnostics) {
  if ($Messages.Count -lt 1 -or $Messages.Count -gt 16384) { throw 'Compiler artifact count is missing or unbounded.' }
  $tests = @($Messages | Where-Object {
    $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'mdreader_app_lib' -and
      $_.profile.test -and $_.executable
  })
  $profiles = @($Messages | Where-Object {
    $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'mdreader_profile' -and
      $_.target.kind -ccontains 'lib'
  })
  if ($tests.Count -ne 1 -or $profiles.Count -ne 1) { throw 'Actual app/profile compiler artifacts are missing or ambiguous.' }
  foreach ($selected in @(
    @{ artifact = $tests[0]; manifest = (Join-Path $Workspace 'src-tauri/Cargo.toml') },
    @{ artifact = $profiles[0]; manifest = (Join-Path $Workspace 'crates/mdreader-profile/Cargo.toml') }
  )) {
    if (![string]::Equals([IO.Path]::GetFullPath($selected.artifact.manifest_path),
        [IO.Path]::GetFullPath($selected.manifest), [StringComparison]::OrdinalIgnoreCase)) {
      throw 'Compiler feature provenance names a foreign manifest.'
    }
  }
  $appFeatures = @($tests[0].features)
  $profileFeatures = @($profiles[0].features)
  $defaultApp = @('default', 'dropbox-connector', 'google-drive-connector')
  if ($appFeatures.Count -ne $defaultApp.Count -or @($defaultApp | Where-Object { $appFeatures -cnotcontains $_ }).Count) {
    throw 'The actual app test does not retain its exact shipping default features.'
  }
  $expectedProfile = @(if ($AdmissionDiagnostics) { 'admission-test-diagnostics' })
  if ($profileFeatures.Count -ne $expectedProfile.Count -or @($expectedProfile | Where-Object { $profileFeatures -cnotcontains $_ }).Count) {
    throw 'The actual profile artifact does not match the requested admission diagnostic mode.'
  }
  return @{ admissionDiagnostics = [bool]$AdmissionDiagnostics; appFeatures = $defaultApp; profileFeatures = @($expectedProfile) }
}
