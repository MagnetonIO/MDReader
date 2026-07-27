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
TRUSTED_RUNNER_TEMP="${RUNNER_TEMP:?RUNNER_TEMP is required}"
CANDIDATE_PARENT="$(mktemp -d "$TRUSTED_RUNNER_TEMP/mdreader-credentialed-candidate.XXXXXX")"
CANDIDATE_ROOT="$CANDIDATE_PARENT/mdreader-credentialed-output"

cleanup_candidate() {
  rm -rf -- "$CANDIDATE_PARENT"
}
trap cleanup_candidate EXIT

node scripts/release/extract-credentialed-output.mjs \
  "$PLATFORM" \
  "$DOWNLOAD_ROOT" \
  "$CANDIDATE_ROOT"

pnpm exec tsx scripts/release/release-cli.ts verify-credentialed-output \
  --platform "$PLATFORM" \
  --version "$VERSION" \
  --source-commit "$SOURCE_COMMIT" \
  --target "$TARGET" \
  --bundles "$BUNDLES" \
  --updater-public-key-sha256 "$UPDATER_PUBLIC_KEY_SHA256" \
  --root "$CANDIDATE_ROOT" \
  --manifest "$CANDIDATE_ROOT/credentialed-output.json"

node scripts/release/publish-credentialed-output.mjs \
  "$CANDIDATE_ROOT" \
  "$OUTPUT_ROOT" \
  "$TRUSTED_RUNNER_TEMP"

cleanup_candidate
trap - EXIT
