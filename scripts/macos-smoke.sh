#!/usr/bin/env bash
#
# Milestone PT — macOS GUI smoke entry point.
#
# Builds (or reuses) the MDReader .app, then runs the native smoke driver
# (apps/mdreader/e2e-native/macos/smoke.ts) which drives the REAL window via
# System Events + synthetic CoreGraphics input and asserts the core flow —
# including the two assertions a "process booted" check can't make: a real
# titlebar drag moves the window, and the red-X close ends the process.
#
# Exits non-zero on any failed step. macOS only (WKWebView has no WebDriver, so
# this is the only harness that exercises the real app on a Mac).
#
# Usage:
#   scripts/macos-smoke.sh                 # build if missing, then run
#   scripts/macos-smoke.sh --skip-build    # reuse the existing .app
#   scripts/macos-smoke.sh --keep          # leave the window open on finish
#
# Requires: a Mac with Xcode command-line tools (swiftc), Node >= 22.13, pnpm.
# The driver must be able to control the machine — grant the controlling
# terminal Accessibility + Screen Recording (System Settings → Privacy).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$REPO_ROOT/target/release/bundle/macos/MDReader.app"
DRIVER="$REPO_ROOT/apps/mdreader/e2e-native/macos/smoke.ts"

SKIP_BUILD=0
DRIVER_ARGS=()
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1 ;;
    *) DRIVER_ARGS+=("$arg") ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macos-smoke.sh runs on macOS only (this is $(uname -s))." >&2
  exit 2
fi

if [[ "$SKIP_BUILD" -eq 0 || ! -d "$APP" ]]; then
  echo "==> Building MDReader.app (release, app bundle only)…"
  # The app's workspace deps (@mdreader/ui, @mdreader/markdown) must be built
  # before Vite can resolve them; turbo does this topologically.
  pnpm --filter '@mdreader/app...' run build
  pnpm --filter @mdreader/app tauri build --bundles app
else
  echo "==> Reusing existing app: $APP"
fi

echo "==> Running macOS GUI smoke driver…"
# Node runs the TypeScript driver directly (native type-stripping). --no-warnings
# hushes the experimental-strip-types notice on Node < 23.
exec node --no-warnings "$DRIVER" --app "$APP" "${DRIVER_ARGS[@]}"
