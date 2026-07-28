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
    if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
      COMMAND+=(--macos-signing-identity "$APPLE_SIGNING_IDENTITY")
    fi
    ;;
  windows)
    # The uncredentialed build matrix generates this configuration before any
    # certificate is inspected, so the thumbprint is legitimately absent there;
    # only the credentialed run supplies it. Signing inputs stay one exact set.
    if [[ -n "${WINDOWS_CERT_THUMBPRINT:-}" ]]; then
      COMMAND+=(--windows-thumbprint "$WINDOWS_CERT_THUMBPRINT")
    fi
    ;;
  linux) ;;
  *)
    echo "unsupported release platform: $PLATFORM" >&2
    exit 1
    ;;
esac
"${COMMAND[@]}"
