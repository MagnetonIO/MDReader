#!/usr/bin/env bash
# Combined integration checkpoint, not a per-feature builder loop.
set -euo pipefail
cd "$(dirname "$0")/.."

# Build shared package declarations once before consumers typecheck.
pnpm --filter @mdreader/ui... build
pnpm --filter @mdreader/app typecheck
pnpm --filter @mdreader/app test
pnpm --filter @mdreader/markdown test
pnpm --filter @mdreader/ui test

# One Cargo invocation shares the native feature graph/cache across the retained
# reader crates. The removed CatDB adapter crates/contracts are no longer gates.
# Real provider/host/browser/release acceptance remains a separate checkpoint.
cargo test --locked --workspace --lib
