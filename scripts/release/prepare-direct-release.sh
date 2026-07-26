#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:?target is required}"
BUNDLES="${2:?bundles are required}"
CONFIG="${3:?release config is required}"
ATTESTATION="${4:?build attestation path is required}"

pnpm exec tsx scripts/release/release-cli.ts attest-build \
  --config "$CONFIG" \
  --attestation "$ATTESTATION" \
  --target "$TARGET" \
  --bundles "$BUNDLES"

pnpm --filter '@mdreader/app...' run build

# Compile the shipping binary before private updater, Apple, or Windows
# credentials enter any process. The release overlay disables the normal
# beforeBuildCommand so application dependencies are not rerun by Tauri.
pnpm --filter @mdreader/app tauri build \
  --no-bundle \
  --target "$TARGET" \
  --features direct-updater \
  --config "$CONFIG"
