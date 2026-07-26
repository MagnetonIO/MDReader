#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
VERSION="${2:?version is required}"
SOURCE_COMMIT="${3:?source commit is required}"
PUBLISHED_AT="${4:?published timestamp is required}"
TARGET="${5:?target is required}"
BUNDLES="${6:?bundles are required}"
RELEASE_CONFIG="${7:?release config is required}"
BUILD_ATTESTATION="${8:?build attestation is required}"

pnpm exec tsx scripts/release/release-cli.ts stage \
  --platform "$PLATFORM" \
  --version "$VERSION" \
  --source-commit "$SOURCE_COMMIT" \
  --published-at "$PUBLISHED_AT" \
  --bundle-root "target/$TARGET/release/bundle" \
  --output-root target/release-stage \
  --release-config "$RELEASE_CONFIG" \
  --build-attestation "$BUILD_ATTESTATION" \
  --build-target "$TARGET" \
  --build-bundles "$BUNDLES"
