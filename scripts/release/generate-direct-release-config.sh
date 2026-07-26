#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
VERSION="${2:?version is required}"
OUTPUT="${3:?output path is required}"
ARGS=()
if [[ "$PLATFORM" == "macos" ]]; then
  ARGS+=(--macos-signing-identity "$APPLE_SIGNING_IDENTITY")
elif [[ "$PLATFORM" == "windows" ]]; then
  ARGS+=(--windows-thumbprint "$WINDOWS_CERT_THUMBPRINT")
fi
pnpm exec tsx scripts/release/release-cli.ts config \
  --version "$VERSION" \
  --output "$OUTPUT" \
  "${ARGS[@]}"
