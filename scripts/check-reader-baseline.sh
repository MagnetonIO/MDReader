#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
pnpm --filter @mdreader/ui... build
pnpm --filter @mdreader/app typecheck
pnpm --filter @mdreader/app test
pnpm --filter @mdreader/markdown test
pnpm --filter @mdreader/ui test
cargo test -p mdreader-connector-local -p mdreader-domain -p mdreader-ingest -p mdreader-presentation -p mdreader-connector-dropbox -p mdreader-connector-google-drive --lib
cargo test -p mdreader-connector-dropbox --test dropbox_api_v2_contract
cargo test -p mdreader-connector-google-drive --test drive_api_v3_contract
cargo test -p mdreader-app --lib
