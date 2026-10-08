#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

node - <<'NODE'
const fs = require('node:fs');
const config = JSON.parse(fs.readFileSync('src-tauri/tauri.e2e.conf.json', 'utf8'));
if (config.identifier !== 'dev.mdreader.app.e2e') throw new Error('E2E identifier changed');
if (config.mainBinaryName !== 'mdreader-e2e') throw new Error('E2E process name changed');
if (config.bundle?.createUpdaterArtifacts !== false) throw new Error('E2E updater artifacts are enabled');
if (config.plugins?.updater) throw new Error('E2E contains a live updater configuration');
NODE

if cargo tree --locked -p mdreader-app --no-default-features | grep -q 'tauri-plugin-updater'; then
  echo 'E2E/default dependency graph unexpectedly compiles tauri-plugin-updater' >&2
  exit 1
fi

cargo tree --locked -p mdreader-app --features direct-updater |
  grep -q 'tauri-plugin-updater' || {
    echo 'direct release feature does not compile tauri-plugin-updater' >&2
    exit 1
  }

CONFLICT_LOG="$(mktemp)"
trap 'rm -f "$CONFLICT_LOG"' EXIT
if cargo check --locked -p mdreader-app --no-default-features --features e2e-build,direct-updater \
  >"$CONFLICT_LOG" 2>&1; then
  echo 'forbidden E2E/direct-updater combination unexpectedly compiled' >&2
  exit 1
fi
grep -q \
  'the direct updater cannot be compiled into the isolated E2E application' \
  "$CONFLICT_LOG" || {
    echo 'forbidden feature combination failed for an unrelated reason' >&2
    cat "$CONFLICT_LOG" >&2
    exit 1
  }

echo 'E2E updater isolation verified'
