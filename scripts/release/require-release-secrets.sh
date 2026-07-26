#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"

COMMON=(
  TAURI_SIGNING_PRIVATE_KEY
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  MDREADER_UPDATER_PUBLIC_KEY
)

case "$PLATFORM" in
  macos)
    REQUIRED=(
      "${COMMON[@]}"
      APPLE_CERTIFICATE
      APPLE_CERTIFICATE_PASSWORD
      APPLE_SIGNING_IDENTITY
      APPLE_ID
      APPLE_PASSWORD
      APPLE_TEAM_ID
    )
    ;;
  windows)
    REQUIRED=(
      "${COMMON[@]}"
      WINDOWS_CERTIFICATE
      WINDOWS_CERTIFICATE_PASSWORD
    )
    ;;
  linux)
    REQUIRED=("${COMMON[@]}")
    ;;
  *)
    printf 'unsupported release platform %s\n' "$PLATFORM" >&2
    exit 1
    ;;
esac

MISSING=()
for NAME in "${REQUIRED[@]}"; do
  if [[ -z "${!NAME:-}" ]]; then
    MISSING+=("$NAME")
  fi
done

if (( ${#MISSING[@]} > 0 )); then
  printf 'missing required release credentials: %s\n' "${MISSING[*]}" >&2
  exit 1
fi

printf 'release credentials present for %s\n' "$PLATFORM"
