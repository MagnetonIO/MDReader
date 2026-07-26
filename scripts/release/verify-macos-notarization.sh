#!/usr/bin/env bash
set -euo pipefail

BUNDLE_ROOT="${1:?bundle root is required}"
DMG_PATH="$(find "$BUNDLE_ROOT" -type f -name '*.dmg' -print -quit)"
[[ -n "$DMG_PATH" ]] || { echo 'signed DMG missing' >&2; exit 1; }
xcrun stapler validate "$DMG_PATH"
spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG_PATH"
