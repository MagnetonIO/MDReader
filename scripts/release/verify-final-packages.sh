#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
BUNDLE_ROOT="${2:?bundle root is required}"
SOURCE_COMMIT="${3:?source commit is required}"
TARGET="${4:?target is required}"
EVIDENCE_PATH="${5:?evidence output is required}"
EXPECTED_VERSION="${6:?expected version is required}"
CREDENTIALED_MANIFEST="$(dirname "$BUNDLE_ROOT")/credentialed-output.json"
TRUSTED_RUNNER_TEMP="${RUNNER_TEMP:?runner temp is required}"
EXTRACT_ROOT="$(mktemp -d "$TRUSTED_RUNNER_TEMP/mdreader-final-packages.XXXXXX")"
MOUNT_POINT=''
mkdir -p "$EXTRACT_ROOT/package-snapshots"

cleanup() {
  if [[ -n "$MOUNT_POINT" ]]; then
    hdiutil detach "$MOUNT_POINT" >/dev/null 2>&1 || true
  fi
  rm -rf -- "$EXTRACT_ROOT"
}
trap cleanup EXIT

exact_file() {
  local label="${1:?label is required}"
  local pattern="${2:?pattern is required}"
  local matches="$EXTRACT_ROOT/${label//[^A-Za-z0-9]/_}.txt"
  find "$BUNDLE_ROOT" -type f -name "$pattern" -print > "$matches"
  local count
  count="$(wc -l < "$matches" | tr -d '[:space:]')"
  if [[ "$count" != '1' ]]; then
    echo "final package verification requires exactly one $label, found $count" >&2
    exit 1
  fi
  IFS= read -r REPLY < "$matches"
  printf '%s\n' "$REPLY"
}

snapshot_file() {
  local source="${1:?source package is required}"
  local name="${2:?snapshot name is required}"
  local snapshot="$EXTRACT_ROOT/package-snapshots/$name"
  cp -p -- "$source" "$snapshot"
  if ! cmp -s -- "$source" "$snapshot"; then
    echo "final package changed while its immutable verification snapshot was created" >&2
    exit 1
  fi
  printf '%s\n' "$snapshot"
}

verify_extractions() {
  local args=(
    --platform "$PLATFORM"
    --target "$TARGET"
    --version "$EXPECTED_VERSION"
    --source-commit "$SOURCE_COMMIT"
    --credentialed-manifest "$CREDENTIALED_MANIFEST"
    --output "$EVIDENCE_PATH"
  )
  if [[ "$PLATFORM" == 'macos' ]]; then
    [[ "${EXPECTED_APPLE_TEAM_ID:-}" =~ ^[A-Z0-9]{10}$ ]] || {
      echo "reviewed Apple team identifier is missing or invalid" >&2
      exit 1
    }
    args+=(--expected-publisher-identity "$EXPECTED_APPLE_TEAM_ID")
  elif [[ "$PLATFORM" == 'windows' ]]; then
    [[ "${EXPECTED_WINDOWS_CERTIFICATE_THUMBPRINT:-}" =~ ^[0-9A-F]{40}$ ]] || {
      echo "reviewed Windows certificate thumbprint is missing or invalid" >&2
      exit 1
    }
    args+=(
      --expected-publisher-identity
      "$EXPECTED_WINDOWS_CERTIFICATE_THUMBPRINT"
    )
  fi
  local package
  for package in "$@"; do
    args+=(--package "$package")
  done
  for package in "${ARTIFACT_ARGS[@]}"; do
    args+=(--artifact "$package")
  done
  pnpm exec tsx scripts/release/verify-package-binaries.ts "${args[@]}"
}

if [[ "$PLATFORM" == 'macos' ]]; then
  DMG_SOURCE="$(exact_file 'macOS DMG' '*.dmg')"
  UPDATER_SOURCE="$(exact_file 'macOS updater' '*.app.tar.gz')"
  DMG="$(snapshot_file "$DMG_SOURCE" 'release.dmg')"
  UPDATER="$(snapshot_file "$UPDATER_SOURCE" 'updater.app.tar.gz')"
  MOUNT_POINT="$EXTRACT_ROOT/dmg"
  mkdir -p "$MOUNT_POINT" "$EXTRACT_ROOT/updater"
  hdiutil attach \
    -nobrowse \
    -readonly \
    -mountpoint "$MOUNT_POINT" \
    "$DMG" >/dev/null
  node scripts/release/safe-package-extract.mjs scan "$MOUNT_POINT"
  node scripts/release/safe-package-extract.mjs \
    tar "$UPDATER" "$EXTRACT_ROOT/updater"
  ARTIFACT_ARGS=("dmg=$DMG" "updater=$UPDATER")
  scripts/release/verify-macos-packaged-runtime.sh \
    dmg "$MOUNT_POINT" \
    updater "$EXTRACT_ROOT/updater"
  verify_extractions \
    "dmg=$MOUNT_POINT" \
    "updater=$EXTRACT_ROOT/updater"
