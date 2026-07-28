#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
VERSION="${2:?version is required}"
OUTPUT="${3:?output path is required}"
COMMAND=(
  pnpm exec tsx scripts/release/release-cli.ts config
  --version "$VERSION"
  --output "$OUTPUT"
)
case "$PLATFORM" in
  macos)
    [[ -n "${MDREADER_MACOS_ORT_DYLIB:-}" ]] || {
      echo "macOS release requires the reviewed ONNX Runtime dylib" >&2
      exit 1
    }
    COMMAND+=(--macos-framework "$MDREADER_MACOS_ORT_DYLIB")
    ;;
  windows)
    # This script is the unsigned boundary. Ambient signing variables are
    # deliberately ignored; credentialed jobs pass signing values as explicit
    # release-cli arguments when they generate their separate bundle overlay.
    ;;
  linux) ;;
  *)
    echo "unsupported release platform: $PLATFORM" >&2
    exit 1
    ;;
esac
"${COMMAND[@]}"
