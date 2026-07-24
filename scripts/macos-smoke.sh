#!/usr/bin/env bash
#
# Milestone PT — macOS GUI smoke entry point (UNIFIED Appium/mac2 harness).
#
# Builds (or reuses) the MDReader .app, then runs the macOS leg of the unified
# harness (apps/mdreader/e2e-native/runners/macos.ts), which drives the REAL
# window through an appium-mac2-driver (XCUITest) WebDriver session and asserts the
# shared FLOW — including the two assertions a "process booted" check can't make:
# a real titlebar drag moves the window (with a content-area negative control), and
# the red-X close ends the process. The webview UI is read through WebKit's
# Accessibility tree (WKWebView exposes no WebDriver bridge).
#
# The Node runner owns the Appium session lifecycle: it kills any stale
# WebDriverAgentMac host (a reused host wedges new sessions), re-seeds the fixture
# workspace, launches the app, and — if no Appium server is already listening on
# :4723 — spawns one from the local APPIUM_HOME and tears it down afterwards.
#
# Exits non-zero on any failed step. macOS only (WKWebView has no WebDriver, so
# this is the only leg that exercises the real app on a Mac).
#
# Usage:
#   scripts/macos-smoke.sh                 # build if missing, then run
#   scripts/macos-smoke.sh --skip-build    # reuse the existing .app
#   scripts/macos-smoke.sh --keep          # leave the window open on finish
#
# Fault injection (RED proof):
#   MDREADER_SMOKE_FAULT=<step-id> scripts/macos-smoke.sh --skip-build
#
# Requires: a Mac with full Xcode (mac2-driver builds WebDriverAgentMac via
# xcodebuild), Node >= 22.13, pnpm. The controlling terminal needs Accessibility
# (for the AX reads) and Screen-Recording (for the capture oracle) in System
# Settings -> Privacy. No AppleScript/Automation grant is needed anymore.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$REPO_ROOT/target/release/bundle/macos/MDReader.app"
E2E_DIR="$REPO_ROOT/apps/mdreader/e2e-native"
export APPIUM_HOME="$E2E_DIR/.appium"

SKIP_BUILD=0
RUNNER_ARGS=()
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1 ;;
    *) RUNNER_ARGS+=("$arg") ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macos-smoke.sh runs on macOS only (this is $(uname -s))." >&2
  exit 2
fi

if [[ "$SKIP_BUILD" -eq 0 || ! -d "$APP" ]]; then
  echo "==> Building MDReader.app (release, app bundle only)…"
  pnpm --filter '@mdreader/app...' run build
  pnpm --filter @mdreader/app tauri build --bundles app
else
  echo "==> Reusing existing app: $APP"
fi

echo "==> Ensuring the e2e-native toolchain + appium-mac2-driver are installed…"
if [[ ! -d "$E2E_DIR/node_modules/webdriverio" ]]; then
  (cd "$E2E_DIR" && npm install)
fi
# Install the mac2 driver into the local APPIUM_HOME if it isn't already there.
if ! (cd "$E2E_DIR" && npx appium driver list --installed 2>/dev/null | grep -q 'mac2'); then
  (cd "$E2E_DIR" && npx appium driver install mac2)
fi

echo "==> Running macOS GUI smoke (unified mac2 runner)…"
# Node runs the TypeScript runner directly via native type-stripping.
# --experimental-strip-types is REQUIRED on Node 22.13–22.x; a no-op on Node >= 23.
exec node --experimental-strip-types --no-warnings \
  "$E2E_DIR/runners/macos.ts" --app "$APP" "${RUNNER_ARGS[@]}"
