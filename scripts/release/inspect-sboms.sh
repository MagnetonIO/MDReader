#!/usr/bin/env bash
set -euo pipefail

STAGE_ROOT="${1:?stage root is required}"
FOUND=false
while IFS= read -r -d '' SBOM; do
  FOUND=true
  pnpm exec tsx -e \
    "const d=JSON.parse(require('node:fs').readFileSync(process.argv[1],'utf8')); if(d.spdxVersion!=='SPDX-2.3') throw new Error('invalid SPDX document')" \
    "$SBOM"
done < <(find "$STAGE_ROOT" -type f -name '*.spdx.json' -print0)
[[ "$FOUND" == true ]] || { echo 'no SBOM documents generated' >&2; exit 1; }
