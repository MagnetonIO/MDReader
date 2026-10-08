#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:?target is required}"
BUNDLES="${2:?bundles are required}"
CONFIG="${3:?release config is required}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

case "$CONFIG" in
  /*) ;;
  *) CONFIG="$ROOT/$CONFIG" ;;
esac

cd "$ROOT/src-tauri"

# Replace the reviewed shell with the Tauri signer/bundler. No frontend,
# TypeScript, package-manager, Cargo, verifier, or scanner process runs inside
# this credential-bearing step.
exec node "$ROOT/ui/node_modules/@tauri-apps/cli/tauri.js" bundle \
  --target "$TARGET" \
  --bundles "$BUNDLES" \
  --config "$CONFIG"