elif [[ "$PLATFORM" == 'windows' ]]; then
  INSTALLER_SOURCE="$(exact_file 'Windows NSIS installer' '*setup.exe')"
  UPDATER_SOURCE="$(exact_file 'Windows updater' '*.nsis.zip')"
  INSTALLER="$(snapshot_file "$INSTALLER_SOURCE" 'installer.exe')"
  UPDATER="$(snapshot_file "$UPDATER_SOURCE" 'updater.nsis.zip')"
  mkdir -p \
    "$EXTRACT_ROOT/nsis" \
    "$EXTRACT_ROOT/updater-archive" \
    "$EXTRACT_ROOT/updater"
  node scripts/release/safe-package-extract.mjs \
    7z "$INSTALLER" "$EXTRACT_ROOT/nsis"
  node scripts/release/safe-package-extract.mjs \
    7z "$UPDATER" "$EXTRACT_ROOT/updater-archive"
  NESTED_LIST="$EXTRACT_ROOT/nested-installers.txt"
  find "$EXTRACT_ROOT/updater-archive" -type f -iname '*setup.exe' -print \
    > "$NESTED_LIST"
  NESTED_COUNT="$(wc -l < "$NESTED_LIST" | tr -d '[:space:]')"
  if [[ "$NESTED_COUNT" != '1' ]]; then
    echo "Windows updater must contain exactly one NSIS installer, found $NESTED_COUNT" >&2
    exit 1
  fi
  IFS= read -r NESTED_INSTALLER < "$NESTED_LIST"
  node scripts/release/safe-package-extract.mjs \
    7z "$NESTED_INSTALLER" "$EXTRACT_ROOT/updater"
  ARTIFACT_ARGS=("nsis=$INSTALLER" "updater=$UPDATER")
  verify_extractions \
    "nsis=$EXTRACT_ROOT/nsis" \
    "updater=$EXTRACT_ROOT/updater"
elif [[ "$PLATFORM" == 'linux' ]]; then
  APPIMAGE_SOURCE="$(exact_file 'Linux AppImage' '*.AppImage')"
  DEB_SOURCE="$(exact_file 'Linux DEB' '*.deb')"
  RPM_SOURCE="$(exact_file 'Linux RPM' '*.rpm')"
  UPDATER_SOURCE="$(exact_file 'Linux updater' '*.AppImage.tar.gz')"
  APPIMAGE="$(snapshot_file "$APPIMAGE_SOURCE" 'release.AppImage')"
  DEB="$(snapshot_file "$DEB_SOURCE" 'release.deb')"
  RPM="$(snapshot_file "$RPM_SOURCE" 'release.rpm')"
  UPDATER="$(snapshot_file "$UPDATER_SOURCE" 'updater.AppImage.tar.gz')"
  DEB_PACKAGE="$(timeout --signal=KILL 15s dpkg-deb --field "$DEB" Package)"
  DEB_ARCH="$(timeout --signal=KILL 15s dpkg-deb --field "$DEB" Architecture)"
  DEB_VERSION="$(timeout --signal=KILL 15s dpkg-deb --field "$DEB" Version)"
  RPM_PACKAGE="$(timeout --signal=KILL 15s rpm -qp --queryformat '%{NAME}' "$RPM")"
  RPM_ARCH="$(timeout --signal=KILL 15s rpm -qp --queryformat '%{ARCH}' "$RPM")"
  RPM_VERSION="$(timeout --signal=KILL 15s rpm -qp --queryformat '%{VERSION}' "$RPM")"
  RPM_RELEASE="$(timeout --signal=KILL 15s rpm -qp --queryformat '%{RELEASE}' "$RPM")"
  [[ "$DEB_PACKAGE" == 'mdreader' && "$DEB_ARCH" == 'amd64' && "$DEB_VERSION" == "$EXPECTED_VERSION" ]] || {
    echo "DEB metadata does not identify the reviewed mdreader/amd64 release version" >&2
    exit 1
  }
  [[ "$RPM_PACKAGE" == 'mdreader' && "$RPM_ARCH" == 'x86_64' && "$RPM_VERSION" == "$EXPECTED_VERSION" && "$RPM_RELEASE" == '1' ]] || {
    echo "RPM metadata does not identify the reviewed mdreader/x86_64 release version" >&2
    exit 1
  }
  mkdir -p \
    "$EXTRACT_ROOT/appimage" \
    "$EXTRACT_ROOT/deb" \
    "$EXTRACT_ROOT/rpm" \
    "$EXTRACT_ROOT/updater-archive" \
    "$EXTRACT_ROOT/updater"
  node scripts/release/safe-package-extract.mjs \
    7z "$APPIMAGE" "$EXTRACT_ROOT/appimage"
  node scripts/release/safe-package-extract.mjs \
    deb "$DEB" "$EXTRACT_ROOT/deb"
  node scripts/release/safe-package-extract.mjs \
    rpm "$RPM" "$EXTRACT_ROOT/rpm"
  node scripts/release/safe-package-extract.mjs \
    tar "$UPDATER" "$EXTRACT_ROOT/updater-archive"
  UPDATER_LIST="$EXTRACT_ROOT/updater-appimages.txt"
  find "$EXTRACT_ROOT/updater-archive" -type f -name '*.AppImage' -print \
    > "$UPDATER_LIST"
  UPDATER_COUNT="$(wc -l < "$UPDATER_LIST" | tr -d '[:space:]')"
  if [[ "$UPDATER_COUNT" != '1' ]]; then
    echo "Linux updater must contain exactly one AppImage, found $UPDATER_COUNT" >&2
    exit 1
  fi
  IFS= read -r UPDATER_APPIMAGE < "$UPDATER_LIST"
  node scripts/release/safe-package-extract.mjs \
    7z "$UPDATER_APPIMAGE" "$EXTRACT_ROOT/updater"
  ARTIFACT_ARGS=(
    "appimage=$APPIMAGE"
    "deb=$DEB"
    "rpm=$RPM"
    "updater=$UPDATER"
  )
  verify_extractions \
    "appimage=$EXTRACT_ROOT/appimage" \
    "deb=$EXTRACT_ROOT/deb" \
    "rpm=$EXTRACT_ROOT/rpm" \
    "updater=$EXTRACT_ROOT/updater"
else
  echo "unsupported final package platform $PLATFORM" >&2
  exit 1
fi
