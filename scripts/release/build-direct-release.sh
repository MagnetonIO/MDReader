#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:?target is required}"
BUNDLES="${2:?bundles are required}"
CONFIG="${3:?release config is required}"
ATTESTATION="${4:?build attestation path is required}"
SEAL="${5:?bundle seal path is required}"
VERSION="$(
  pnpm exec tsx -e \
    "import { readWorkspaceVersion } from './scripts/release/release-config.ts'; process.stdout.write(readWorkspaceVersion('./Cargo.toml'))"
)"

pnpm exec tsx scripts/release/release-cli.ts attest-build \
  --config "$CONFIG" \
  --attestation "$ATTESTATION" \
  --target "$TARGET" \
  --bundles "$BUNDLES"
pnpm --filter '@mdreader/app...' run build
pnpm --filter @mdreader/app tauri build \
  --target "$TARGET" \
  --bundles "$BUNDLES" \
  --features direct-updater \
  --config "$CONFIG"
pnpm exec tsx scripts/release/release-cli.ts verify-build-attestation \
  --config "$CONFIG" \
  --attestation "$ATTESTATION" \
  --version "$VERSION" \
  --target "$TARGET" \
  --bundles "$BUNDLES"
pnpm exec tsx scripts/release/release-cli.ts seal-build \
  --bundle-root "target/$TARGET/release/bundle" \
  --config "$CONFIG" \
  --attestation "$ATTESTATION" \
  --seal "$SEAL" \
  --version "$VERSION" \
  --target "$TARGET" \
  --bundles "$BUNDLES"
