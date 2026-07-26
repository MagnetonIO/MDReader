#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
STAGE_ROOT="${2:?stage root is required}"
FRAGMENT="$STAGE_ROOT/fragment-$PLATFORM.json"
test -s "$FRAGMENT"

if [[ "$PLATFORM" == "macos" ]]; then EXPECTED=2
elif [[ "$PLATFORM" == "windows" ]]; then EXPECTED=2
else EXPECTED=4
fi

ACTUAL="$(
  pnpm exec tsx -e \
    "const d=JSON.parse(require('node:fs').readFileSync(process.argv[1],'utf8')); if(d.artifacts.some((a)=>!a.sha256||!a.sbomSha256||(a.kind==='updater'&&!a.signature))) throw new Error('incomplete checksum, SBOM, or signature metadata'); process.stdout.write(String(d.artifacts.length))" \
    "$FRAGMENT"
)"
[[ "$ACTUAL" == "$EXPECTED" ]] || { echo 'partial staged platform matrix' >&2; exit 1; }
