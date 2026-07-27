#!/usr/bin/env bash
set -euo pipefail

BUNDLE_ROOT="${1:?bundle root is required}"
REPORT_PATH="${2:?ABI report path is required}"
IDENTITY_EVIDENCE="${3:?identity evidence path is required}"
EXPECTED_VERSION="${4:?expected version is required}"
TRUSTED_RUNNER_TEMP="${RUNNER_TEMP:?runner temp is required}"
INSPECTION_ROOT="$(mktemp -d "$TRUSTED_RUNNER_TEMP/mdreader-linux-inspection.XXXXXX")"
trap 'rm -rf -- "$INSPECTION_ROOT"' EXIT

mapfile -t APPIMAGES < <(find "$BUNDLE_ROOT" -type f -name '*.AppImage')
mapfile -t DEBS < <(find "$BUNDLE_ROOT" -type f -name '*.deb')
mapfile -t RPMS < <(find "$BUNDLE_ROOT" -type f -name '*.rpm')
[[ "${#APPIMAGES[@]}" -eq 1 && "${#DEBS[@]}" -eq 1 && "${#RPMS[@]}" -eq 1 ]] || {
  echo 'incomplete Linux package matrix' >&2
  exit 1
}

node - "$IDENTITY_EVIDENCE" "${APPIMAGES[0]}" "$EXPECTED_VERSION" <<'NODE'
const { createHash } = require('node:crypto');
const { readFileSync } = require('node:fs');
const [evidencePath, appImagePath, expectedVersion] = process.argv.slice(2);
const evidence = JSON.parse(readFileSync(evidencePath, 'utf8'));
const appimage = evidence.packages?.find((entry) => entry.kind === 'appimage');
const actual = createHash('sha256')
  .update(readFileSync(appImagePath))
  .digest('hex');
if (
  evidence.schemaVersion !== 2 ||
  evidence.platform !== 'linux' ||
  evidence.version !== expectedVersion ||
  appimage?.artifactSha256 !== actual ||
  appimage?.entrypointContract !== 'linux-apprun-target' ||
  appimage?.entrypointPath !== 'usr/bin/mdreader-app'
) {
  throw new Error('Linux package smoke is not bound to reviewed package identity evidence');
}
NODE

node scripts/release/safe-package-extract.mjs \
  7z "${APPIMAGES[0]}" "$INSPECTION_ROOT/appimage"
find "$INSPECTION_ROOT/appimage" -type f -exec sh -c \
  'readelf --version-info "$1" 2>/dev/null || true' _ {} \; \
  > "$REPORT_PATH"
pnpm exec tsx scripts/release/release-cli.ts verify-linux-abi \
  --report "$REPORT_PATH" \
  --max-glibc 2.31
dpkg-deb --info "${DEBS[0]}" >/dev/null
rpm -qpi "${RPMS[0]}" >/dev/null
