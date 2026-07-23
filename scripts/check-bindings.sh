#!/usr/bin/env bash
#
# tauri-specta binding drift guard.
#
# The TypeScript bindings at apps/mdreader/ui/src/bindings.ts are GENERATED from
# the Rust command surface in apps/mdreader/src-tauri and committed to the repo.
# They must never drift from the Rust source of truth.
#
# This regenerates them headlessly via the `export_bindings` test (no window, no
# webview, no dev server) and then fails if the working tree changed. CI runs
# this to reject PRs whose bindings weren't regenerated after a command change.
#
# Usage: ./scripts/check-bindings.sh
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

cd "$repo_root"

# The test writes apps/mdreader/ui/src/bindings.ts as a side effect. `--quiet`
# keeps CI logs readable; the export itself panics (non-zero) on failure.
cargo test --package mdreader-app export_bindings --quiet

git diff --exit-code -- apps/mdreader/ui/src/bindings.ts
