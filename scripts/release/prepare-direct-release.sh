#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:?target is required}"
_BUNDLES="${2:?bundles are required}"
CONFIG="${3:?release config is required}"

node scripts/release/public-oauth-config.mjs assert-hashes

if [[ "$TARGET" == "universal-apple-darwin" ]]; then
  [[ -n "${MDREADER_MACOS_ORT_LIB_LOCATION:-}" ]] || {
    echo "macOS release is missing its reviewed ONNX Runtime directory" >&2
    exit 1
  }
  [[ -n "${MDREADER_MACOS_ORT_MANIFEST:-}" ]] || {
    echo "macOS release is missing its reviewed ONNX Runtime manifest" >&2
    exit 1
  }
  [[ "$MDREADER_MACOS_ORT_MANIFEST" == "$MDREADER_MACOS_ORT_LIB_LOCATION/runtime.json" ]] || {
    echo "macOS ONNX Runtime manifest is not bound to its library directory" >&2
    exit 1
  }
  node scripts/release/prepare-macos-ort-runtime.mjs \
    --verify \
    --output "$MDREADER_MACOS_ORT_LIB_LOCATION"
  export ORT_LIB_LOCATION="$MDREADER_MACOS_ORT_LIB_LOCATION"
  export ORT_PREFER_DYNAMIC_LINK=1
elif [[ -n "${MDREADER_MACOS_ORT_LIB_LOCATION:-}${MDREADER_MACOS_ORT_MANIFEST:-}" ]]; then
  echo "non-macOS release received macOS ONNX Runtime authority" >&2
  exit 1
fi

pnpm --filter '@mdreader/app...' run build

# Compile the shipping binary before private updater, Apple, or Windows
# credentials enter any process. The release overlay disables the normal
# beforeBuildCommand so application dependencies are not rerun by Tauri.
pnpm --filter @mdreader/app tauri build \
  --no-bundle \
  --target "$TARGET" \
  --features direct-updater \
  --config "$CONFIG"
