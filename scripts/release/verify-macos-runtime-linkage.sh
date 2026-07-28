#!/usr/bin/env bash
set -euo pipefail

# The reviewed ONNX Runtime dylib carries the install name
# `@rpath/libonnxruntime.1.23.2.dylib`, so the MDReader executable only resolves
# it at launch when the binary carries a matching LC_RPATH and the dylib is
# present where that LC_RPATH points. Nothing in Tauri's macOS bundler adds the
# load command, so this contract asserts both halves — the rpath the linker
# recorded and the file dyld would find through it — before credentials are used
# (staged) and again on the signed application (bundled).
#
# Written for bash 3.2 (the macOS system shell): no mapfile, no associative
# arrays, no empty-array expansion under `set -u`.

MODE="${1:?mode is required (staged|bundled)}"
EXPECTED_RPATH='@executable_path/../Frameworks'
RUNTIME_DYLIB='libonnxruntime.1.23.2.dylib'

fail() {
  echo "$1" >&2
  exit 1
}

directory_identity() {
  stat -f '%d:%i' "$1"
}

architectures() {
  lipo -archs "$1"
}

# LC_RPATH paths recorded for one architecture slice.
load_command_rpaths() {
  otool -arch "$2" -l "$1" | awk '
    $1 == "cmd" { command = $2 }
    command == "LC_RPATH" && $1 == "path" { print $2 }
  '
}

# @rpath-relative dependencies recorded for one architecture slice. The first
# `otool -L` line is the file header, never a load command.
load_command_rpath_dependencies() {
  otool -arch "$2" -L "$1" | tail -n +2 | awk '$1 ~ /^@rpath\// { print $1 }'
}

verify_linkage() {
  binary="$1"
  frameworks="$2"

  [[ -f "$binary" && ! -L "$binary" ]] ||
    fail "MDReader executable is missing from $binary"
  [[ -d "$frameworks" && ! -L "$frameworks" ]] ||
    fail "packaged runtime directory is missing from $frameworks"

  archs="$(architectures "$binary")"
  [[ -n "$archs" ]] || fail 'MDReader executable exposes no architecture slice'

  for arch in $archs; do
    rpath_count=0
    while IFS= read -r rpath; do
      [[ -n "$rpath" ]] || continue
      rpath_count=$((rpath_count + 1))
      [[ "$rpath" == "$EXPECTED_RPATH" ]] ||
        fail "MDReader executable ($arch) records an unreviewed LC_RPATH: $rpath"
      resolved="$(dirname "$binary")/${rpath#@executable_path/}"
      if [[ "$MODE" == "bundled" ]]; then
        [[ -d "$resolved" ]] ||
          fail "MDReader executable ($arch) LC_RPATH does not resolve to a directory"
        [[ "$(directory_identity "$resolved")" == "$(directory_identity "$frameworks")" ]] ||
          fail "MDReader executable ($arch) LC_RPATH does not resolve to the packaged runtime directory"
      fi
    done <<< "$(load_command_rpaths "$binary" "$arch")"
    [[ "$rpath_count" -eq 1 ]] ||
      fail "MDReader executable ($arch) does not carry exactly one reviewed LC_RPATH"

    runtime_dependencies=0
    while IFS= read -r dependency; do
      [[ -n "$dependency" ]] || continue
      name="${dependency#@rpath/}"
      [[ "$name" != */* ]] ||
        fail "MDReader executable ($arch) loads a nested @rpath dependency: $dependency"
      [[ -f "$frameworks/$name" && ! -L "$frameworks/$name" ]] ||
        fail "MDReader executable ($arch) loads $dependency, which the packaged runtime does not provide"
      if [[ "$name" == "$RUNTIME_DYLIB" ]]; then
        runtime_dependencies=$((runtime_dependencies + 1))
      fi
    done <<< "$(load_command_rpath_dependencies "$binary" "$arch")"
    [[ "$runtime_dependencies" -eq 1 ]] ||
      fail "MDReader executable ($arch) is not linked to $RUNTIME_DYLIB exactly once"
  done
}

case "$MODE" in
  staged)
    BINARY="${2:?compiled executable is required}"
    STAGED_FRAMEWORKS="${3:?staged ONNX Runtime directory is required}"
    verify_linkage "$BINARY" "$STAGED_FRAMEWORKS"
    ;;
  bundled)
    APP_PATH="${2:?application bundle is required}"
    verify_linkage \
      "$APP_PATH/Contents/MacOS/mdreader-app" \
      "$APP_PATH/Contents/Frameworks"
    ;;
  *)
    fail "unsupported macOS runtime linkage mode: $MODE"
    ;;
esac
