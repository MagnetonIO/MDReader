#!/usr/bin/env bash
set -euo pipefail

OUTPUT="${1:?publisher output is required}"
mkdir -p "$(dirname "$OUTPUT")"

pnpm exec esbuild scripts/release/publish-sealed-plan.ts \
  --bundle \
  --platform=node \
  --format=esm \
  --target=node22 \
  --outfile="$OUTPUT"

if grep -Eq 'node:child_process|require\\(["'"'"']child_process["'"'"']\\)|cargo run|pnpm exec|tsx' "$OUTPUT"; then
  printf 'sealed publisher contains a forbidden general-tool subprocess path\n' >&2
  exit 1
fi

PUBLISHER_SHA256="$(
  node -e \
    "const fs=require('node:fs');const c=require('node:crypto');process.stdout.write(c.createHash('sha256').update(fs.readFileSync(process.argv[1])).digest('hex'))" \
    "$OUTPUT"
)"
printf 'publisher_sha256=%s\n' "$PUBLISHER_SHA256" >> "$GITHUB_OUTPUT"
