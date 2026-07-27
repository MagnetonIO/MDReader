#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
VERSION="${2:?version is required}"
SOURCE_COMMIT="${3:?source commit is required}"
TARGET="${4:?target is required}"
BUNDLES="${5:?bundles are required}"
DOWNLOAD_ROOT="${6:?credentialed archive download root is required}"
OUTPUT_ROOT="${7:?credentialed output root is required}"
UPDATER_PUBLIC_KEY_SHA256="${8:?updater public key fingerprint is required}"

node scripts/release/extract-credentialed-output.mjs \
  "$PLATFORM" \
  "$DOWNLOAD_ROOT" \
  "$OUTPUT_ROOT"

pnpm exec tsx scripts/release/release-cli.ts verify-credentialed-output \
  --platform "$PLATFORM" \
  --version "$VERSION" \
  --source-commit "$SOURCE_COMMIT" \
  --target "$TARGET" \
  --bundles "$BUNDLES" \
  --updater-public-key-sha256 "$UPDATER_PUBLIC_KEY_SHA256" \
  --root "$OUTPUT_ROOT" \
  --manifest "$OUTPUT_ROOT/credentialed-output.json"
