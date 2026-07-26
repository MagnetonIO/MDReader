#!/usr/bin/env bash
set -euo pipefail

BUNDLE_ROOT="${1:?bundle root is required}"
APP_PATH="$(find "$BUNDLE_ROOT" -type d -name 'MDReader.app' -print -quit)"
[[ -n "$APP_PATH" ]] || { echo 'MDReader.app missing' >&2; exit 1; }
codesign --verify --deep --strict --verbose=2 "$APP_PATH"
spctl --assess --type execute --verbose=2 "$APP_PATH"
