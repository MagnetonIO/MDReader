#!/usr/bin/env bash
set -euo pipefail

[[ "$#" -ge 2 && $(( $# % 2 )) -eq 0 ]] || {
  echo 'package label/root pairs are required' >&2
  exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

while [[ "$#" -gt 0 ]]; do
  LABEL="$1"
  ROOT="$2"
  shift 2

  [[ -d "$ROOT" && ! -L "$ROOT" ]] || {
    echo "$LABEL extracted package root is missing" >&2
    exit 1
  }
  MATCHES="$(mktemp "${TMPDIR:-/tmp}/mdreader-${LABEL//[^A-Za-z0-9]/_}-apps.XXXXXX")"
  trap 'rm -f -- "$MATCHES"' EXIT
  find "$ROOT" -type d -name 'MDReader.app' -prune -print > "$MATCHES"
  COUNT="$(wc -l < "$MATCHES" | tr -d '[:space:]')"
  if [[ "$COUNT" != '1' ]]; then
    echo "$LABEL package must contain exactly one MDReader.app, found $COUNT" >&2
    exit 1
  fi
  IFS= read -r APP_PATH < "$MATCHES"
  "$SCRIPT_DIR/verify-macos-runtime-linkage.sh" bundled "$APP_PATH"
  rm -f -- "$MATCHES"
  trap - EXIT
done
