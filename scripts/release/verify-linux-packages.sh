#!/usr/bin/env bash
set -euo pipefail

BUNDLE_ROOT="${1:?bundle root is required}"
REPORT_PATH="${2:?ABI report path is required}"
mapfile -t APPIMAGES < <(find "$BUNDLE_ROOT" -type f -name '*.AppImage')
mapfile -t DEBS < <(find "$BUNDLE_ROOT" -type f -name '*.deb')
mapfile -t RPMS < <(find "$BUNDLE_ROOT" -type f -name '*.rpm')
[[ "${#APPIMAGES[@]}" -eq 1 && "${#DEBS[@]}" -eq 1 && "${#RPMS[@]}" -eq 1 ]] || {
  echo 'incomplete Linux package matrix' >&2
  exit 1
}
chmod +x "${APPIMAGES[0]}"
"${APPIMAGES[0]}" --appimage-extract >/dev/null
find squashfs-root -type f -exec sh -c \
  'readelf --version-info "$1" 2>/dev/null || true' _ {} \; \
  > "$REPORT_PATH"
pnpm exec tsx scripts/release/release-cli.ts verify-linux-abi \
  --report "$REPORT_PATH" \
  --max-glibc 2.31
dpkg-deb --info "${DEBS[0]}" >/dev/null
rpm -qpi "${RPMS[0]}" >/dev/null
