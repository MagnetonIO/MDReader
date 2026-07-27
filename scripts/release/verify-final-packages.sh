#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
BUNDLE_ROOT="${2:?bundle root is required}"
SOURCE_COMMIT="${3:?source commit is required}"
TARGET="${4:?target is required}"
EVIDENCE_PATH="${5:?evidence output is required}"
TRUSTED_RUNNER_TEMP="${RUNNER_TEMP:?runner temp is required}"
EXTRACT_ROOT="$(mktemp -d "$TRUSTED_RUNNER_TEMP/mdreader-final-packages.XXXXXX")"
MOUNT_POINT=''

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

verify_extractions() {
  local args=(
    --platform "$PLATFORM"
    --target "$TARGET"
    --source-commit "$SOURCE_COMMIT"
    --output "$EVIDENCE_PATH"
  )
  local package
  for package in "$@"; do
    args+=(--package "$package")
  done
  pnpm exec tsx scripts/release/verify-package-binaries.ts "${args[@]}"
}

if [[ "$PLATFORM" == 'macos' ]]; then
  DMG="$(exact_file 'macOS DMG' '*.dmg')"
  UPDATER="$(exact_file 'macOS updater' '*.app.tar.gz')"
  MOUNT_POINT="$EXTRACT_ROOT/dmg"
  mkdir -p "$MOUNT_POINT" "$EXTRACT_ROOT/updater"
  hdiutil attach \
    -nobrowse \
    -readonly \
    -mountpoint "$MOUNT_POINT" \
    "$DMG" >/dev/null
  tar -xzf "$UPDATER" -C "$EXTRACT_ROOT/updater"
  verify_extractions \
    "dmg=$MOUNT_POINT" \
    "updater=$EXTRACT_ROOT/updater"
elif [[ "$PLATFORM" == 'windows' ]]; then
  INSTALLER="$(exact_file 'Windows NSIS installer' '*setup.exe')"
  UPDATER="$(exact_file 'Windows updater' '*.nsis.zip')"
  mkdir -p \
    "$EXTRACT_ROOT/nsis" \
    "$EXTRACT_ROOT/updater-archive" \
    "$EXTRACT_ROOT/updater"
  7z x -y "-o$EXTRACT_ROOT/nsis" "$INSTALLER" >/dev/null
  7z x -y "-o$EXTRACT_ROOT/updater-archive" "$UPDATER" >/dev/null
  NESTED_LIST="$EXTRACT_ROOT/nested-installers.txt"
  find "$EXTRACT_ROOT/updater-archive" -type f -iname '*setup.exe' -print \
    > "$NESTED_LIST"
  NESTED_COUNT="$(wc -l < "$NESTED_LIST" | tr -d '[:space:]')"
  if [[ "$NESTED_COUNT" != '1' ]]; then
    echo "Windows updater must contain exactly one NSIS installer, found $NESTED_COUNT" >&2
    exit 1
  fi
  IFS= read -r NESTED_INSTALLER < "$NESTED_LIST"
  7z x -y "-o$EXTRACT_ROOT/updater" "$NESTED_INSTALLER" >/dev/null
  verify_extractions \
    "nsis=$EXTRACT_ROOT/nsis" \
    "updater=$EXTRACT_ROOT/updater"
elif [[ "$PLATFORM" == 'linux' ]]; then
  APPIMAGE="$(exact_file 'Linux AppImage' '*.AppImage')"
  DEB="$(exact_file 'Linux DEB' '*.deb')"
  RPM="$(exact_file 'Linux RPM' '*.rpm')"
  UPDATER="$(exact_file 'Linux updater' '*.AppImage.tar.gz')"
  mkdir -p \
    "$EXTRACT_ROOT/appimage" \
    "$EXTRACT_ROOT/deb" \
    "$EXTRACT_ROOT/rpm" \
    "$EXTRACT_ROOT/updater-archive" \
    "$EXTRACT_ROOT/updater"
  7z x -y "-o$EXTRACT_ROOT/appimage" "$APPIMAGE" >/dev/null
  dpkg-deb -x "$DEB" "$EXTRACT_ROOT/deb"
  (
    cd "$EXTRACT_ROOT/rpm"
    rpm2cpio "$RPM" | cpio -idm --quiet
  )
  tar -xzf "$UPDATER" -C "$EXTRACT_ROOT/updater-archive"
  UPDATER_LIST="$EXTRACT_ROOT/updater-appimages.txt"
  find "$EXTRACT_ROOT/updater-archive" -type f -name '*.AppImage' -print \
    > "$UPDATER_LIST"
  UPDATER_COUNT="$(wc -l < "$UPDATER_LIST" | tr -d '[:space:]')"
  if [[ "$UPDATER_COUNT" != '1' ]]; then
    echo "Linux updater must contain exactly one AppImage, found $UPDATER_COUNT" >&2
    exit 1
  fi
  IFS= read -r UPDATER_APPIMAGE < "$UPDATER_LIST"
  7z x -y "-o$EXTRACT_ROOT/updater" "$UPDATER_APPIMAGE" >/dev/null
  verify_extractions \
    "appimage=$EXTRACT_ROOT/appimage" \
    "deb=$EXTRACT_ROOT/deb" \
    "rpm=$EXTRACT_ROOT/rpm" \
    "updater=$EXTRACT_ROOT/updater"
else
  echo "unsupported final package platform $PLATFORM" >&2
  exit 1
fi
